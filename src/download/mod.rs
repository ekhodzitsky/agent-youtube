#[cfg(test)]
mod faults;
mod source;
use source::*;
mod gate;
pub(crate) mod http;
mod lease;
#[cfg(test)]
mod metrics;
mod recovery;
mod sabr;
mod samsung;
pub(crate) mod tls;
mod wire;

use crate::{Result, storage, web::USER_AGENT as WEB_USER_AGENT};
use base64::{
    Engine,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use http::{Http, check};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

const METADATA_LIMIT: usize = 16 * 1024 * 1024;
const RANGE_BYTES: u64 = 1024 * 1024;
const USER_AGENT: &str =
    "com.google.ios.youtube/21.26.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
const CLIENT_VERSION: &str = "21.26.4";
const MWEB_USER_AGENT: &str = "Mozilla/5.0 (iPad; CPU OS 16_7_10 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1,gzip(gfe)";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum WebClient {
    Web,
    Mweb,
    Samsung,
    Visionos,
    Embedded,
}
impl WebClient {
    fn requires_attestation(self) -> bool {
        matches!(self, Self::Web | Self::Mweb)
    }
}

#[derive(Clone)]
pub struct Downloader {
    http: Http,
    runtime: Arc<Mutex<Option<crate::web::Worker>>>,
    bootstrap: Arc<Mutex<Option<(Instant, Source)>>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    video_id: String,
    itag: u64,
    last_modified: String,
    xtags: String,
    length: u64,
    mime: String,
    direct: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_track: Option<String>,
}
#[derive(Clone)]
struct Source {
    raw: Vec<u8>,
    duration_seconds: Option<u64>,
    identity: Identity,
    url: String,
    config: Vec<u8>,
    client: Value,
    user_agent: String,
    video_format: Vec<u8>,
    format: Vec<u8>,
    direct: bool,
    session: sabr::Session,
    web_client: Option<WebClient>,
    player_script: Option<String>,
    token_expires: Option<Instant>,
    expires_at: Option<u64>,
}
#[derive(Default, Serialize, Deserialize, Clone)]
struct Progress {
    bytes: u64,
    digest: String,
    #[serde(default)]
    init: bool,
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(default)]
    end_ms: u64,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    version: u32,
    identity: Identity,
    progress: Progress,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web_client: Option<WebClient>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_seconds: Option<u64>,
}

