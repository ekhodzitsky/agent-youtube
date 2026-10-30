//! Isolated-process memory report schema and live-heap tracker (TASK-12).
//!
//! The Criterion `mem` bench still runs every peer in one process (historical).
//! `benches/mem_iso.rs` uses this schema with one OS process per peer/case.

use std::fmt::Write as _;

/// Running live-heap accounting (allocation minus free).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeapTracker {
    pub allocs: u64,
    pub alloc_bytes: u64,
    pub live: u64,
    pub peak_live: u64,
}

impl HeapTracker {
    pub fn alloc(&mut self, size: u64) {
        self.allocs = self.allocs.saturating_add(1);
        self.alloc_bytes = self.alloc_bytes.saturating_add(size);
        self.live = self.live.saturating_add(size);
        if self.live > self.peak_live {
            self.peak_live = self.live;
        }
    }

    pub fn free(&mut self, size: u64) {
        self.live = self.live.saturating_sub(size);
    }
}

/// One peer/case/phase measurement from a fresh process.
#[derive(Clone, Debug, PartialEq)]
pub struct MemReport {
    pub peer: String,
    pub case: String,
    pub phase: String,
    pub baseline_rss: u64,
    pub peak_rss: u64,
    pub peak_rss_delta: u64,
    pub peak_live: u64,
    pub allocs: u64,
    pub alloc_bytes: u64,
    pub retained: u64,
}

impl MemReport {
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"peer\":\"{}\",\"case\":\"{}\",\"phase\":\"{}\",\
             \"baseline_rss\":{},\"peak_rss\":{},\"peak_rss_delta\":{},\
             \"peak_live\":{},\"allocs\":{},\"alloc_bytes\":{},\"retained\":{}}}",
            self.peer,
            self.case,
            self.phase,
            self.baseline_rss,
            self.peak_rss,
            self.peak_rss_delta,
            self.peak_live,
            self.allocs,
            self.alloc_bytes,
            self.retained
        );
        s
    }
}
