//! Optional stage timers. Inactive unless the process has `SYOM_PROF` set.

use std::sync::OnceLock;
use std::time::Instant;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SYOM_PROF").is_some())
}

pub(crate) fn stamp() -> Option<Instant> {
    on().then(Instant::now)
}

pub(crate) fn ns(t: Option<Instant>) -> u64 {
    match t {
        Some(t) => u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX),
        None => 0,
    }
}
