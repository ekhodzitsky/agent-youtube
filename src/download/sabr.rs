//! Audio-only SABR negotiation and validated UMP segment assembly.
//! Wire field numbers follow LuanRT/googlevideo's MIT-licensed protocol schemas.
use super::{
    Downloader, Progress, Receipt, Source, commit, http,
    wire::{self, Value, bytes, fields, number},
};
use crate::Result;
use sha2::Sha256;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};

#[derive(Clone, Default)]
pub(super) struct Session {
    pub(super) po_token: Vec<u8>,
    cookie: Vec<u8>,
    contexts: BTreeMap<u64, Vec<u8>>,
    send: BTreeSet<u64>,
    initialized: bool,
    request: u64,
    end_segment: Option<u64>,
    end_ms: Option<u64>,
    cdn_fallback_attempted: bool,
}
fn advertised_alternate(url: &str) -> Option<String> {
    let uri: ureq::http::Uri = url.parse().ok()?;
    if uri.scheme_str() != Some("https") || uri.path() != "/videoplayback" || url.contains('#') {
        return None;
    }
    let host = uri.host()?;
    let authority = uri.authority()?.as_str();
    if authority != host && authority != format!("{host}:443") {
        return None;
    }
    let label = host.strip_suffix(".googlevideo.com")?;
    let prefix = if label.starts_with("rr") { "rr" } else { "r" };
    let (index, original) = label.strip_prefix(prefix)?.split_once("---")?;
    let numeric = |value: &str| {
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<u32>().is_ok_and(|n| n > 0)
    };
    let node = |value: &str| {
        value.strip_prefix("sn-").is_some_and(|rest| {
            !rest.is_empty()
                && rest.len() <= 48
                && !rest.starts_with('-')
                && !rest.ends_with('-')
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    };
    if label.len() > 63 || !numeric(index) || !node(original) {
        return None;
    }
    let query = uri.query()?;
    let mut controls = [None; 6];
    for item in query.split('&') {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        // Do not permit percent-encoded aliases of control names.
        if key.contains('%') {
            return None;
        }
        let slot = match key {
            "sabr" => 0,
            "fvip" => 1,
            "mn" => 2,
            "fallback_count" => 3,
            "sparams" => 4,
            "lsparams" => 5,
            _ => continue,
        };
        if controls[slot].replace(value).is_some() {
            return None;
        }
    }
    if controls[0] != Some("1")
        || !numeric(controls[1]?)
        || !matches!(controls[3], None | Some("0"))
    {
        return None;
    }
    for signed in controls[4..].iter().flatten() {
        let names = signed.replace("%2C", ",").replace("%2c", ",");
        if names.split(',').any(|name| {
            name == "fallback_count"
                || name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            return None;
        }
    }
    // Commas may be escaped; node labels themselves must be literal DNS labels.
    let nodes = controls[2]?.replace("%2C", ",").replace("%2c", ",");
    if nodes.split(',').any(|value| !node(value)) {
        return None;
    }
    let mut advertised = nodes.split(',');
    // fallback_count=1 means mn[1]; never skip an ambiguous primary or duplicate.
    if advertised.next() != Some(original) {
        return None;
    }
    let alternate = advertised.next()?;
    if alternate == original {
        return None;
    }
    let label = format!("{prefix}{}---{alternate}", controls[1]?);
    if label.len() > 63 {
        return None;
    }
    let port = if uri.port_u16().is_some() { ":443" } else { "" };
    let query = if controls[3].is_some() {
        query
            .split('&')
            .map(|item| {
                if item.starts_with("fallback_count=") {
                    "fallback_count=1"
                } else {
                    item
                }
            })
            .collect::<Vec<_>>()
            .join("&")
    } else {
        format!("{query}&fallback_count=1")
    };
    Some(format!(
        "https://{label}.googlevideo.com{port}/videoplayback?{query}"
    ))
}

fn dispatch_with_fallback(
    url: &mut String,
    session: &mut Session,
    body: &[u8],
    cancelled: &AtomicBool,
    mut send: impl FnMut(&str, &[u8]) -> Result<ureq::http::Response<ureq::Body>>,
) -> Result<ureq::http::Response<ureq::Body>> {
    http::check(cancelled)?;
    let separator = if url.contains('?') { '&' } else { '?' };
    let request_url = format!("{url}{separator}rn={}", session.request);
    session.request += 1;
    let result = send(&request_url, body);
    let dns = result
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<crate::protocol::Failure>())
        .is_some_and(|failure| {
            failure.code == "http_transport" && failure.details()["transport_phase"] == "dns"
        });
    if dns
        && !session.cdn_fallback_attempted
        && let Some(alternate) = advertised_alternate(url)
    {
        http::check(cancelled)?;
        // One extra dispatch per source session, shared by all its SABR cycles.
        // Keep the logical request number and body identical across endpoints.
        session.cdn_fallback_attempted = true;
        #[cfg(test)]
        super::metrics::cdn_attempt();
        let result = send(&format!("{alternate}&rn={}", session.request - 1), body);
        if result
            .as_ref()
            .is_ok_and(|response| response.status().as_u16() == 200)
        {
            *url = alternate;
            #[cfg(test)]
            super::metrics::cdn_selected();
        }
        return result;
    }
    result
}
struct Segment {
    offset: u64,
    length: u64,
    sequence: u64,
    init: bool,
    end_ms: u64,
    payload: Vec<u8>,
}
fn time_range(start: u64, duration: u64) -> Vec<u8> {
    let mut out = Vec::new();
    wire::num(&mut out, 1, start);
    wire::num(&mut out, 2, duration);
    wire::num(&mut out, 3, 1000);
    out
}
fn buffered(format: &[u8], duration: u64, start: u64, end: u64) -> Vec<u8> {
    let mut out = Vec::new();
    wire::blob(&mut out, 1, format);
    wire::num(&mut out, 2, 0);
    wire::num(&mut out, 3, duration);
    wire::num(&mut out, 4, start);
    wire::num(&mut out, 5, end);
    wire::blob(&mut out, 6, &time_range(0, duration));
    out
}
pub(super) fn request(source: &Source, progress: &Progress) -> Vec<u8> {
    let mut abr = Vec::new();
    for (key, value) in [
        (23, 5_000_000),
        (28, progress.end_ms),
        (34, 1),
        (40, 1),
        (44, 1),
    ] {
        wire::num(&mut abr, key, value);
    }
    if let Some(track) = &source.identity.audio_track {
        wire::blob(&mut abr, 69, track.as_bytes());
    }
    wire::put_var(&mut abr, 35 * 8 + 5);
    abr.extend_from_slice(&1_f32.to_le_bytes());
    let mut client = Vec::new();
    for (key, name) in [
        (12, "deviceMake"),
        (13, "deviceModel"),
        (17, "clientVersion"),
        (18, "osName"),
        (19, "osVersion"),
    ] {
        if let Some(value) = source.client[name].as_str() {
            wire::blob(&mut client, key, value.as_bytes());
        }
    }
    let name = source.client["clientName"]
        .as_u64()
        .or_else(|| match source.client["clientName"].as_str() {
            Some("WEB") => Some(1),
            Some("MWEB") => Some(2),
            Some("IOS") => Some(5),
            _ => None,
        })
        .unwrap_or(1);
    wire::num(&mut client, 16, name);
    let mut context = Vec::new();
    wire::blob(&mut context, 1, &client);
    if !source.session.po_token.is_empty() {
        wire::blob(&mut context, 2, &source.session.po_token);
    }
    if !source.session.cookie.is_empty() {
        wire::blob(&mut context, 3, &source.session.cookie);
    }
    for key in &source.session.send {
        if let Some(value) = source.session.contexts.get(key) {
            let mut item = Vec::new();
            wire::num(&mut item, 1, *key);
            wire::blob(&mut item, 2, value);
            wire::blob(&mut context, 5, &item);
        }
    }
    let mut out = Vec::new();
    wire::blob(&mut out, 1, &abr);
    // The protocol requires a video format reference. Mark it fully buffered and
    // enable only audio, so no video payload is requested. Reject any sent anyway.
    wire::blob(&mut out, 2, &source.video_format);
    if source.session.initialized || progress.init {
        wire::blob(&mut out, 2, &source.format);
    }
    if let Some(sequence) = progress.sequence {
        wire::blob(
            &mut out,
            3,
            &buffered(&source.format, progress.end_ms, 0, sequence),
        );
    }
    let max = i32::MAX as u64;
    wire::blob(&mut out, 3, &buffered(&source.video_format, max, max, max));
    wire::blob(&mut out, 5, &source.config);
    wire::blob(&mut out, 16, &source.format);
    wire::blob(&mut out, 17, &source.video_format);
    wire::blob(&mut out, 19, &context);
    out
}
fn same_format(data: &[u8], source: &Source) -> Result<bool> {
    let f = fields(data)?;
    Ok(number(&f, 1) == Some(source.identity.itag)
        && number(&f, 2).map(|n| n.to_string()).as_deref()
            == Some(source.identity.last_modified.as_str())
        && bytes(&f, 3).unwrap_or_default() == source.identity.xtags.as_bytes())
}
pub(super) fn download_cycle(
    downloader: &Downloader,
    source: &mut Source,
    file: &mut File,
    receipt: &mut Receipt,
    journal: &Path,
    hasher: &mut Sha256,
    cancelled: &AtomicBool,
) -> Result<()> {
    #[cfg(test)]
    if super::metrics::inject_stall(receipt) {
        http::check(cancelled)?;
        return Ok(());
    }
    #[cfg(test)]
    super::faults::media_failure()?;
    let body = request(source, &receipt.progress);
    let mut response = http::retry_transport(
        dispatch_with_fallback(
            &mut source.url,
            &mut source.session,
            &body,
            cancelled,
            |url, body| {
                downloader.http.request(
                    url,
                    Some(body),
                    &[
                        ("Content-Type", "application/x-protobuf"),
                        ("Accept", "application/vnd.yt-ump"),
                        ("User-Agent", source.user_agent.as_str()),
                    ],
                    cancelled,
                )
            },
        ),
        "SABR HTTP transport failed",
    )?;
    let status = response.status().as_u16();
    if status == 429 || (status >= 400 && response.headers().contains_key("retry-after")) {
        return Err(http::rate_limited(
            response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok()),
            format!("SABR returned HTTP {status}; media retained for resume"),
        ));
    }
    if matches!(status, 403 | 408 | 410 | 500..=599) {
        return Err(http::media_failure(
            "download_http_temporary",
            Some(status),
            format!("SABR returned HTTP {status}"),
        ));
    }
    if status != 200 {
        return Err(format!("SABR returned HTTP {status}").into());
    }
    let mut reader = response.body_mut().as_reader();
    let mut segments: BTreeMap<u8, Segment> = BTreeMap::new();
    let mut response_bytes = 0_usize;
    let mut backoff = 0;
    loop {
        http::check(cancelled)?;
        let part = wire::part(&mut reader).map_err(|error| {
            if error.downcast_ref::<std::io::Error>().is_some() {
                http::media_failure("download_incomplete", None, "interrupted UMP response")
            } else {
                error
            }
        })?;
        let Some((kind, data)) = part else {
            break;
        };
        response_bytes = response_bytes
            .checked_add(data.len())
            .ok_or("SABR response length overflow")?;
        if response_bytes > 64 * 1024 * 1024 {
            return Err("SABR response exceeds its byte limit".into());
        }
        match kind {
            42 => {
                let meta = fields(&data)?;
                if same_format(
                    bytes(&meta, 2).ok_or("SABR format metadata has no identity")?,
                    source,
                )? {
                    let mime =
                        std::str::from_utf8(bytes(&meta, 5).ok_or("SABR format has no MIME")?)?;
                    if !mime.starts_with("audio/mp4;") || !mime.contains("mp4a") {
                        return Err("SABR changed audio codec".into());
                    }
                    source.session.initialized = true;
                    source.session.end_ms = number(&meta, 3);
                    source.session.end_segment = number(&meta, 4);
                }
            }
            20 => {
                let header = fields(&data)?;
                if number(&header, 7).is_some_and(|n| n != 0) {
                    return Err("compressed SABR media is unsupported".into());
                }
                if let Some(video_id) = bytes(&header, 2)
                    && video_id != source.identity.video_id.as_bytes()
                {
                    return Err("SABR returned another video".into());
                }
                let selected = if let Some(format) = bytes(&header, 13) {
                    same_format(format, source)?
                } else {
                    number(&header, 3) == Some(source.identity.itag)
                        && number(&header, 4).map(|n| n.to_string()).as_deref()
                            == Some(source.identity.last_modified.as_str())
                        && bytes(&header, 5).unwrap_or_default() == source.identity.xtags.as_bytes()
                };
                if !selected {
                    return Err("SABR returned an unrequested media format".into());
                }
                let id = u8::try_from(number(&header, 1).ok_or("media header has no id")?)?;
                if segments.len() >= 16 || segments.contains_key(&id) {
                    return Err("duplicate or excessive active SABR media headers".into());
                }
                let length = number(&header, 14).ok_or("SABR segment has no declared length")?;
                if length == 0 || length > 8 * 1024 * 1024 {
                    return Err("SABR segment exceeds size bounds".into());
                }
                let range = bytes(&header, 15)
                    .map(fields)
                    .transpose()?
                    .unwrap_or_default();
                let scale = number(&range, 3).unwrap_or(1000);
                if scale == 0 {
                    return Err("invalid media timescale".into());
                }
                let start = number(&header, 11)
                    .or_else(|| {
                        number(&range, 1)
                            .and_then(|n| n.checked_mul(1000))
                            .map(|n| n / scale)
                    })
                    .unwrap_or(0);
                let duration = number(&header, 12)
                    .or_else(|| {
                        number(&range, 2)
                            .and_then(|n| n.checked_mul(1000))
                            .map(|n| n / scale)
                    })
                    .unwrap_or(0);
                segments.insert(
                    id,
                    Segment {
                        offset: number(&header, 6).ok_or("SABR segment has no byte offset")?,
                        length,
                        sequence: number(&header, 9).unwrap_or(0),
                        init: number(&header, 8) == Some(1),
                        end_ms: start.checked_add(duration).ok_or("media time overflow")?,
                        payload: Vec::new(),
                    },
                );
            }
            21 => {
                let (id, payload) = data.split_first().ok_or("empty UMP media part")?;
                let segment = segments
                    .get_mut(id)
                    .ok_or("UMP media has no active header")?;
                if segment.payload.len() as u64 + payload.len() as u64 > segment.length {
                    return Err("SABR segment exceeds its declared length".into());
                }
                segment.payload.extend_from_slice(payload);
            }
            22 => {
                if data.len() != 1 {
                    return Err("invalid UMP media end".into());
                }
                let segment = segments
                    .remove(&data[0])
                    .ok_or("UMP media end has no active header")?;
                if segment.payload.len() as u64 != segment.length {
                    return Err("truncated SABR segment".into());
                }
                let end = segment
                    .offset
                    .checked_add(segment.length)
                    .ok_or("segment range overflow")?;
                if end <= receipt.progress.bytes {
                    // A refreshed SABR session may replay its init or old segments.
                    // Compare bytes rather than appending duplicates or trusting sequence alone.
                    file.seek(SeekFrom::Start(segment.offset))?;
                    let mut old = vec![0; segment.payload.len()];
                    file.read_exact(&mut old)?;
                    file.seek(SeekFrom::Start(receipt.progress.bytes))?;
                    if old != segment.payload {
                        return Err("SABR replay differs from durable audio".into());
                    }
                    #[cfg(test)]
                    super::metrics::replay(segment.length);
                    continue;
                }
                if segment.offset != receipt.progress.bytes {
                    return Err("SABR media has a gap or overlapping byte range".into());
                }
                let mut progress = receipt.progress.clone();
                if segment.init {
                    if progress.init || segment.offset != 0 {
                        return Err("unexpected SABR initialization segment".into());
                    }
                    progress.init = true;
                } else {
                    if !progress.init
                        || progress
                            .sequence
                            .is_some_and(|n| n.checked_add(1) != Some(segment.sequence))
                    {
                        return Err("SABR media sequence has a gap".into());
                    }
                    progress.sequence = Some(segment.sequence);
                    progress.end_ms = segment.end_ms;
                }
                commit(
                    file,
                    receipt,
                    journal,
                    hasher,
                    &segment.payload,
                    Some(progress),
                )?;
            }
            35 => {
                let policy = fields(&data)?;
                backoff = number(&policy, 4).unwrap_or(0);
                if let Some(cookie) = bytes(&policy, 7) {
                    source.session.cookie = cookie.to_vec();
                }
            }
            43 => {
                let redirect = fields(&data)?;
                let url =
                    std::str::from_utf8(bytes(&redirect, 1).ok_or("SABR redirect has no URL")?)?;
                downloader.http.validate_url(url)?;
                source.url = url.into();
            }
            44 => return Err("SABR server reported a protocol error; media retained".into()),
            57 => {
                let context = fields(&data)?;
                let kind = number(&context, 1).ok_or("SABR context has no type")?;
                if source.session.contexts.len() >= 64 {
                    return Err("too many SABR contexts".into());
                }
                if let Some(value) = bytes(&context, 3) {
                    source.session.contexts.insert(kind, value.to_vec());
                }
                if number(&context, 4) == Some(1) {
                    source.session.send.insert(kind);
                }
            }
            58 => {
                let status = fields(&data)?;
                if number(&status, 1) == Some(3) && source.web_client.is_some() {
                    return Err(http::media_failure(
                        "download_refresh_required",
                        None,
                        "SABR requires refreshed attestation",
                    ));
                }
                if number(&status, 1).is_some_and(|n| n != 1 && n != 2) {
                    return Err("YouTube requires stream authorization; media retained".into());
                }
            }
            59 => {
                for (key, value) in fields(&data)? {
                    let numbers = match value {
                        Value::Number(n) => vec![n],
                        Value::Bytes(mut b) => {
                            let mut n = Vec::new();
                            while !b.is_empty() {
                                if n.len() >= 64 {
                                    return Err("too many SABR context policies".into());
                                }
                                n.push(wire::varint(&mut b)?);
                            }
                            n
                        }
                        _ => continue,
                    };
                    for n in numbers {
                        match key {
                            1 => {
                                source.session.send.insert(n);
                            }
                            2 => {
                                source.session.send.remove(&n);
                            }
                            3 => {
                                source.session.send.remove(&n);
                                source.session.contexts.remove(&n);
                            }
                            _ => {}
                        }
                    }
                }
                if source.session.send.len() > 64 {
                    return Err("too many SABR context policies".into());
                }
            }
            46 => {
                return Err(http::media_failure(
                    "download_refresh_required",
                    None,
                    "SABR requested a fresh player response",
                ));
            }
            _ => {}
        }
    }
    if !segments.is_empty() {
        return Err(http::media_failure(
            "download_incomplete",
            None,
            "UMP response ended before its media end markers",
        ));
    }
    // Timing/sequence metadata cannot authorize completion of a shorter file.
    if source
        .session
        .end_segment
        .zip(receipt.progress.sequence)
        .is_some_and(|(end, seen)| seen >= end)
        && receipt.progress.bytes < source.identity.length
    {
        return Err("SABR ended before the declared audio length".into());
    }
    if receipt.progress.bytes < source.identity.length {
        if backoff > 30_000 {
            let seconds = backoff.div_ceil(1000);
            if downloader.http.origin == "https://www.youtube.com" {
                super::gate::Gate::user()?.defer(seconds, cancelled)?;
            }
            return Err(http::rate_limited(
                Some(&seconds.to_string()),
                "SABR requested a long backoff; media retained for resume",
            ));
        }
        http::pause(backoff, cancelled)?;
    }
    Ok(())
}

#[cfg(test)]
mod cdn_tests {
    use super::*;
    const URL: &str = "https://rr1---sn-first.googlevideo.com/videoplayback?sabr=1&fvip=2&mn=sn-first%2Csn-second&sig=a%2Fb+Z&x=1&x=2";
    fn dns() -> Box<dyn std::error::Error + Send + Sync> {
        crate::protocol::failure_details(
            "http_transport",
            Some(true),
            "fixture",
            serde_json::json!({"transport_phase":"dns"}),
        )
    }
    fn response(status: u16) -> ureq::http::Response<ureq::Body> {
        ureq::http::Response::builder()
            .status(status)
            .body(ureq::Body::builder().data(Vec::new()))
            .unwrap()
    }
    #[test]
    fn advertised_cdn_preserves_raw_query_and_uses_fvip() {
        assert_eq!(
            advertised_alternate(URL).as_deref(),
            Some(
                "https://rr2---sn-second.googlevideo.com/videoplayback?sabr=1&fvip=2&mn=sn-first%2Csn-second&sig=a%2Fb+Z&x=1&x=2&fallback_count=1"
            )
        );
        let explicit = format!(
            "{}&fallback_count=0&sparams=id%2Cfvip&lsparams=mn",
            URL.replace(".com/", ".com:443/")
        );
        let alternate = advertised_alternate(&explicit).unwrap();
        assert!(alternate.starts_with("https://rr2---sn-second.googlevideo.com:443/"));
        assert!(alternate.ends_with("&fallback_count=1&sparams=id%2Cfvip&lsparams=mn"));
        assert!(advertised_alternate(&URL.replace("%2C", ",")).is_some());
        assert!(advertised_alternate(&URL.replace("%2C", "%2c")).is_some());
        assert!(
            advertised_alternate(&URL.replace("rr1---", "r1---"))
                .unwrap()
                .starts_with("https://r2---")
        );
    }
    #[test]
    fn advertised_cdn_rejects_ambiguous_or_untrusted_controls() {
        for url in [
            URL.replace("https:", "http:"),
            URL.replace("https://", "https://user@"),
            URL.replace(".com/", ".com:444/"),
            URL.replace(".com/", ".com:65536/"),
            URL.replace(".com/", ".com:invalid/"),
            URL.replace(".googlevideo.com", ".googlevideo.com.evil"),
            URL.replace("/videoplayback", "/other"),
            format!("{URL}#fragment"),
            URL.replace("sabr=1", "sabr=0"),
            URL.replace("fvip=2&", ""),
            URL.replace("fvip=2", "fvip=0"),
            URL.replace("fvip=2", "fvip=%32"),
            URL.replace("fvip=2", "fvip=2x"),
            URL.replace("fvip=2", "fvip=9999999999999999999"),
            URL.replace("mn=", "m%6e="),
            URL.replace("mn=sn-first%2Csn-second", "mn=sn-first"),
            URL.replace("mn=sn-first%2Csn-second", "mn=sn-second%2Csn-first"),
            URL.replace("mn=sn-first%2Csn-second", "mn=sn-second%2Csn-third"),
            URL.replace(
                "mn=sn-first%2Csn-second",
                "mn=sn-first%2Csn-first%2Csn-second",
            ),
            URL.replace("sn-second", "sn-%73econd"),
            URL.replace("sn-second", "sn-second.evil"),
            URL.replace("sn-second", "sn-second%2Fevil"),
            URL.replace("sn-second", "sn-second-"),
            URL.replace("sn-second", "sn-"),
            URL.replace("sn-second", &format!("sn-{}", "a".repeat(49))),
            format!("{URL}&fvip=3"),
            format!("{URL}&sabr=1"),
            format!("{URL}&mn=sn-third"),
            format!("{URL}&fallback_count=1"),
            format!("{URL}&fallback_count=0&fallback_count=0"),
            format!("{URL}&fallback%5fcount=0"),
            format!("{URL}&sparams=id,fallback_count"),
            format!("{URL}&lsparams=mn%2Cfallback_count"),
            format!("{URL}&sparams=fallback%5fcount"),
            format!("{URL}&sparams=id&sparams=mn"),
            format!("{URL}&lsparams=id&lsparams=mn"),
        ] {
            assert!(advertised_alternate(&url).is_none(), "accepted {url}");
        }
        let url = URL.replace(
            "mn=sn-first%2Csn-second",
            "mn=sn-first%2Csn-second%2Csn-third",
        );
        assert!(
            advertised_alternate(&url)
                .unwrap()
                .starts_with("https://rr2---sn-second.googlevideo.com/")
        );
    }
    #[test]
    fn dns_fallback_reuses_body_and_request_number_then_pins_success() {
        let mut url = URL.to_owned();
        let mut session = Session::default();
        let cancelled = AtomicBool::new(false);
        let mut calls = Vec::new();
        let result = dispatch_with_fallback(
            &mut url,
            &mut session,
            b"same-body",
            &cancelled,
            |url, body| {
                calls.push((url.to_owned(), body.to_vec()));
                if calls.len() == 1 {
                    Err(dns())
                } else {
                    Ok(response(200))
                }
            },
        );
        assert!(result.is_ok());
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, calls[1].1);
        assert!(calls.iter().all(|(url, _)| url.ends_with("&rn=0")));
        assert_eq!(session.request, 1);
        assert_eq!(url, advertised_alternate(URL).unwrap());
        dispatch_with_fallback(&mut url, &mut session, b"next", &cancelled, |url, _| {
            assert!(url.starts_with("https://rr2---sn-second.googlevideo.com/"));
            assert!(url.ends_with("&rn=1"));
            Ok(response(200))
        })
        .unwrap();
    }
    #[test]
    fn cdn_fallback_never_responds_to_http_status_or_unknown_transport() {
        let cancelled = AtomicBool::new(false);
        for status in [200, 302, 403, 429, 503] {
            let mut url = URL.to_owned();
            let mut session = Session::default();
            let mut calls = 0;
            let reply =
                dispatch_with_fallback(&mut url, &mut session, b"body", &cancelled, |_, _| {
                    calls += 1;
                    Ok(response(status))
                })
                .unwrap();
            assert_eq!(reply.status().as_u16(), status);
            assert_eq!(calls, 1);
            assert!(!session.cdn_fallback_attempted);
            assert_eq!(url, URL);
        }
        for error in [
            http::rate_limited(Some("120"), "fixture pause"),
            crate::protocol::failure("http_transport", Some(true), "unknown fixture failure"),
        ] {
            let mut error = Some(error);
            let mut calls = 0;
            let mut session = Session::default();
            assert!(
                dispatch_with_fallback(
                    &mut URL.to_owned(),
                    &mut session,
                    b"body",
                    &cancelled,
                    |_, _| {
                        calls += 1;
                        Err(error.take().unwrap())
                    }
                )
                .is_err()
            );
            assert_eq!(calls, 1);
            assert!(!session.cdn_fallback_attempted);
        }
    }
    #[test]
    fn cdn_fallback_failure_keeps_origin_and_spends_the_session_budget() {
        let cancelled = AtomicBool::new(false);
        for status in [403, 429, 503] {
            let mut url = URL.to_owned();
            let mut session = Session::default();
            let mut calls = 0;
            let reply =
                dispatch_with_fallback(&mut url, &mut session, b"body", &cancelled, |_, _| {
                    calls += 1;
                    if calls == 1 {
                        Err(dns())
                    } else {
                        let mut reply = response(status);
                        reply
                            .headers_mut()
                            .insert("retry-after", "120".parse().unwrap());
                        Ok(reply)
                    }
                })
                .unwrap();
            assert_eq!(reply.status().as_u16(), status);
            assert_eq!(reply.headers()["retry-after"], "120");
            assert_eq!(url, URL);
            assert!(session.cdn_fallback_attempted);
            // UMP43 replaces the URL but retains the same Session and budget.
            url = format!("{URL}&redirected=1");
            assert!(
                dispatch_with_fallback(&mut url, &mut session, b"body", &cancelled, |_, _| {
                    calls += 1;
                    Err(dns())
                })
                .is_err()
            );
            assert_eq!(calls, 3);
        }
        let mut session = Session::default();
        let mut calls = 0;
        let mut url = URL.to_owned();
        for _ in 0..4 {
            assert!(
                dispatch_with_fallback(&mut url, &mut session, b"body", &cancelled, |_, _| {
                    calls += 1;
                    Err(dns())
                })
                .is_err()
            );
        }
        assert_eq!(calls, 5);
        assert_eq!(url, URL);
    }
    #[test]
    fn cdn_fallback_checks_cancellation_before_extra_dispatch() {
        let cancelled = AtomicBool::new(false);
        let mut session = Session::default();
        let mut calls = 0;
        let result = dispatch_with_fallback(
            &mut URL.to_owned(),
            &mut session,
            b"body",
            &cancelled,
            |_, _| {
                calls += 1;
                cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(dns())
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert!(!session.cdn_fallback_attempted);
    }
}
