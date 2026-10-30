//! Temporary, receipt-bound source context; never contains generated attestation state.
use super::{
    METADATA_LIMIT, Receipt, Source, WebClient, http::Http, owned, parse_source_for, query_value,
    suffix,
};
use crate::{Result, storage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lease {
    version: u32,
    expires_at: u64,
    player: Value,
    client: Value,
    web_client: Option<WebClient>,
    player_script: Option<String>,
}

pub(super) fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub(super) fn deadline(root: &Value, url: &str) -> Result<Option<u64>> {
    let Some(expires) = query_value(url, "expire").and_then(|value| value.parse::<u64>().ok())
    else {
        return Ok(None);
    };
    let seconds = root["streamingData"]["expiresInSeconds"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok());
    Ok(Some(if let Some(seconds) = seconds {
        expires.min(now()?.saturating_add(seconds))
    } else {
        expires
    }))
}

pub(super) fn expired(source: &Source) -> Result<bool> {
    let threshold = now()?.saturating_add(60);
    Ok(source
        .expires_at
        .is_some_and(|expires| expires <= threshold))
}

fn path(prefix: &Path, digest: &str) -> Result<PathBuf> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid source lease digest; media preserved".into());
    }
    Ok(suffix(prefix, &format!(".lease-{digest}")))
}

fn fields(value: &Value, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .filter_map(|key| {
                value
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect(),
    )
}

pub(super) fn save(source: &Source, prefix: &Path) -> Result<Option<String>> {
    let Some(expires_at) = source.expires_at else {
        return Ok(None);
    };
    if expired(source)? {
        return Ok(None);
    }
    let root: Value = serde_json::from_slice(&source.raw)?;
    // Keep the selection/negotiation evidence, not visitor/tracking/ad data.
    let formats: Vec<Value> = root["streamingData"]["adaptiveFormats"]
        .as_array()
        .ok_or("missing lease formats")?
        .iter()
        .map(|format| {
            fields(
                format,
                &[
                    "mimeType",
                    "width",
                    "height",
                    "audioTrack",
                    "drmFamilies",
                    "bitrate",
                    "contentLength",
                    "itag",
                    "lastModified",
                    "xtags",
                    "url",
                ],
            )
        })
        .collect();
    let lease = Lease {
        version: 1,
        expires_at,
        player: serde_json::json!({
            "playabilityStatus": fields(&root["playabilityStatus"], &["status"]),
            "videoDetails": fields(&root["videoDetails"], &["videoId", "isLive"]),
            "duration": source.duration_seconds,
            "streamingData": {"adaptiveFormats":formats,"serverAbrStreamingUrl":root["streamingData"]["serverAbrStreamingUrl"]},
            "playerConfig":{"mediaCommonConfig":{"mediaUstreamerRequestConfig":{"videoPlaybackUstreamerConfig":root.pointer("/playerConfig/mediaCommonConfig/mediaUstreamerRequestConfig/videoPlaybackUstreamerConfig")}}},
        }),
        client: fields(
            &source.client,
            &[
                "clientName",
                "clientVersion",
                "deviceMake",
                "deviceModel",
                "osName",
                "osVersion",
            ],
        ),
        web_client: source.web_client,
        player_script: source.player_script.clone(),
    };
    let bytes = serde_json::to_vec(&lease)?;
    if bytes.len() > METADATA_LIMIT * 2 {
        return Err("source lease exceeds its byte limit".into());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let path = path(prefix, &digest)?;
    owned(&path)?;
    if path.exists() {
        if std::fs::metadata(&path)?.len() != bytes.len() as u64
            || storage::digest(&path)? != digest
        {
            return Err("source lease integrity mismatch; media preserved".into());
        }
    } else {
        storage::atomic_write_with(&path, false, |file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            use std::io::Write;
            Ok(file.write_all(&bytes)?)
        })?;
    }
    Ok(Some(digest))
}

pub(super) fn load(
    prefix: &Path,
    receipt: &Receipt,
    id: &str,
    http: &Http,
) -> Result<Option<Source>> {
    let Some(digest) = &receipt.lease else {
        return Ok(None);
    };
    let path = path(prefix, digest)?;
    owned(&path)?;
    let bytes = storage::read_text(&path, (METADATA_LIMIT * 2) as u64)?;
    if format!("{:x}", Sha256::digest(bytes.as_bytes())) != *digest {
        return Err("source lease integrity mismatch; media preserved".into());
    }
    let lease: Lease = serde_json::from_str(&bytes)?;
    if lease.version != 1 || lease.web_client != receipt.web_client {
        return Err("invalid source lease client/version; media preserved".into());
    }
    let mut source = parse_source_for(
        serde_json::to_vec(&lease.player)?,
        id,
        http,
        Some(&receipt.identity),
    )?;
    if source.identity != receipt.identity {
        return Err("source lease representation changed; media preserved".into());
    }
    if source
        .duration_seconds
        .zip(receipt.duration_seconds)
        .is_some_and(|(current, saved)| current != saved)
    {
        return Err("source lease duration mismatch; media preserved".into());
    }
    let bound = source
        .expires_at
        .ok_or("source lease has no signed expiry")?;
    if lease.expires_at > bound {
        return Err("source lease exceeds its signed expiry".into());
    }
    let client_matches = match lease.web_client {
        Some(WebClient::Web) => lease.client["clientName"] == "WEB",
        Some(WebClient::Mweb) => lease.client["clientName"] == "MWEB",
        Some(WebClient::Samsung) => lease.client["clientName"] == "TVHTML5" && source.direct,
        Some(WebClient::Visionos) => lease.client["clientName"] == "VISIONOS" && source.direct,
        Some(WebClient::Embedded) => {
            lease.client["clientName"] == "WEB_EMBEDDED_PLAYER" && source.direct
        }
        None => lease.client["clientName"] == 5,
    };
    if !client_matches
        || lease.client["clientVersion"]
            .as_str()
            .is_none_or(|version| version.is_empty() || version.len() > 128)
    {
        return Err("source lease has invalid client context".into());
    }
    if let Some(script) = &lease.player_script {
        if !super::valid_player_script(script) {
            return Err("source lease has an untrusted player script".into());
        }
    } else if query_value(&source.url, "n").is_some() {
        return Err("source lease is missing its player script".into());
    }
    source.expires_at = Some(lease.expires_at);
    source.web_client = lease.web_client;
    source.client = lease.client;
    source.player_script = lease.player_script;
    source.user_agent = match source.web_client {
        Some(WebClient::Web) => super::WEB_USER_AGENT,
        Some(WebClient::Mweb) => super::MWEB_USER_AGENT,
        Some(WebClient::Samsung) => super::samsung::USER_AGENT,
        Some(WebClient::Visionos) => super::samsung::VISIONOS_USER_AGENT,
        Some(WebClient::Embedded) => super::samsung::EMBEDDED_USER_AGENT,
        None => super::USER_AGENT,
    }
    .into();
    // Callers check expiry before any network use. A validated expired lease
    // can still establish the identity of an already complete local download.
    Ok(Some(source))
}
