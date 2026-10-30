//! Test-hook counters only; no URLs, headers or tokens are retained.
use std::cell::{Cell, RefCell};
#[derive(Default)]
struct Stall {
    after_bytes: Option<u64>,
    persistent: bool,
    cycles: u64,
    renewals: u64,
    prefix_bytes: u64,
    prefix_sha256: String,
    identity: Option<super::Identity>,
}
const CATEGORIES: [&str; 6] = ["watch", "player", "script", "attestation", "media", "other"];
thread_local! {
    static STALL: RefCell<Stall> = RefCell::new(Stall::default());
    static REQUESTS: Cell<u64> = const { Cell::new(0) };
    static REPLAYED: Cell<u64> = const { Cell::new(0) };
    static ENDPOINTS: Cell<[u64; 6]> = const { Cell::new([0; 6]) };
    static DNS_ARMED: Cell<bool> = const { Cell::new(false) };
    static DNS_INJECTED: Cell<u64> = const { Cell::new(0) };
    static COMMITTED: Cell<u64> = const { Cell::new(0) };
    static DNS_AFTER_BYTES: Cell<u64> = const { Cell::new(0) };
    static DNS_INJECTED_AFTER: Cell<u64> = const { Cell::new(0) };
    static COMMITTED_DIGEST: RefCell<String> = const { RefCell::new(String::new()) };
    static DNS_PREFIX_DIGEST: RefCell<String> = const { RefCell::new(String::new()) };
    static CDN_ATTEMPTS: Cell<u64> = const { Cell::new(0) };
    static CDN_SELECTED: Cell<u64> = const { Cell::new(0) };
}
pub(super) fn reset() {
    super::faults::reset();
    STALL.set(Stall::default());
    REQUESTS.set(0);
    REPLAYED.set(0);
    ENDPOINTS.set([0; 6]);
    DNS_ARMED.set(false);
    DNS_INJECTED.set(0);
    COMMITTED.set(0);
    DNS_AFTER_BYTES.set(0);
    DNS_INJECTED_AFTER.set(0);
    COMMITTED_DIGEST.with_borrow_mut(String::clear);
    DNS_PREFIX_DIGEST.with_borrow_mut(String::clear);
    CDN_ATTEMPTS.set(0);
    CDN_SELECTED.set(0);
}
pub(super) fn arm_stall(after_bytes: u64, persistent: bool) {
    STALL.set(Stall {
        after_bytes: Some(after_bytes),
        persistent,
        ..Stall::default()
    });
}
pub(super) fn inject_stall(receipt: &super::Receipt) -> bool {
    STALL.with_borrow_mut(|stall| {
        if stall
            .after_bytes
            .is_none_or(|limit| receipt.progress.bytes < limit)
            || receipt.progress.bytes == 0
            || (stall.renewals > 0 && !stall.persistent)
        {
            return false;
        }
        if stall.cycles == 0 {
            stall.prefix_bytes = receipt.progress.bytes;
            stall.prefix_sha256.clone_from(&receipt.progress.digest);
            stall.identity = Some(receipt.identity.clone());
        }
        stall.cycles += 1;
        true
    })
}
pub(super) fn stalled_source_renewed() {
    STALL.with_borrow_mut(|stall| stall.renewals += 1);
}
pub(super) fn cdn_attempt() {
    CDN_ATTEMPTS.set(CDN_ATTEMPTS.get() + 1);
}
pub(super) fn cdn_selected() {
    CDN_SELECTED.set(CDN_SELECTED.get() + 1);
}
pub(super) fn committed(bytes: u64, digest: &str) {
    COMMITTED.set(bytes);
    COMMITTED_DIGEST.with_borrow_mut(|value| digest.clone_into(value));
}
pub(super) fn arm_dns_failure(after_bytes: u64) {
    DNS_ARMED.set(true);
    DNS_AFTER_BYTES.set(after_bytes);
}
pub(super) fn inject_dns_failure(url: &str) -> bool {
    let eligible = url.parse::<ureq::http::Uri>().is_ok_and(|uri| {
        uri.scheme_str() == Some("https")
            && uri
                .host()
                .is_some_and(|host| host.ends_with(".googlevideo.com"))
            && uri.path() == "/videoplayback"
            && super::query_value(url, "sabr") == Some("1")
    });
    if eligible && COMMITTED.get() >= DNS_AFTER_BYTES.get() && DNS_ARMED.replace(false) {
        DNS_INJECTED.set(DNS_INJECTED.get() + 1);
        DNS_INJECTED_AFTER.set(COMMITTED.get());
        DNS_PREFIX_DIGEST.set(COMMITTED_DIGEST.with_borrow(Clone::clone));
        true
    } else {
        false
    }
}
pub(super) fn request(url: &str, attestation: bool) {
    REQUESTS.set(REQUESTS.get().saturating_add(1));
    let category = if attestation {
        3
    } else if let Ok(uri) = url.parse::<ureq::http::Uri>() {
        match uri.host() {
            Some(host) if host.ends_with(".googlevideo.com") => 4,
            Some("www.youtube.com" | "m.youtube.com" | "127.0.0.1") => match uri.path() {
                "/" | "/watch" => 0,
                path if path.starts_with("/embed/") => 0,
                "/youtubei/v1/player" => 1,
                path if path.starts_with("/s/player/") => 2,
                _ => 5,
            },
            _ => 5,
        }
    } else {
        5
    };
    let mut counts = ENDPOINTS.get();
    counts[category] = counts[category].saturating_add(1);
    ENDPOINTS.set(counts);
}
pub(super) fn replay(bytes: u64) {
    REPLAYED.set(REPLAYED.get().saturating_add(bytes));
}
pub(super) fn snapshot() -> serde_json::Value {
    let counts = ENDPOINTS.get();
    let categories: serde_json::Map<_, _> = CATEGORIES
        .iter()
        .zip(counts)
        .map(|(name, count)| ((*name).to_owned(), count.into()))
        .collect();
    serde_json::json!({"downloader_http_requests":REQUESTS.get(),"verified_replayed_audio_bytes":REPLAYED.get(),
        "requests_by_endpoint":categories,
        "access_fault":super::faults::snapshot(),
        "stall": STALL.with_borrow(|stall| serde_json::json!({
            "after_bytes":stall.after_bytes,"persistent":stall.persistent,
            "cycles":stall.cycles,"renewals":stall.renewals,
            "prefix_bytes":stall.prefix_bytes,"prefix_sha256":stall.prefix_sha256,
            "identity":stall.identity,
        })),
        "injected_dns_failures":DNS_INJECTED.get(),
        "dns_injected_after_committed_bytes":DNS_INJECTED_AFTER.get(),
        "dns_retained_prefix_sha256":DNS_PREFIX_DIGEST.with_borrow(Clone::clone),
        "cdn_fallback_attempts":CDN_ATTEMPTS.get(),"cdn_fallback_selections":CDN_SELECTED.get(),
        "media_wire_bytes":null,"coverage":"downloader request attempts include injected DNS failures and exclude interpreter-worker HTTP; replay counts verified retained audio only, not failed partial bodies or TCP retransmissions"})
}

