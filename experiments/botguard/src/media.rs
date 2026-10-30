//! Optional bounded audio access check. Tokens and signed URLs never leave memory.
use crate::USER_AGENT;
use crate::{Result, read_limited};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

pub fn check(agent: &ureq::Agent, video: &str, token: &[u8]) -> Result<Value> {
    let mut page = agent
        .get("https://www.youtube.com/watch")
        .query("v", video)
        .query("hl", "en")
        .query("app", "desktop")
        .header("Accept-Encoding", "identity")
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|_| "media check: watch request failed")?;
    let page = String::from_utf8(read_limited(page.body_mut().as_reader())?)?;
    eprintln!(
        "media check: watch bytes={}, config_markers={}, context_present={}",
        page.len(),
        page.matches("ytcfg.set(").count(),
        page.contains("INNERTUBE_CONTEXT")
    );
    if page.is_empty() {
        return Err("media check: empty watch response".into());
    }
    let config = embedded(&page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
    let visitor = config
        .pointer("/INNERTUBE_CONTEXT/client/visitorData")
        .and_then(Value::as_str)
        .ok_or("media check: missing visitor context")?;
    let mut client = config
        .pointer("/INNERTUBE_CONTEXT/client")
        .cloned()
        .ok_or("media check: missing client")?;
    client["clientName"] = json!("MWEB");
    client["clientVersion"] = json!("2.20260708.05.00");
    client["userAgent"] = json!(USER_AGENT);
    let mut payload = json!({"videoId":video,"context":{"client":client},
        "contentCheckOk":true,"racyCheckOk":true,
        "playbackContext":{"contentPlaybackContext":{"html5Preference":"HTML5_PREF_WANTS"}}});
    let sts = if let Some(sts) = config.get("STS").and_then(Value::as_u64) {
        sts
    } else {
        let script = player_script(agent, &page)?;
        script
            .split_once("signatureTimestamp:")
            .and_then(|(_, s)| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("media check: missing player timestamp")?
    };
    payload["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"] = json!(sts);

    let mut response = agent
        .post("https://www.youtube.com/youtubei/v1/player")
        .query("prettyPrint", "false")
        .header("Accept-Encoding", "identity")
        .header("User-Agent", USER_AGENT)
        .header("Content-Type", "application/json")
        .header("Origin", "https://www.youtube.com")
        .header("X-Youtube-Client-Name", "2")
        .header("X-Youtube-Client-Version", "2.20260708.05.00")
        .header("X-Goog-Visitor-Id", visitor)
        .send(serde_json::to_vec(&payload)?)
        .map_err(|_| "media check: player HTTP request failed")?;
    let player: Value = serde_json::from_slice(&read_limited(response.body_mut().as_reader())?)?;
    if player
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        != Some("OK")
    {
        return Ok(
            json!({"client":"MWEB","acceptance":"player_unavailable", "playability":player.pointer("/playabilityStatus/status").and_then(Value::as_str).filter(|s| matches!(*s,"UNPLAYABLE"|"ERROR"|"LOGIN_REQUIRED"|"LIVE_STREAM_OFFLINE"))}),
        );
    }
    let audio = player
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .and_then(|formats| {
            formats.iter().find(|f| {
                f["itag"] == 140
                    && f["mimeType"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("audio/mp4;"))
                    && f.get("drmFamilies").is_none()
            })
        });
    let Some(audio) = audio else {
        return Ok(json!({"client":"MWEB","acceptance":"no_aac_format"}));
    };
    let Some(url) = audio["url"].as_str() else {
        return Ok(json!({"client":"MWEB","acceptance":"no_direct_audio_url",
            "sabr":player.pointer("/streamingData/serverAbrStreamingUrl").is_some()}));
    };
    validate_media_url(url)?;
    let mut url = url.to_owned();
    if let Some(n) = url.split(['?', '&']).find_map(|p| p.strip_prefix("n=")) {
        let solution = solve_n(&player_script(agent, &page)?, n)?;
        let (base, query) = url.split_once('?').ok_or("media check: missing query")?;
        let query = query
            .split('&')
            .filter(|p| !p.starts_with("n="))
            .collect::<Vec<_>>()
            .join("&");
        // Solver values are restricted to URL-safe characters before interpolation.
        url = format!("{base}?{query}&n={solution}");
    }
    let response = agent
        .get(&url)
        .query("pot", URL_SAFE_NO_PAD.encode(token))
        .header("Accept-Encoding", "identity")
        .header("User-Agent", USER_AGENT)
        .header("Range", "bytes=0-65535")
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Ok(json!({"client":"MWEB","acceptance":"http_rejected","status":status}));
        }
        Err(_) => return Err("media check: audio request failed".into()),
    };
    if response.status().as_u16() != 206
        || response
            .headers()
            .get("content-range")
            .and_then(|h| h.to_str().ok())
            .is_none_or(|h| !h.starts_with("bytes 0-65535/"))
    {
        return Err("media check: invalid partial audio response".into());
    }
    let bytes = read_limited(response.body_mut().as_reader())?;
    if bytes.len() != 65536 || bytes.get(4..8) != Some(b"ftyp") {
        return Err("media check: invalid audio container prefix".into());
    }
    Ok(json!({"client":"MWEB","acceptance":"audio_prefix_received","bytes":bytes.len(),"itag":140}))
}