impl Downloader {
    pub fn youtube() -> Result<Self> {
        Ok(Self {
            http: Http::new("https://www.youtube.com")?,
            runtime: Arc::new(Mutex::new(None)),
            bootstrap: Arc::new(Mutex::new(None)),
        })
    }
    /// An explicit loopback server for embedding/offline HTTP protocol tests.
    /// The CLI never accepts a replacement YouTube origin.
    pub fn loopback(origin: &str) -> Result<Self> {
        if !origin.starts_with("http://127.0.0.1:") {
            return Err("expected a loopback test origin".into());
        }
        Ok(Self {
            http: Http::new(origin)?,
            runtime: Arc::new(Mutex::new(None)),
            bootstrap: Arc::new(Mutex::new(None)),
        })
    }
    fn source(
        &self,
        id: &str,
        cancelled: &AtomicBool,
        wanted: Option<&Identity>,
    ) -> Result<Source> {
        if self.http.origin == "https://www.youtube.com" {
            return self.web_source(id, None, cancelled, wanted);
        }
        let body = serde_json::to_vec(&serde_json::json!({
            "videoId": id, "context": {"client": {"clientName":"IOS", "clientVersion":CLIENT_VERSION,
            "deviceMake":"Apple", "deviceModel":"iPhone16,2", "osName":"iPhone", "osVersion":"18.3.2.22D82", "hl":"en", "gl":"US"}},
            "contentCheckOk":true, "racyCheckOk":true
        }))?;
        let url = format!("{}/youtubei/v1/player?prettyPrint=false", self.http.origin);
        let mut response = self.http.request(
            &url,
            Some(&body),
            &[
                ("Content-Type", "application/json"),
                ("User-Agent", USER_AGENT),
            ],
            cancelled,
        )?;
        if response.status().as_u16() != 200 {
            return Err(format!(
                "YouTube player returned HTTP {}",
                response.status().as_u16()
            )
            .into());
        }
        let raw = http::read(&mut response, METADATA_LIMIT, cancelled)?;
        match parse_source_for(raw, id, &self.http, wanted) {
            Ok(source) => Ok(source),
            Err(api_error)
                if api_error
                    .downcast_ref::<crate::protocol::Failure>()
                    .is_some_and(|failure| {
                        matches!(
                            failure.code,
                            "youtube_source_unavailable"
                                | "youtube_bot_check"
                                | "youtube_rate_limited"
                        )
                    }) =>
            {
                check(cancelled)?;
                let url = format!("{}/watch?v={id}&hl=en", self.http.origin);
                let mut page =
                    self.http
                        .request(&url, None, &[("User-Agent", "Mozilla/5.0")], cancelled)?;
                if page.status().as_u16() != 200 {
                    return Err(api_error);
                }
                let page = http::read(&mut page, METADATA_LIMIT, cancelled)?;
                let page = std::str::from_utf8(&page)?;
                let player =
                    match embedded_json(page, "ytInitialPlayerResponse", "playabilityStatus") {
                        Ok(player) => player,
                        Err(_) => return Err(api_error),
                    };
                let mut source =
                    parse_source_for(serde_json::to_vec(&player)?, id, &self.http, wanted)?;
                let config = embedded_json(page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
                let client = config
                    .pointer("/INNERTUBE_CONTEXT/client")
                    .ok_or("watch page has no client context")?
                    .clone();
                source.client = client;
                source.user_agent = "Mozilla/5.0".into();
                Ok(source)
            }
            Err(error) => Err(error),
        }
    }
    fn with_runtime<T>(
        &self,
        operation: impl FnOnce(&mut crate::web::Worker) -> Result<T>,
    ) -> Result<T> {
        let mut runtime = self.runtime.lock().map_err(|_| "Web runtime lock failed")?;
        if runtime.is_none() {
            *runtime = Some(crate::web::Worker::start()?)
        }
        let result = operation(runtime.as_mut().ok_or("Web runtime unavailable")?);
        if result.is_err()
            && let Some(mut worker) = runtime.take()
        {
            worker.close()?;
        }
        result
    }
    fn web_source(
        &self,
        id: &str,
        client: Option<WebClient>,
        cancelled: &AtomicBool,
        wanted: Option<&Identity>,
    ) -> Result<Source> {
        recover_request(
            || {
                if let Some(client) = client {
                    return self.web_source_once(id, client, cancelled, wanted);
                }
                let mut last = None;
                for client in [
                    WebClient::Web,
                    WebClient::Mweb,
                    WebClient::Samsung,
                    WebClient::Visionos,
                    WebClient::Embedded,
                ] {
                    match self.web_source_once(id, client, cancelled, wanted) {
                        Err(error) if recovery::source_failure(error.as_ref()) => {
                            last = Some(error)
                        }
                        result => return result,
                    }
                }
                Err(last.ok_or("no qualified YouTube clients")?)
            },
            |millis| http::pause(millis, cancelled),
            cancelled,
            true,
        )
    }
    fn web_source_once(
        &self,
        id: &str,
        client: WebClient,
        cancelled: &AtomicBool,
        wanted: Option<&Identity>,
    ) -> Result<Source> {
        if client == WebClient::Samsung {
            return self.samsung_source(id, cancelled, wanted);
        }
        if matches!(client, WebClient::Visionos | WebClient::Embedded) {
            return self.anonymous_source(id, client, cancelled, wanted);
        }
        let (name, user_agent) = match client {
            WebClient::Web => ("WEB", WEB_USER_AGENT),
            WebClient::Mweb => ("MWEB", MWEB_USER_AGENT),
            WebClient::Samsung => ("TVHTML5", samsung::USER_AGENT),
            WebClient::Visionos => ("VISIONOS", samsung::VISIONOS_USER_AGENT),
            WebClient::Embedded => ("WEB_EMBEDDED_PLAYER", samsung::EMBEDDED_USER_AGENT),
        };
        let origin = if client == WebClient::Mweb && self.http.origin == "https://www.youtube.com" {
            "https://m.youtube.com"
        } else {
            &self.http.origin
        };
        let (page, cookies) = self.watch_page(id, origin, user_agent, cancelled)?;
        let page = page.as_str();
        let player = embedded_json(page, "ytInitialPlayerResponse", "playabilityStatus")?;
        let mut source =
            match parse_source_for(serde_json::to_vec(&player)?, id, &self.http, wanted) {
                Ok(source) => source,
                Err(error)
                    if error
                        .downcast_ref::<crate::protocol::Failure>()
                        .is_some_and(|failure| failure.code == "youtube_bot_check") =>
                {
                    match self.watch_player_source(id, client, page, &cookies, wanted, cancelled)? {
                        Some(source) => source,
                        None => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            };
        let config = embedded_json(page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
        source.client = config
            .pointer("/INNERTUBE_CONTEXT/client")
            .ok_or("watch page has no Web client")?
            .clone();
        if source.client["clientName"].as_str() != Some(name) {
            return Err("watch page returned an unsupported client".into());
        }
        source.user_agent = user_agent.into();
        source.web_client = Some(client);
        if query_value(&source.url, "n").is_some() {
            source.player_script = Some(player_script_path(page)?);
        }
        Ok(source)
    }
    fn watch_player_source(
        &self,
        id: &str,
        selected: WebClient,
        page: &str,
        cookies: &str,
        wanted: Option<&Identity>,
        cancelled: &AtomicBool,
    ) -> Result<Option<Source>> {
        let (name, number, label, user_agent) = match selected {
            WebClient::Web => ("WEB", "1", "web", WEB_USER_AGENT),
            WebClient::Mweb => ("MWEB", "2", "mweb", MWEB_USER_AGENT),
            _ => return Ok(None),
        };
        let Ok(config) = embedded_json(page, "ytcfg.set(", "INNERTUBE_CONTEXT") else {
            return Ok(None);
        };
        let context = &config["INNERTUBE_CONTEXT"];
        let client = &context["client"];
        let Some(version) = client["clientVersion"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 128 && value.is_ascii())
        else {
            return Ok(None);
        };
        let Some(visitor) = client["visitorData"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 8192 && value.is_ascii())
        else {
            return Ok(None);
        };
        let Some(timestamp) = config["STS"]
            .as_u64()
            .filter(|value| (1..=u64::from(u32::MAX)).contains(value))
        else {
            return Ok(None);
        };
        if client["clientName"] != name {
            return Ok(None);
        }
        check(cancelled)?;
        crate::protocol::progress(
            "download",
            "requesting player API after a bot-denied watch response",
            serde_json::json!({"client":label,"route":"player_api"}),
        );
        let body = serde_json::to_vec(&serde_json::json!({"videoId":id,"context":context,
            "playbackContext":{"contentPlaybackContext":{"html5Preference":"HTML5_PREF_WANTS","signatureTimestamp":timestamp}},
            "contentCheckOk":true,"racyCheckOk":true}))?;
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("User-Agent", user_agent),
            ("X-YouTube-Client-Name", number),
            ("X-YouTube-Client-Version", version),
            ("X-Goog-Visitor-Id", visitor),
            ("Origin", self.http.origin.as_str()),
        ];
        if !cookies.is_empty() {
            headers.push(("Cookie", cookies));
        }
        let mut response = self.http.request(
            &format!("{}/youtubei/v1/player?prettyPrint=false", self.http.origin),
            Some(&body),
            &headers,
            cancelled,
        )?;
        let status = response.status().as_u16();
        if status == 429 || (status >= 400 && response.headers().contains_key("retry-after")) {
            return Err(http::rate_limited(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()),
                "YouTube player API requested a cooldown",
            ));
        }
        if status != 200 {
            let transient = matches!(status, 408 | 500 | 502 | 503 | 504);
            return Err(crate::protocol::failure_details(
                if transient {
                    "youtube_temporary_failure"
                } else {
                    "youtube_http_error"
                },
                Some(transient),
                "YouTube player API unavailable",
                serde_json::json!({"http_status":status,"client":label,"route":"player_api"}),
            ));
        }
        parse_source_for(
            http::read(&mut response, METADATA_LIMIT, cancelled)?,
            id,
            &self.http,
            wanted,
        )
        .map(Some)
    }
    fn watch_page(
        &self,
        id: &str,
        origin: &str,
        user_agent: &str,
        cancelled: &AtomicBool,
    ) -> Result<(String, String)> {
        let mut response = self.http.request(
            &format!("{origin}/watch?v={id}&hl=en"),
            None,
            &[("User-Agent", user_agent)],
            cancelled,
        )?;
        let status = response.status().as_u16();
        if status == 429 {
            return Err(http::rate_limited(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok()),
                "YouTube watch returned HTTP 429; retry later",
            ));
        }
        if status >= 400
            && let Some(retry_after) = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
        {
            return Err(http::rate_limited(
                Some(retry_after),
                format!("YouTube watch returned HTTP {status}; retry later"),
            ));
        }
        if status != 200 {
            let (code, retryable) = match status {
                408 | 500 | 502 | 503 | 504 => ("youtube_temporary_failure", true),
                _ => ("youtube_http_error", false),
            };
            return Err(crate::protocol::failure_details(
                code,
                Some(retryable),
                format!("YouTube watch returned HTTP {status}"),
                serde_json::json!({"http_status":status,"route":"watch"}),
            ));
        }
        let cookies = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let page = String::from_utf8(http::read(&mut response, METADATA_LIMIT, cancelled)?)?;
        Ok((page, cookies))
    }
    fn prepare_web(
        &self,
        source: &mut Source,
        id: &str,
        refresh: bool,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        if source.web_client.is_none() {
            return Ok(());
        }
        if let Some(n) = query_value(&source.url, "n") {
            let path = source
                .player_script
                .as_deref()
                .ok_or("missing player script path")?;
            let player = recover_request(
                || {
                    let mut response = self.http.request(
                        &format!("{}{path}", self.http.origin),
                        None,
                        &[("User-Agent", &source.user_agent)],
                        cancelled,
                    )?;
                    let status = response.status().as_u16();
                    if status == 429
                        || (status >= 400 && response.headers().contains_key("retry-after"))
                    {
                        return Err(http::rate_limited(
                            response
                                .headers()
                                .get("retry-after")
                                .and_then(|value| value.to_str().ok()),
                            "player script server requested a cooldown",
                        ));
                    }
                    if status != 200 {
                        let transient = matches!(status, 408 | 500..=599);
                        return Err(crate::protocol::failure_details(
                            if transient {
                                "youtube_temporary_failure"
                            } else {
                                "youtube_http_error"
                            },
                            Some(transient),
                            "player script unavailable",
                            serde_json::json!({"http_status":status}),
                        ));
                    }
                    Ok(String::from_utf8(http::read(
                        &mut response,
                        METADATA_LIMIT,
                        cancelled,
                    )?)?)
                },
                |millis| http::pause(millis, cancelled),
                cancelled,
                false,
            )?;
            let solution =
                self.with_runtime(|runtime| runtime.solve_n(player, n.into(), cancelled))?;
            source.url = replace_query(&source.url, "n", &solution)?;
        }
        self.refresh_token(source, id, refresh, cancelled)
    }
    fn refresh_token(
        &self,
        source: &mut Source,
        id: &str,
        refresh: bool,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        if !source
            .web_client
            .is_some_and(WebClient::requires_attestation)
        {
            return Ok(());
        }
        if self.http.origin == "https://www.youtube.com" {
            gate::Gate::user()?.before(false, cancelled)?;
        }
        let token = recover_request(
            || self.with_runtime(|runtime| runtime.mint(id, refresh, cancelled)),
            |millis| http::pause(millis, cancelled),
            cancelled,
            false,
        )?;
        if source.direct {
            use base64::engine::general_purpose::URL_SAFE_NO_PAD;
            source.url = replace_query(&source.url, "pot", &URL_SAFE_NO_PAD.encode(&token.bytes))?;
        }
        source.session.po_token = token.bytes;
        source.token_expires = Some(token.expires);
        #[cfg(test)]
        faults::token_refreshed();
        Ok(())
    }
    pub fn metadata(&self, id: &str, cancelled: &AtomicBool) -> Result<Vec<u8>> {
        let source = self.source(id, cancelled, None)?;
        let raw = source.raw.clone();
        *self
            .bootstrap
            .lock()
            .map_err(|_| "source bootstrap lock failed")? = Some((Instant::now(), source));
        Ok(raw)
    }
    /// The prefix must already be durably owned by the caller's checkpoint.
    pub fn download(&self, id: &str, prefix: &Path, cancelled: &AtomicBool) -> Result<PathBuf> {
        let outcome = self.download_impl(id, prefix, cancelled);
        // Long-lived callers must not retain the interpreter or token state after
        // a media, persistence or cancellation failure.
        let worker = self
            .runtime
            .lock()
            .map_err(|_| "Web runtime lock failed")?
            .take();
        if let Some(mut worker) = worker {
            worker.close()?;
        }
        outcome.map_err(|e| {
            if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                return crate::protocol::failure_details(
                    "interrupted",
                    Some(true),
                    "download interrupted; run the same command to resume",
                    serde_json::json!({"stage":"download","path":prefix,"preserved":true}),
                );
            }
            let e = storage::audio_write_error(e, prefix, "download");
            if e.is::<crate::protocol::Failure>() {
                return e;
            }
            let io = e.is::<std::io::Error>();
            crate::protocol::failure_details(
                if io {
                    "download_io"
                } else {
                    "download_protocol_invalid"
                },
                if io { None } else { Some(false) },
                e,
                serde_json::json!({"stage":"download","path":prefix,"preserved":true}),
            )
        })
    }
    fn download_impl(&self, id: &str, prefix: &Path, cancelled: &AtomicBool) -> Result<PathBuf> {
        check(cancelled)?;
        let path = suffix(prefix, ".m4a");
        let journal = suffix(prefix, ".json");
        // Never follow links or open unknown files before validating their receipts.
        owned(&path)?;
        owned(&journal)?;
        let mut retained: Option<Receipt> = if journal.exists() {
            Some(serde_json::from_str(&storage::read_text(
                &journal,
                1024 * 1024,
            )?)?)
        } else {
            None
        };
        let replace_receipt = retained.is_some();
        if retained
            .as_ref()
            .is_some_and(|receipt| receipt.version != 1)
        {
            return Err("unsupported download receipt; partial media preserved".into());
        }
        if retained.as_ref().is_some_and(|receipt| {
            receipt.identity.video_id != id || receipt.progress.bytes > receipt.identity.length
        }) {
            return Err(
                "invalid download receipt identity/progress; partial media preserved".into(),
            );
        }
        let verified = if let Some(receipt) = &retained {
            if path.exists() {
                let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
                let hasher = verify_partial(&mut file, &receipt.progress)?;
                Some((file, hasher))
            } else {
                if receipt.progress.bytes != 0 {
                    return Err("missing partial download; prior receipt preserved".into());
                }
                None
            }
        } else {
            if path.exists() {
                return Err(
                    "unreceipted download file; preserve it and use a new --work-dir".into(),
                );
            }
            None
        };
        let pinned = retained.as_ref().and_then(|receipt| receipt.web_client);
        let saved = self
            .bootstrap
            .lock()
            .map_err(|_| "source bootstrap lock failed")?
            .take();
        let cached = retained
            .as_ref()
            .map(|receipt| lease::load(prefix, receipt, id, &self.http))
            .transpose()?
            .flatten();
        if let (Some(receipt), Some((file, _)), Some(_)) = (&retained, &verified, &cached)
            && receipt.progress.bytes == receipt.identity.length
        {
            check(cancelled)?;
            // Both the whole file hash and the receipt-bound source identity
            // have been checked. Expired URLs cannot invalidate durable bytes.
            file.set_len(receipt.identity.length)?;
            file.sync_all()?;
            return Ok(path);
        }
        let cached = match cached {
            Some(source) if !lease::expired(&source)? => Some(source),
            _ => None,
        };
        let selected = cached.or_else(|| {
            saved
                .filter(|(created, source)| {
                    source.identity.video_id == id
                        && created.elapsed().as_secs() < 60
                        && pinned.is_none_or(|client| source.web_client == Some(client))
                        && retained
                            .as_ref()
                            .is_none_or(|receipt| receipt.identity == source.identity)
                })
                .map(|(_, source)| source)
        });
        let (mut source, prepared) = match selected {
            Some(source) if !lease::expired(&source)? => (source, false),
            _ => match retained.as_mut() {
                Some(receipt) => (
                    self.replace_source(id, prefix, receipt, &[], cancelled)?,
                    true,
                ),
                None => (self.source(id, cancelled, None)?, false),
            },
        };
        let mut receipt = if let Some(receipt) = retained {
            if receipt.identity.video_id != source.identity.video_id
                || receipt.identity.audio_track != source.identity.audio_track
            {
                return Err(crate::protocol::failure(
                    "download_audio_track_changed",
                    Some(false),
                    "source changed video or audio track; partial media preserved",
                ));
            }
            if receipt.identity != source.identity {
                return Err(crate::protocol::failure(
                    "download_representation_changed",
                    Some(false),
                    "download representation changed; use a new --work-dir; partial media preserved",
                ));
            }
            receipt
        } else {
            if path.exists() {
                return Err(
                    "unreceipted download file; preserve it and use a new --work-dir".into(),
                );
            }
            Receipt {
                version: 1,
                identity: source.identity.clone(),
                etag: None,
                web_client: source.web_client,
                lease: None,
                duration_seconds: source.duration_seconds,
                progress: Progress {
                    digest: format!("{:x}", Sha256::digest([])),
                    ..Progress::default()
                },
            }
        };
        if lease::expired(&source)? {
            return Err(crate::protocol::failure(
                "youtube_temporary_failure",
                Some(true),
                "source URL expires too soon; partial media preserved",
            ));
        }
        let parent = prefix.parent().unwrap_or(Path::new("."));
        let space = storage::disk_space_at(parent, "download")?;
        space.require_at(
            &path,
            space
                .allocation(
                    source
                        .identity
                        .length
                        .saturating_sub(receipt.progress.bytes),
                )?
                .saturating_add(64 * 1024),
            "download",
        )?;
        space.require_files(parent, if path.exists() { 2 } else { 4 }, "download")?;
        receipt.web_client = source.web_client;
        receipt.duration_seconds = source.duration_seconds.or(receipt.duration_seconds);
        receipt.lease = lease::save(&source, prefix)?;
        storage::atomic_write(&journal, &serde_json::to_vec(&receipt)?, replace_receipt)?;
        let (mut file, mut hasher) = if let Some(verified) = verified {
            verified
        } else {
            let mut file = storage::private_file().read(true).open(&path)?;
            let hasher = verify_partial(&mut file, &receipt.progress)?;
            (file, hasher)
        };
        file.set_len(receipt.progress.bytes)?;
        file.seek(SeekFrom::Start(receipt.progress.bytes))?;
        file.sync_all()?;
        let mut refused_clients = Vec::new();
        if !prepared && let Err(error) = self.prepare_web(&mut source, id, false, cancelled) {
            if !recovery::client_failure(error.as_ref()) {
                return Err(error);
            }
            refused_clients.push(source.web_client);
            source = self.replace_source(id, prefix, &mut receipt, &refused_clients, cancelled)?;
        }
        let mut failures = 0;
        let mut refreshed_attestation = false;
        let mut renewed_stalled_source = false;
        let mut stable_retry_used = false;
        let mut refresh_before_exclusion = false;
        while receipt.progress.bytes < source.identity.length {
            check(cancelled)?;
            #[cfg(test)]
            faults::before_cycle(&mut source, &receipt);
            if lease::expired(&source)? {
                failures += 1;
                if failures > 3 {
                    return Err(http::media_failure(
                        "download_source_expired",
                        None,
                        "source repeatedly expired; media retained for resume",
                    ));
                }
                source =
                    self.replace_source(id, prefix, &mut receipt, &refused_clients, cancelled)?;
                refresh_before_exclusion = false;
                continue;
            }
            if source
                .web_client
                .is_some_and(WebClient::requires_attestation)
                && source
                    .token_expires
                    .is_none_or(|expires| Instant::now() >= expires)
                && let Err(error) = self.refresh_token(&mut source, id, false, cancelled)
            {
                if !recovery::client_failure(error.as_ref()) {
                    return Err(error);
                }
                refused_clients.push(source.web_client);
                source =
                    self.replace_source(id, prefix, &mut receipt, &refused_clients, cancelled)?;
                refresh_before_exclusion = false;
            }
            let before = receipt.progress.bytes;
            let result = if source.direct {
                self.range(
                    &source,
                    &mut file,
                    &mut receipt,
                    &journal,
                    &mut hasher,
                    cancelled,
                )
            } else {
                sabr::download_cycle(
                    self,
                    &mut source,
                    &mut file,
                    &mut receipt,
                    &journal,
                    &mut hasher,
                    cancelled,
                )
            };
            if let Err(error) = result {
                check(cancelled)?;
                // Protocol/identity errors are not silently retried or reset.
                let failure = error.downcast_ref::<crate::protocol::Failure>();
                if !failure.is_some_and(|f| {
                    matches!(
                        f.code,
                        "download_transport"
                            | "download_http_temporary"
                            | "download_incomplete"
                            | "download_refresh_required"
                    )
                }) {
                    return Err(error);
                }
                if before == receipt.progress.bytes {
                    failures += 1;
                } else {
                    failures = 1;
                }
                let refresh = failure.is_some_and(|f| {
                    matches!(f.http_status(), Some(403 | 410))
                        || f.code == "download_refresh_required"
                });
                // Each excluded client allows one remaining source attempt;
                // ordinary transport retries retain the original three-retry cap.
                let limit = if refresh {
                    3 + refused_clients.len()
                } else {
                    3
                };
                if failures as usize > limit {
                    return Err(crate::protocol::with_attempts(error, failures));
                }
                let retry_deadline = lease::now()?.saturating_add(65);
                if !stable_retry_used
                    && source.direct
                    && failure.is_some_and(|f| f.http_status() == Some(403))
                    && source
                        .expires_at
                        .is_some_and(|expiry| expiry > retry_deadline)
                    && (!source
                        .web_client
                        .is_some_and(WebClient::requires_attestation)
                        || source.token_expires.is_some_and(|expiry| {
                            expiry.saturating_duration_since(Instant::now())
                                > Duration::from_secs(5)
                        }))
                {
                    // A fresh direct URL can become usable without replacing its
                    // context. Charge this one retry to the existing failure budget.
                    stable_retry_used = true;
                    refresh_before_exclusion = true;
                    http::pause(5_000, cancelled)?;
                    continue;
                }
                http::pause(250 << (failures - 1), cancelled)?;
                if refresh {
                    if failures > 1
                        && !refresh_before_exclusion
                        && !refused_clients.contains(&source.web_client)
                    {
                        refused_clients.push(source.web_client);
                    }
                    source =
                        self.replace_source(id, prefix, &mut receipt, &refused_clients, cancelled)?;
                    refresh_before_exclusion = false;
                }
            } else if receipt.progress.bytes == before {
                failures += 1;
                if failures > 3 {
                    if source
                        .web_client
                        .is_some_and(WebClient::requires_attestation)
                        && !refreshed_attestation
                    {
                        if let Err(error) = self.refresh_token(&mut source, id, true, cancelled) {
                            if !recovery::client_failure(error.as_ref()) {
                                return Err(error);
                            }
                            refused_clients.push(source.web_client);
                            source = self.replace_source(
                                id,
                                prefix,
                                &mut receipt,
                                &refused_clients,
                                cancelled,
                            )?;
                        }
                        refreshed_attestation = true;
                        failures = 0;
                    } else if !source.direct && !renewed_stalled_source {
                        // A live URL and fresh token do not guarantee that the
                        // persisted SABR context still yields audio. Renew once
                        // without discarding its verified representation/prefix.
                        source = self.replace_source(
                            id,
                            prefix,
                            &mut receipt,
                            &refused_clients,
                            cancelled,
                        )?;
                        #[cfg(test)]
                        metrics::stalled_source_renewed();
                        renewed_stalled_source = true;
                        refreshed_attestation = true;
                        refresh_before_exclusion = false;
                        failures = 0;
                    } else {
                        return Err(http::media_failure(
                            "download_no_progress",
                            None,
                            "download made no progress; media retained for resume",
                        ));
                    }
                }
                http::pause(250, cancelled)?;
            } else {
                failures = 0;
                refresh_before_exclusion = false;
                refreshed_attestation = false;
                renewed_stalled_source = false;
                crate::protocol::progress(
                    "download",
                    "committed audio bytes",
                    serde_json::json!({"downloaded_bytes":receipt.progress.bytes,"total_bytes":source.identity.length,"durable":true}),
                );
            }
        }
        if receipt.progress.bytes != receipt.identity.length {
            return Err("download length does not match its representation".into());
        }
        file.sync_all()?;
        if storage::digest(&path)? != receipt.progress.digest {
            return Err("download failed final integrity verification".into());
        }
        Ok(path)
    }
    fn replace_source(
        &self,
        id: &str,
        prefix: &Path,
        receipt: &mut Receipt,
        excluded: &[Option<WebClient>],
        cancelled: &AtomicBool,
    ) -> Result<Source> {
        let mut excluded = excluded.to_vec();
        let mut etag = receipt.etag.clone();
        let source = loop {
            let mut source = recovery::select(
                receipt,
                &excluded,
                |client| {
                    if let Some(client) = client {
                        self.web_source_once(id, client, cancelled, Some(&receipt.identity))
                    } else {
                        self.source(id, cancelled, Some(&receipt.identity))
                    }
                },
                cancelled,
            )?;
            if let Err(error) = self.prepare_web(&mut source, id, true, cancelled) {
                if recovery::client_failure(error.as_ref()) {
                    excluded.push(source.web_client);
                    continue;
                }
                return Err(error);
            }
            if source.web_client != receipt.web_client || source.direct != receipt.identity.direct {
                match self.verify_source_prefix(&source, receipt, cancelled) {
                    Ok(observed) => etag = observed,
                    Err(error) => {
                        if error
                            .downcast_ref::<crate::protocol::Failure>()
                            .is_some_and(|failure| matches!(failure.http_status(), Some(403 | 410)))
                        {
                            excluded.push(source.web_client);
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
            break source;
        };
        if source.direct != receipt.identity.direct {
            receipt.identity = source.identity.clone();
            receipt.progress.init = false;
            receipt.progress.sequence = None;
            receipt.progress.end_ms = 0;
        }
        receipt.etag = etag;
        receipt.web_client = source.web_client;
        receipt.duration_seconds = source.duration_seconds.or(receipt.duration_seconds);
        receipt.lease = lease::save(&source, prefix)?;
        storage::atomic_write(
            &suffix(prefix, ".json"),
            &serde_json::to_vec(receipt)?,
            true,
        )?;
        #[cfg(test)]
        faults::renewed();
        Ok(source)
    }
    pub(crate) fn same_track(previous: &Path, current: &Path) -> Result<()> {
        let identity = |prefix: &Path| -> Result<Identity> {
            let path = suffix(prefix, ".json");
            owned(&path)?;
            let receipt: Receipt = serde_json::from_str(&storage::read_text(&path, 1024 * 1024)?)?;
            if receipt.version != 1 {
                return Err("unsupported acquisition receipt".into());
            }
            Ok(receipt.identity)
        };
        let previous = identity(previous)?;
        let current = identity(current)?;
        if previous.video_id != current.video_id || previous.audio_track != current.audio_track {
            return Err(crate::protocol::failure(
                "download_audio_track_changed",
                Some(false),
                "independent acquisition changed video or audio track; all media preserved",
            ));
        }
        Ok(())
    }
    pub(crate) fn evidence(prefix: &Path, audio: &Path) -> Result<Vec<u8>> {
        let path = suffix(prefix, ".json");
        owned(&path)?;
        let receipt: Receipt = serde_json::from_str(&storage::read_text(&path, 1024 * 1024)?)?;
        if receipt.version != 1
            || receipt.progress.bytes != receipt.identity.length
            || std::fs::metadata(audio)?.len() != receipt.identity.length
            || storage::digest(audio)? != receipt.progress.digest
        {
            return Err("download evidence does not match source".into());
        }
        // Identity and committed bytes only; URLs, tokens and sessions are excluded.
        Ok(serde_json::to_vec(
            &serde_json::json!({"status":"verified","representation":receipt.identity,"sha256":receipt.progress.digest,"bytes":receipt.progress.bytes,"duration_seconds":receipt.duration_seconds}),
        )?)
    }
    fn range(
        &self,
        source: &Source,
        file: &mut File,
        receipt: &mut Receipt,
        journal: &Path,
        hasher: &mut Sha256,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let start = receipt.progress.bytes;
        let end = start
            .saturating_add(RANGE_BYTES - 1)
            .min(source.identity.length - 1);
        let (bytes, etag) =
            self.range_bytes(source, start, end, receipt.etag.as_deref(), cancelled)?;
        receipt.etag = etag;
        commit(file, receipt, journal, hasher, &bytes, None)?;
        Ok(())
    }
    fn verify_source_prefix(
        &self,
        source: &Source,
        receipt: &Receipt,
        cancelled: &AtomicBool,
    ) -> Result<Option<String>> {
        let mut hasher = Sha256::new();
        let mut start = 0;
        let mut etag = receipt.etag.clone();
        while start < receipt.progress.bytes {
            check(cancelled)?;
            let end = start
                .saturating_add(RANGE_BYTES - 1)
                .min(receipt.progress.bytes - 1);
            let (bytes, observed) =
                self.range_bytes(source, start, end, etag.as_deref(), cancelled)?;
            etag = observed;
            hasher.update(bytes);
            start = end + 1;
        }
        if format!("{:x}", hasher.finalize()) != receipt.progress.digest {
            return Err(crate::protocol::failure(
                "download_source_mismatch",
                Some(false),
                "alternate client prefix differs from retained audio; partial media preserved",
            ));
        }
        #[cfg(test)]
        metrics::replay(receipt.progress.bytes);
        Ok(etag)
    }
    fn range_bytes(
        &self,
        source: &Source,
        start: u64,
        end: u64,
        expected_etag: Option<&str>,
        cancelled: &AtomicBool,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let range = format!("bytes={start}-{end}");
        let mut headers = vec![
            ("Range", range.as_str()),
            ("User-Agent", source.user_agent.as_str()),
        ];
        if let Some(etag) = expected_etag {
            headers.push(("If-Range", etag));
        }
        let mut response = http::retry_transport(
            self.http.request(&source.url, None, &headers, cancelled),
            "HTTP range transport failed",
        )?;
        let status = response.status().as_u16();
        if status == 429 || (status >= 400 && response.headers().contains_key("retry-after")) {
            return Err(http::rate_limited(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok()),
                format!("audio server returned HTTP {status}; media retained for resume"),
            ));
        }
        if matches!(status, 403 | 408 | 410 | 500..=599) {
            return Err(http::media_failure(
                "download_http_temporary",
                Some(status),
                format!("audio server returned HTTP {status}"),
            ));
        }
        if status != 206 {
            return Err(format!("expected HTTP 206 for a byte range, received {status}").into());
        }
        let expected = format!("bytes {start}-{end}/{}", source.identity.length);
        if response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            != Some(expected.as_str())
        {
            return Err("Content-Range does not match the requested audio span".into());
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.starts_with('"') && v.ends_with('"'))
            .map(str::to_owned);
        if expected_etag.is_some() && expected_etag != etag.as_deref() {
            return Err("audio ETag changed during download; partial media preserved".into());
        }
        let count = end - start + 1;
        if response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n != count)
        {
            return Err("Content-Length does not match the requested range".into());
        }
        let bytes =
            http::read(&mut response, usize::try_from(count + 1)?, cancelled).map_err(|e| {
                if e.downcast_ref::<crate::protocol::Failure>()
                    .is_some_and(|f| f.code == "http_transport")
                {
                    http::media_failure("download_incomplete", None, "incomplete HTTP range body")
                } else {
                    e
                }
            })?;
        if bytes.len() as u64 != count {
            return Err(http::media_failure(
                "download_incomplete",
                None,
                "truncated HTTP range body",
            ));
        }
        Ok((bytes, etag))
    }
}
fn recover_request<T>(
    mut request: impl FnMut() -> Result<T>,
    mut pause: impl FnMut(u64) -> Result<()>,
    cancelled: &AtomicBool,
    source_rejections: bool,
) -> Result<T> {
    for attempt in 0..4 {
        check(cancelled)?;
        match request() {
            Ok(source) => return Ok(source),
            Err(error) => {
                check(cancelled)?;
                let retryable = error
                    .downcast_ref::<crate::protocol::Failure>()
                    .is_some_and(|failure| {
                        matches!(failure.code, "youtube_temporary_failure" | "http_transport")
                            || (source_rejections
                                && matches!(
                                    failure.code,
                                    "youtube_bot_check" | "youtube_rate_limited"
                                ))
                    });
                if !retryable {
                    return Err(error);
                }
                if attempt == 3 {
                    return Err(crate::protocol::with_attempts(error, 4));
                }
                let code = error
                    .downcast_ref::<crate::protocol::Failure>()
                    .map(|failure| failure.code)
                    .unwrap_or("source_failed");
                // A fresh anonymous request may succeed after a transient rejection;
                // never loop indefinitely or retry account/content restrictions.
                if error
                    .downcast_ref::<crate::protocol::Failure>()
                    .is_some_and(|failure| {
                        failure.retry_after().is_some() && failure.retry_after_seconds().is_none()
                    })
                {
                    return Err(error);
                }
                let suggested = error
                    .downcast_ref::<crate::protocol::Failure>()
                    .and_then(|failure| failure.retry_after_seconds())
                    .unwrap_or(0);
                // Do not retry earlier than the server requests or hold an agent
                // indefinitely. Longer waits are returned as structured guidance.
                if suggested > 30 {
                    return Err(error);
                }
                let delay = (5000 << attempt).max(suggested * 1000);
                crate::protocol::progress(
                    "retry",
                    format!(
                        "YouTube request {code}; retry {}/3 after {} seconds",
                        attempt + 1,
                        delay / 1000
                    ),
                    serde_json::json!({"cause_code":code,"attempt":attempt+1,"retry_after_seconds":delay/1000}),
                );
                pause(delay)?;
            }
        }
    }
    Err("source recovery exhausted".into())
}
fn suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut path = prefix.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}
fn owned(path: &Path) -> Result<()> {
    storage::reject_nonregular(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(path).is_ok_and(|m| m.nlink() != 1) {
            return Err("unsafe download scratch: hard-linked file".into());
        }
    }
    Ok(())
}
fn verify_partial(file: &mut File, progress: &Progress) -> Result<Sha256> {
    if file.metadata()?.len() < progress.bytes {
        return Err("partial download is shorter than its durable receipt".into());
    }
    let mut hasher = Sha256::new();
    let mut left = progress.bytes;
    let mut buffer = [0; 64 * 1024];
    while left > 0 {
        let n = left.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n])?;
        hasher.update(&buffer[..n]);
        left -= n as u64;
    }
    if format!("{:x}", hasher.clone().finalize()) != progress.digest {
        return Err("partial download integrity mismatch; media preserved".into());
    }
    Ok(hasher)
}
fn commit(
    file: &mut File,
    receipt: &mut Receipt,
    journal: &Path,
    hasher: &mut Sha256,
    bytes: &[u8],
    progress: Option<Progress>,
) -> Result<()> {
    if receipt
        .progress
        .bytes
        .checked_add(bytes.len() as u64)
        .is_none_or(|n| n > receipt.identity.length)
    {
        return Err("audio payload exceeds declared representation length".into());
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    hasher.update(bytes);
    let mut next = progress.unwrap_or_else(|| receipt.progress.clone());
    next.bytes = receipt.progress.bytes + bytes.len() as u64;
    next.digest = format!("{:x}", hasher.clone().finalize());
    receipt.progress = next;
    storage::atomic_write(journal, &serde_json::to_vec(receipt)?, true)?;
    #[cfg(test)]
    metrics::committed(receipt.progress.bytes, &receipt.progress.digest);
    Ok(())
}

#[cfg(test)]
mod tests;

fn query_value<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    url.split_once('?')?.1.split('&').find_map(|part| {
        part.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
    })
}
fn replace_query(url: &str, key: &str, value: &str) -> Result<String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("invalid Web request parameter".into());
    }
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut pairs: Vec<_> = query
        .split('&')
        .filter(|p| !p.is_empty() && p.split_once('=').is_none_or(|(k, _)| k != key))
        .map(str::to_owned)
        .collect();
    pairs.push(format!("{key}={value}"));
    Ok(format!("{base}?{}", pairs.join("&")))
}
fn player_script_path(page: &str) -> Result<String> {
    for (offset, marker) in page.match_indices("\"jsUrl\":") {
        if let Some(Ok(Value::String(path))) =
            serde_json::Deserializer::from_str(&page[offset + marker.len()..])
                .into_iter::<Value>()
                .next()
            && valid_player_script(&path)
        {
            return Ok(path);
        }
    }
    Err("watch page has no supported player script".into())
}
fn valid_player_script(path: &str) -> bool {
    path.starts_with("/s/player/")
        && path.split('/').all(|s| s != "." && s != "..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
}