#[test]
fn injected_dns_failure_is_one_shot_sabr_only_and_resettable() {
    reset();
    arm_dns_failure(0);
    assert!(!inject_dns_failure(
        "https://www.youtube.com/watch?v=private"
    ));
    assert!(!inject_dns_failure(
        "https://r1.googlevideo.com/videoplayback?sig=private"
    ));
    let url = "https://rr1---sn-a.googlevideo.com/videoplayback?sabr=1&sig=private";
    assert!(inject_dns_failure(url));
    assert!(!inject_dns_failure(url));
    assert_eq!(snapshot()["injected_dns_failures"], 1);
    assert!(!snapshot().to_string().contains("private"));
    reset();
    assert!(!inject_dns_failure(url));
    assert_eq!(snapshot()["injected_dns_failures"], 0);
    arm_dns_failure(1024);
    committed(1023, "earlier");
    assert!(!inject_dns_failure(url));
    committed(1024, "fixture");
    assert!(inject_dns_failure(url));
    assert_eq!(snapshot()["dns_injected_after_committed_bytes"], 1024);
    assert_eq!(snapshot()["dns_retained_prefix_sha256"], "fixture");
    reset();
}

#[test]
fn request_categories_preserve_total_and_reset() {
    reset();
    request("https://www.youtube.com/unknown?secret=private", false);
    let row = snapshot();
    assert_eq!(row["downloader_http_requests"], 1);
    assert_eq!(row["requests_by_endpoint"]["other"], 1);
    reset();
    assert_eq!(snapshot()["requests_by_endpoint"]["other"], 0);
}

#[test]
fn request_categories_are_fixed_private_and_thread_local() {
    reset();
    for (url, attestation) in [
        ("https://www.youtube.com/watch?v=private-video", false),
        ("https://m.youtube.com/watch?v=private-video", false),
        ("https://www.youtube.com/embed/private-video", false),
        ("https://www.youtube.com/", false),
        (
            "https://www.youtube.com/youtubei/v1/player?key=private-key",
            false,
        ),
        (
            "https://www.youtube.com/s/player/private-player/base.js",
            false,
        ),
        (
            "https://www.youtube.com/api/jnn/v1/GenerateIT?token=private-token",
            true,
        ),
        ("https://www.google.com/js/bg/private-script", true),
        (
            "https://r1.googlevideo.com/videoplayback?sig=private-signature",
            false,
        ),
        ("https://www.youtube.com/unknown?secret=private", false),
        ("invalid private url", false),
    ] {
        request(url, attestation);
    }
    let row = snapshot();
    assert_eq!(
        row["requests_by_endpoint"],
        serde_json::json!({"watch":4,"player":1,"script":1,"attestation":2,"media":1,"other":2})
    );
    assert_eq!(row["downloader_http_requests"], 11);
    assert!(!row.to_string().contains("private"));
    let child = std::thread::spawn(|| {
        assert_eq!(snapshot()["downloader_http_requests"], 0);
        request("https://www.youtube.com/watch", false);
        snapshot()
    })
    .join()
    .unwrap();
    assert_eq!(child["downloader_http_requests"], 1);
    assert_eq!(snapshot(), row);
    reset();
}