fn embedded(page: &str, marker: &str, key: &str) -> Result<Value> {
    for (index, _) in page.match_indices(marker) {
        let data = &page[index + marker.len()..];
        if let Some(Ok(value)) = serde_json::Deserializer::from_str(data)
            .into_iter::<Value>()
            .next()
            && (key.is_empty() || value.get(key).is_some())
        {
            return Ok(value);
        }
    }
    Err("media check: missing page config".into())
}

fn validate_media_url(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| "invalid media URI")?;
    if uri.scheme_str() != Some("https")
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || uri.port_u16().is_some_and(|p| p != 443)
        || !uri.host().is_some_and(|h| h.ends_with(".googlevideo.com"))
    {
        return Err("media check: disallowed media origin".into());
    }
    Ok(())
}

#[test]
fn media_origins_are_restricted() {
    for url in [
        "http://r.googlevideo.com/a",
        "https://googlevideo.com.evil/a",
        "https://user@r.googlevideo.com/a",
        "https://r.googlevideo.com:81/a",
    ] {
        assert!(validate_media_url(url).is_err());
    }
    assert!(validate_media_url("https://r.googlevideo.com/videoplayback?x=1").is_ok());
}

fn player_script(agent: &ureq::Agent, page: &str) -> Result<String> {
    let path = embedded(page, "\"jsUrl\":", "")?;
    let path = path
        .as_str()
        .filter(|p| {
            p.starts_with("/s/player/")
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
        })
        .ok_or("media check: disallowed player script path")?;
    let mut response = agent
        .get(format!("https://www.youtube.com{path}"))
        .header("Accept-Encoding", "identity")
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|_| "media check: player script request failed")?;
    Ok(String::from_utf8(read_limited(
        response.body_mut().as_reader(),
    )?)?)
}

fn solve_n(player: &str, challenge: &str) -> Result<String> {
    let mut context = boa_engine::Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(10_000_000);
    context.runtime_limits_mut().set_recursion_limit(512);
    eprintln!("media check: preparing player with native parser");
    let prepared = crate::player::prepare(player)?;
    eprintln!("media check: evaluating prepared player");
    crate::eval(
        &mut context,
        &format!("globalThis._result={{}};{prepared}"),
        "prepared player",
    )?;
    eprintln!("media check: solving n challenge");
    let result = crate::eval(
        &mut context,
        &format!(
            "JSON.stringify(_result.n({}))",
            serde_json::to_string(challenge)?
        ),
        "player n solver",
    )?;
    let result: Value = serde_json::from_str(
        &result
            .as_string()
            .ok_or("media check: missing solver result")?
            .to_std_string_escaped(),
    )?;
    let solution = result
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 4096
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or("media check: n challenge not solved")?;
    Ok(solution.to_owned())
}
