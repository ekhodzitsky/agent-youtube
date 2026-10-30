//! Qualified anonymous direct clients; bootstrap visitor state before player calls.
use super::{
    Downloader, Identity, Source, WebClient, embedded_json, http, parse_source_for,
    player_script_path,
};
use crate::Result;
use serde_json::json;
use std::sync::atomic::AtomicBool;

pub(super) const USER_AGENT: &str = "Mozilla/5.0 (SMART-TV; Linux; Tizen 2.4.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/2.4.0 TV Safari/538.1";
pub(super) const VISIONOS_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
pub(super) const EMBEDDED_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15,gzip(gfe)";

impl Downloader {
    pub(super) fn samsung_source(
        &self,
        id: &str,
        cancelled: &AtomicBool,
        wanted: Option<&Identity>,
    ) -> Result<Source> {
        self.anonymous_source(id, WebClient::Samsung, cancelled, wanted)
    }
    pub(super) fn anonymous_source(
        &self,
        id: &str,
        selected: WebClient,
        cancelled: &AtomicBool,
        wanted: Option<&Identity>,
    ) -> Result<Source> {
        let (page, mut cookies) =
            self.watch_page(id, &self.http.origin, super::WEB_USER_AGENT, cancelled)?;
        let config = embedded_json(&page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
        let mut visitor = config
            .pointer("/INNERTUBE_CONTEXT/client/visitorData")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 8192)
            .ok_or("anonymous bootstrap has no valid visitor data")?
            .to_owned();
        let timestamp = config["STS"]
            .as_u64()
            .ok_or("anonymous bootstrap has no signature timestamp")?;
        let (number, version, user_agent, mut client) = match selected {
            WebClient::Samsung => (
                "7",
                "5.20260707",
                USER_AGENT,
                json!({"clientName":"TVHTML5","deviceMake":"Samsung","deviceModel":"UKS9800","userAgent":USER_AGENT,"osName":"Tizen","osVersion":"2.4.0"}),
            ),
            WebClient::Visionos => (
                "101",
                "1.02",
                VISIONOS_USER_AGENT,
                json!({"clientName":"VISIONOS","deviceMake":"Apple","platform":"MOBILE","osName":"visionOS","osVersion":"26.5.23O471","deviceModel":"RealityDevice17,1"}),
            ),
            WebClient::Embedded => (
                "56",
                "2.20260708.00.00",
                EMBEDDED_USER_AGENT,
                json!({"clientName":"WEB_EMBEDDED_PLAYER","userAgent":EMBEDDED_USER_AGENT,"clientScreen":"EMBED"}),
            ),
            _ => return Err("expected a qualified anonymous direct client".into()),
        };
        client["clientVersion"] = version.into();
        client["hl"] = "en".into();
        client["timeZone"] = "UTC".into();
        client["utcOffsetMinutes"] = 0.into();
        let mut body = json!({"videoId":id,"context":{"client":client},"playbackContext":{"contentPlaybackContext":{"html5Preference":"HTML5_PREF_WANTS","signatureTimestamp":timestamp}},"contentCheckOk":true,"racyCheckOk":true});
        if selected == WebClient::Visionos {
            body["contentCheckOk"] = "true".into();
            body.as_object_mut()
                .ok_or("invalid player request")?
                .remove("racyCheckOk");
        }
        if selected == WebClient::Embedded {
            let mut response = self.http.request(
                &format!("{}/embed/{id}?html5=1", self.http.origin),
                None,
                &[
                    ("User-Agent", super::WEB_USER_AGENT),
                    ("Referer", "https://www.reddit.com/"),
                ],
                cancelled,
            )?;
            let status = response.status().as_u16();
            if status == 429 || (status >= 400 && response.headers().contains_key("retry-after")) {
                return Err(http::rate_limited(
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok()),
                    "YouTube embed requested a cooldown",
                ));
            }
            if status != 200 {
                return Err(crate::protocol::failure_details(
                    "youtube_http_error",
                    Some(matches!(status, 408 | 500..=599)),
                    "YouTube embed bootstrap unavailable",
                    json!({"http_status":status}),
                ));
            }
            let embed_cookies = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .filter_map(|value| value.split(';').next())
                .collect::<Vec<_>>()
                .join("; ");
            if !embed_cookies.is_empty() {
                cookies = embed_cookies;
            }
            let embedded_page =
                String::from_utf8(http::read(&mut response, super::METADATA_LIMIT, cancelled)?)?;
            let embedded = embedded_json(&embedded_page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
            visitor = embedded["VISITOR_DATA"]
                .as_str()
                .or_else(|| {
                    embedded
                        .pointer("/INNERTUBE_CONTEXT/client/visitorData")
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|value| !value.is_empty() && value.len() <= 8192)
                .ok_or("embed bootstrap has no valid visitor data")?
                .to_owned();
            let flags = embedded.pointer("/WEB_PLAYER_CONTEXT_CONFIGS/WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER/encryptedHostFlags").and_then(serde_json::Value::as_str).filter(|value| !value.is_empty() && value.len() <= 65536).ok_or("embed bootstrap has no valid host flags")?;
            body["context"]["thirdParty"] = json!({"embedUrl":"https://www.reddit.com/"});
            body["playbackContext"]["contentPlaybackContext"]["encryptedHostFlags"] = flags.into();
        }
        let body = serde_json::to_vec(&body)?;
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("User-Agent", user_agent),
            ("X-YouTube-Client-Name", number),
            ("X-YouTube-Client-Version", version),
            ("Origin", self.http.origin.as_str()),
            ("X-Goog-Visitor-Id", visitor.as_str()),
        ];
        if !cookies.is_empty() {
            headers.push(("Cookie", &cookies));
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
                "YouTube anonymous player requested a cooldown",
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
                format!("YouTube anonymous player returned HTTP {status}"),
                json!({"http_status":status}),
            ));
        }
        let mut source = parse_source_for(
            http::read(&mut response, super::METADATA_LIMIT, cancelled)?,
            id,
            &self.http,
            wanted,
        )?;
        if !source.direct {
            return Err(crate::protocol::failure(
                "youtube_audio_unsupported",
                Some(false),
                "anonymous client returned an unqualified streaming protocol",
            ));
        }
        source.client = client;
        source.user_agent = user_agent.into();
        source.web_client = Some(selected);
        if super::query_value(&source.url, "n").is_some() {
            source.player_script = Some(player_script_path(&page)?);
        }
        Ok(source)
    }
}
