//! Best-effort resident allocation planning; no recording-duration ceiling.
use crate::Result;
use std::path::Path;

#[cfg(test)]
thread_local! { static TEST_AVAILABLE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_capacity_for_test<T>(bytes: u64, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_AVAILABLE.set(self.0);
        }
    }
    let _restore = Restore(TEST_AVAILABLE.replace(Some(bytes)));
    run()
}
pub(crate) fn available() -> Option<u64> {
    #[cfg(test)]
    if let Some(bytes) = TEST_AVAILABLE.get() {
        return Some(bytes);
    }
    #[cfg(target_os = "linux")]
    {
        let host = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("MemAvailable:")
                        .and_then(|s| s.split_whitespace().next())?
                        .parse::<u64>()
                        .ok()?
                        .checked_mul(1024)
                })
            });
        let membership = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
            });
        let cgroup = cgroup_headroom(
            Path::new("/sys/fs/cgroup"),
            membership.as_deref().unwrap_or("/"),
        );
        let address = address_headroom();
        [host, cgroup, address].into_iter().flatten().min()
    }
    #[cfg(not(target_os = "linux"))]
    None
}
#[cfg(target_os = "linux")]
fn cgroup_headroom(root: &Path, membership: &str) -> Option<u64> {
    let relative = Path::new(membership.trim_start_matches('/'));
    let safe = relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    let mut path = if safe {
        root.join(relative)
    } else {
        root.to_owned()
    };
    let mut available = None;
    loop {
        let limit = std::fs::read_to_string(path.join("memory.max")).ok();
        let used = std::fs::read_to_string(path.join("memory.current")).ok();
        if let (Some(limit), Some(used)) = (limit, used)
            && let Some(bytes) = headroom(&limit, &used)
        {
            available = Some(available.map_or(bytes, |old: u64| old.min(bytes)));
        }
        if path == root || !path.pop() {
            break;
        }
    }
    available
}
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
fn address_headroom() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // Only read initialized fields after a successful query.
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) } != 0
        || limit.rlim_cur == libc::RLIM_INFINITY
    {
        return None;
    }
    let pages = std::fs::read_to_string("/proc/self/statm")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    Some(limit.rlim_cur.saturating_sub(pages.checked_mul(page)?))
}
#[cfg(any(target_os = "linux", test))]
fn headroom(limit: &str, used: &str) -> Option<u64> {
    Some(
        limit
            .trim()
            .parse::<u64>()
            .ok()?
            .saturating_sub(used.trim().parse().ok()?),
    )
}
pub(crate) fn budget() -> Option<u64> {
    available().map(|bytes| bytes / 4)
}
pub(crate) fn require(bytes: u64, path: &Path, stage: &'static str) -> Result<()> {
    check(bytes, available(), path, stage)
}
fn check(bytes: u64, available: Option<u64>, path: &Path, stage: &'static str) -> Result<()> {
    if let Some(available) = available
        && bytes > available / 4
    {
        return Err(crate::protocol::failure_details(
            "resource_limit",
            Some(true),
            "planned resident allocation exceeds current memory budget; media preserved",
            serde_json::json!({"path":path.to_string_lossy(),"stage":stage,"required_bytes":bytes,"available_bytes":available,"budget_bytes":available/4,"preserved":true}),
        ));
    }
    Ok(())
}
pub(crate) fn allocation_error(
    path: &Path,
    stage: &'static str,
    error: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        "resource_limit",
        Some(true),
        error,
        serde_json::json!({"path":path.to_string_lossy(),"stage":stage,"preserved":true}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_plans_fail_before_allocation_without_a_duration_cap() {
        let path = Path::new("source.audio");
        assert!(check(1024, Some(4096), path, "decode_audio").is_ok());
        let error = check(1025, Some(4096), path, "decode_audio").unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "resource_limit");
        assert_eq!(failure.retryable, Some(true));
        assert!(check(1 << 40, Some(1 << 44), path, "decode_audio").is_ok());
        assert_eq!(headroom("4096", "4097"), Some(0));
        assert_eq!(headroom("max", "1000"), None);
        assert_eq!(headroom("8192", "2048"), Some(6144));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn nested_cgroup_uses_the_most_restrictive_ancestor() {
        let root =
            std::env::temp_dir().join(format!("agent-youtube-memory-{}", std::process::id()));
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        for (path, limit, used) in [
            ("", "max", "10"),
            ("a", "4096", "2048"),
            ("a/b", "8192", "512"),
        ] {
            std::fs::write(root.join(path).join("memory.max"), limit).unwrap();
            std::fs::write(root.join(path).join("memory.current"), used).unwrap();
        }
        assert_eq!(cgroup_headroom(&root, "/a/b"), Some(2048));
        std::fs::write(root.join("a/b/memory.max"), "1024").unwrap();
        assert_eq!(cgroup_headroom(&root, "/a/b"), Some(512));
        assert_eq!(cgroup_headroom(&root, "/../escape"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
