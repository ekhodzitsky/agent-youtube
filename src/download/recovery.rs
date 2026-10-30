//! Bounded client selection without modifying retained media or receipts.
use super::{Receipt, Source, WebClient, http::check, lease};
use crate::{Result, protocol};
use std::sync::atomic::AtomicBool;

pub(super) fn client_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<protocol::Failure>()
        .is_some_and(|failure| {
            // Called only for Web preparation/token refresh. A denied runtime
            // request may leave a compatible client usable; cooldowns still win.
            failure.retry_after().is_none()
                && failure.retry_after_seconds().is_none()
                && (matches!(
                    failure.code,
                    "youtube_attestation_failed" | "youtube_player_challenge_failed"
                ) || (failure.code == "youtube_http_error"
                    && failure.http_status() == Some(403)))
        })
}

pub(super) fn source_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<protocol::Failure>()
        .is_some_and(|failure| {
            failure.retry_after().is_none()
                && (matches!(
                    failure.code,
                    "youtube_bot_check" | "youtube_audio_unsupported"
                ) || (failure.retryable == Some(true)
                    && matches!(failure.code, "youtube_temporary_failure" | "http_transport"))
                    || (failure.code == "youtube_http_error"
                        && (failure.http_status() == Some(403)
                            || (failure.retryable == Some(true)
                                && matches!(
                                    failure.http_status(),
                                    Some(408 | 500 | 502 | 503 | 504)
                                )))))
        })
}

pub(super) fn select(
    receipt: &Receipt,
    excluded: &[Option<WebClient>],
    mut request: impl FnMut(Option<WebClient>) -> Result<Source>,
    cancelled: &AtomicBool,
) -> Result<Source> {
    let mut attempted = Vec::new();
    let mut changed = false;
    for client in [
        receipt.web_client,
        Some(WebClient::Samsung),
        Some(WebClient::Visionos),
        Some(WebClient::Mweb),
        Some(WebClient::Web),
        Some(WebClient::Embedded),
    ] {
        check(cancelled)?;
        if excluded.contains(&client) || attempted.contains(&client) {
            continue;
        }
        attempted.push(client);
        let source = match request(client) {
            Ok(source) => source,
            Err(error) => {
                check(cancelled)?;
                if source_failure(error.as_ref()) {
                    continue;
                }
                // Cooldowns, content restrictions and unknown failures are not
                // grounds to request another client.
                return Err(error);
            }
        };
        #[cfg(test)]
        let source = super::faults::candidate(source);
        let mut identity = source.identity.clone();
        if source.direct && (!excluded.is_empty() || source.web_client != receipt.web_client) {
            // Transport may change only towards a range-verifiable source. The
            // caller must prove the entire prefix before publishing this change.
            identity.direct = receipt.identity.direct;
        }
        if identity != receipt.identity {
            if identity.video_id != receipt.identity.video_id
                || identity.audio_track != receipt.identity.audio_track
            {
                return Err(protocol::failure(
                    "download_audio_track_changed",
                    Some(false),
                    "refreshed source changed video or audio track; partial media preserved",
                ));
            }
            changed = true;
            continue;
        }
        // A SABR prefix cannot be verified with HTTP ranges. Keep that session
        // pinned until a separate transport-aware restart is qualified.
        if !source.direct && source.web_client != receipt.web_client {
            continue;
        }
        if lease::expired(&source)? {
            return Err(protocol::failure(
                "youtube_temporary_failure",
                Some(true),
                "refreshed source URL expires too soon; partial media preserved",
            ));
        }
        return Ok(source);
    }
    if changed {
        return Err(protocol::failure_details(
            "download_representation_changed",
            Some(false),
            "download representation changed across qualified clients; partial media preserved",
            serde_json::json!({"media_retained":true,"clients_attempted":attempted.len(),"clients_excluded":excluded.len()}),
        ));
    }
    Err(protocol::failure_details(
        "download_recovery_exhausted",
        Some(true),
        "no compatible qualified client source; media retained for resume",
        serde_json::json!({"media_retained":true,"clients_attempted":attempted.len(),"clients_excluded":excluded.len()}),
    ))
}
