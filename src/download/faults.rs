//! Research-only access faults; no credentials or signed URLs in evidence.
use super::{Identity, Receipt, Source, WebClient};
use std::{cell::RefCell, time::Instant};

#[derive(Default)]
struct Fault {
    kind: String,
    after_bytes: u64,
    injected: bool,
    pending_403: bool,
    prefix_bytes: u64,
    prefix_sha256: String,
    identity: Option<Identity>,
    renewals: u64,
    token_refreshes: u64,
    changed_candidates: u64,
}
thread_local! {
    static FAULT: RefCell<Fault> = RefCell::new(Fault::default());
}
pub(super) fn reset() {
    FAULT.set(Fault::default());
}
pub(super) fn arm(kind: &str, after_bytes: u64) -> crate::Result<()> {
    if !matches!(
        kind,
        "403" | "source-expiry" | "token-expiry" | "track" | "representation"
    ) || after_bytes == 0
    {
        return Err("invalid research access fault or prefix threshold".into());
    }
    FAULT.set(Fault {
        kind: kind.into(),
        after_bytes,
        ..Fault::default()
    });
    Ok(())
}
pub(super) fn before_cycle(source: &mut Source, receipt: &Receipt) {
    FAULT.with_borrow_mut(|fault| {
        if fault.kind.is_empty()
            || fault.injected
            || source.direct
            || receipt.progress.bytes == 0
            || receipt.progress.bytes < fault.after_bytes
            || (fault.kind == "token-expiry"
                && !source
                    .web_client
                    .is_some_and(WebClient::requires_attestation))
        {
            return;
        }
        fault.injected = true;
        fault.prefix_bytes = receipt.progress.bytes;
        fault.prefix_sha256.clone_from(&receipt.progress.digest);
        fault.identity = Some(receipt.identity.clone());
        match fault.kind.as_str() {
            "403" => fault.pending_403 = true,
            "token-expiry" => source.token_expires = Some(Instant::now()),
            _ => source.expires_at = Some(0),
        }
    });
}
pub(super) fn media_failure() -> crate::Result<()> {
    if FAULT.with_borrow_mut(|fault| std::mem::take(&mut fault.pending_403)) {
        return Err(super::http::media_failure(
            "download_http_temporary",
            Some(403),
            "research-injected SABR HTTP 403",
        ));
    }
    Ok(())
}
pub(super) fn candidate(mut source: Source) -> Source {
    FAULT.with_borrow_mut(|fault| {
        if fault.injected {
            match fault.kind.as_str() {
                "track" => source.identity.audio_track = Some("research-incompatible-track".into()),
                "representation" => source.identity.last_modified.push_str("-research-changed"),
                _ => return,
            }
            fault.changed_candidates += 1;
        }
    });
    source
}
pub(super) fn renewed() {
    FAULT.with_borrow_mut(|fault| {
        if fault.injected {
            fault.renewals += 1;
        }
    });
}
pub(super) fn token_refreshed() {
    FAULT.with_borrow_mut(|fault| {
        if fault.injected {
            fault.token_refreshes += 1;
        }
    });
}
pub(super) fn snapshot() -> serde_json::Value {
    FAULT.with_borrow(|fault| serde_json::json!({
        "kind":fault.kind,"injected":fault.injected,"after_bytes":fault.after_bytes,
        "prefix_bytes":fault.prefix_bytes,"prefix_sha256":fault.prefix_sha256,"identity":fault.identity,
        "renewals":fault.renewals,"token_refreshes":fault.token_refreshes,"changed_candidates":fault.changed_candidates,
    }))
}
