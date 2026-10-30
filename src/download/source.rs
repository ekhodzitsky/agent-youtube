//! Validate player responses and select a compatible audio representation.
use super::*;

#[cfg(test)]
pub(super) fn parse_source(raw: Vec<u8>, id: &str, http: &Http) -> Result<Source> {
    parse_source_for(raw, id, http, None)
}
pub(super) fn parse_source_for(
    raw: Vec<u8>,
    id: &str,
    http: &Http,
    wanted: Option<&Identity>,
) -> Result<Source> {
    parse_source_inner(raw, id, http, wanted).map_err(|e| {
        if e.is::<crate::protocol::Failure>() {
            e
        } else {
            crate::protocol::failure("youtube_metadata_invalid", Some(false), e)
        }
    })
}
pub(super) fn parse_source_inner(
    raw: Vec<u8>,
    id: &str,
    http: &Http,
    wanted: Option<&Identity>,
) -> Result<Source> {
    let root: Value = serde_json::from_slice(&raw)?;
    if root["playabilityStatus"]["status"].as_str() != Some("OK") {
        let reason = root["playabilityStatus"]["reason"]
            .as_str()
            .unwrap_or("restricted or unplayable video");
        let normalized = reason.replace('’', "'").to_ascii_lowercase();
        let (code, retryable) = if (root["playabilityStatus"]["status"] == "LOGIN_REQUIRED"
            && normalized.contains("sign in to confirm you're not a bot"))
            || root
                .pointer("/playabilityStatus/errorScreen/playerCaptchaViewModel")
                .is_some_and(Value::is_object)
        {
            ("youtube_bot_check", true)
        } else if normalized.contains("this content isn't available, try again later") {
            ("youtube_rate_limited", true)
        } else {
            ("youtube_source_unavailable", false)
        };
        return Err(crate::protocol::failure(
            code,
            Some(retryable),
            format!("YouTube source unavailable: {reason}"),
        ));
    }
    let details = &root["videoDetails"];
    if details["videoId"].as_str() != Some(id) {
        return Err("YouTube returned an unexpected video id".into());
    }
    if details["isLive"].as_bool() == Some(true)
        || root
            .pointer("/microformat/playerMicroformatRenderer/liveBroadcastDetails/isLiveNow")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return Err(crate::protocol::failure(
            "youtube_live_unsupported",
            Some(false),
            "live sources are not supported; use a completed recording",
        ));
    }
    let formats = root["streamingData"]["adaptiveFormats"]
        .as_array()
        .ok_or("YouTube returned no adaptive audio formats")?;
    let mut candidates: Vec<_> = formats
        .iter()
        .filter(|f| {
            f["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("audio/mp4;") && m.contains("mp4a.40."))
                && f.get("width").is_none()
                && f.get("height").is_none()
                && f.pointer("/audioTrack/isAutoDubbed")
                    .and_then(Value::as_bool)
                    != Some(true)
                && f.get("drmFamilies").is_none()
        })
        .collect();
    // English player metadata distinguishes original from default. Default can
    // be a dub; without a unique original identity, labeled tracks are unsafe.
    let labeled = formats.iter().any(|f| f.get("audioTrack").is_some());
    let mut original_track = None;
    if labeled {
        candidates.retain(|f| {
            let name = f
                .pointer("/audioTrack/displayName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            name.split(|c: char| !c.is_ascii_alphabetic())
                .any(|word| word == "original")
                && !name.contains("descriptive")
                && !name.contains("dubbed")
        });
        for f in &candidates {
            let id = f
                .pointer("/audioTrack/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or("original audio track has no identity")?;
            if original_track.is_some_and(|previous| previous != id) {
                return Err("ambiguous original audio tracks; refusing to select a dub".into());
            }
            original_track = Some(id);
        }
    }
    if labeled && original_track.is_none() {
        return Err(crate::protocol::failure(
            "youtube_audio_unsupported",
            Some(false),
            "cannot establish the original audio track; refusing default or dubbed audio",
        ));
    }
    // Refresh the retained representation before considering quality. The full
    // identity is still checked by recovery before any media or receipt mutation.
    let retained = wanted.and_then(|wanted| {
        candidates.iter().copied().find(|f| {
            f["itag"].as_u64() == Some(wanted.itag)
                && f["lastModified"].as_str() == Some(wanted.last_modified.as_str())
                && f["xtags"].as_str().unwrap_or("") == wanted.xtags
                && f["contentLength"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    == Some(wanted.length)
                && f["mimeType"].as_str() == Some(wanted.mime.as_str())
                && original_track == wanted.audio_track.as_deref()
        })
    });
    let audio = retained
        .or_else(|| {
            candidates
                .into_iter()
                .max_by_key(|f| f["bitrate"].as_u64().unwrap_or(0))
        })
        .ok_or_else(|| {
            crate::protocol::failure(
                "youtube_audio_unsupported",
                Some(false),
                "YouTube returned no supported original audio-only AAC/M4A format",
            )
        })?;
    let length = audio["contentLength"]
        .as_str()
        .ok_or("audio format has no content length")?
        .parse::<u64>()?;
    if length == 0 {
        return Err("audio length must be nonzero".into());
    }
    let is_direct = audio["url"].as_str().is_some();
    let identity = Identity {
        video_id: id.into(),
        audio_track: original_track.map(str::to_owned),
        direct: is_direct,
        itag: audio["itag"].as_u64().ok_or("audio has no itag")?,
        last_modified: audio["lastModified"]
            .as_str()
            .ok_or("audio has no representation timestamp")?
            .into(),
        xtags: audio["xtags"].as_str().unwrap_or("").into(),
        length,
        mime: audio["mimeType"]
            .as_str()
            .ok_or("missing audio MIME")?
            .into(),
    };
    let direct = audio["url"].as_str();
    let is_direct = direct.is_some();
    let (url, config, video_format) = if let Some(url) = direct {
        (url.to_owned(), Vec::new(), Vec::new())
    } else {
        let url = root["streamingData"]["serverAbrStreamingUrl"]
            .as_str()
            .ok_or("YouTube requires an unsupported ciphered audio link")?
            .to_owned();
        let config = root.pointer("/playerConfig/mediaCommonConfig/mediaUstreamerRequestConfig/videoPlaybackUstreamerConfig").and_then(Value::as_str).ok_or("SABR has no playback configuration")?;
        let base64 = GeneralPurpose::new(
            &base64::alphabet::URL_SAFE,
            GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
        );
        let config = base64.decode(config)?;
        let video = formats
            .iter()
            .filter(|f| {
                f["mimeType"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("video/"))
            })
            .min_by_key(|f| f["bitrate"].as_u64().unwrap_or(u64::MAX))
            .ok_or("SABR has no format metadata for audio-only negotiation")?;
        (url, config, format_id(video)?)
    };
    http.validate_url(&url)?;
    let format = format_id(audio)?;
    let expires_at = lease::deadline(&root, &url)?;
    Ok(Source {
        duration_seconds: root["videoDetails"]["lengthSeconds"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| root["duration"].as_u64())
            .filter(|seconds| *seconds > 0),
        raw,
        identity,
        url,
        config,
        video_format,
        format,
        direct: is_direct,
        session: sabr::Session::default(),
        web_client: None,
        player_script: None,
        token_expires: None,
        expires_at,
        client: serde_json::json!({"clientName":5,"clientVersion":CLIENT_VERSION,"deviceMake":"Apple","deviceModel":"iPhone16,2","osName":"iPhone","osVersion":"18.3.2.22D82"}),
        user_agent: USER_AGENT.into(),
    })
}
pub(super) fn embedded_json(page: &str, marker: &str, key: &str) -> Result<Value> {
    for (offset, _) in page.match_indices(marker) {
        let tail = &page[offset + marker.len()..];
        let Some(start) = tail.find('{').filter(|pos| *pos < 32) else {
            continue;
        };
        if let Some(Ok(value)) = serde_json::Deserializer::from_str(&tail[start..])
            .into_iter::<Value>()
            .next()
            && value.get(key).is_some()
        {
            return Ok(value);
        }
    }
    Err("watch page has no supported player bootstrap data".into())
}
pub(super) fn format_id(f: &Value) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    wire::num(
        &mut data,
        1,
        f["itag"].as_u64().ok_or("format has no itag")?,
    );
    wire::num(
        &mut data,
        2,
        f["lastModified"]
            .as_str()
            .ok_or("format has no modification timestamp")?
            .parse()?,
    );
    if let Some(tags) = f["xtags"].as_str() {
        wire::blob(&mut data, 3, tags.as_bytes());
    }
    Ok(data)
}
