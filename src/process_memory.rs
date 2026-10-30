//! Optional Linux process-group RSS observation; shared pages can be counted twice.
use crate::Result;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct Monitor {
    limit: Option<u64>,
    peak: u64,
    sampled: Option<Instant>,
}
impl Monitor {
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            peak: 0,
            sampled: None,
        }
    }
    pub fn check(&mut self, group: u32, program: &Path) -> Result<()> {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        if self
            .sampled
            .is_some_and(|at| at.elapsed() < Duration::from_millis(200))
        {
            return Ok(());
        }
        self.sampled = Some(Instant::now());
        let observed = group_rss(group).map_err(|e| {
            crate::protocol::failure_details(
                "resource_monitor_unavailable",
                None,
                e,
                serde_json::json!({"stage":"recognize","path":program,"preserved":true}),
            )
        })?;
        self.peak = self.peak.max(observed);
        if observed > limit {
            return Err(crate::protocol::failure_details(
                "resource_limit",
                Some(true),
                "ASR process group exceeded observed resident-memory budget; media preserved",
                serde_json::json!({"stage":"recognize","path":program,"observed_bytes":observed,
                    "budget_bytes":limit,"preserved":true,"measurement":"process_group_rss"}),
            ));
        }
        Ok(())
    }
    pub fn report(&self) {
        if let Some(limit) = self.limit {
            crate::protocol::progress(
                "resource",
                format!(
                    "ASR observed process-group peak RSS {} bytes; budget {limit} bytes",
                    self.peak
                ),
                serde_json::json!({"measurement":"process_group_rss","peak_bytes":self.peak,"budget_bytes":limit}),
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn vanished_or_inaccessible(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) || error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
fn group_rss(group: u32) -> Result<u64> {
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })?;
    let mut bytes = 0_u64;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let text = match std::fs::read(entry.path().join("stat")) {
            Ok(text) => text,
            Err(e) if vanished_or_inaccessible(&e) => continue,
            Err(e) => return Err(e.into()),
        };
        if let Some(pages) = rss_pages(&text, group) {
            bytes = bytes
                .checked_add(pages.checked_mul(page).ok_or("RSS size overflow")?)
                .ok_or("RSS sum overflow")?;
        }
    }
    Ok(bytes)
}
#[cfg(not(target_os = "linux"))]
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
fn group_rss(_: u32) -> Result<u64> {
    Err("ASR memory monitoring requires Linux".into())
}

#[cfg(any(target_os = "linux", test))]
fn rss_pages(stat: &[u8], group: u32) -> Option<u64> {
    // comm is arbitrary bytes; only the fields after its final ')' are ASCII.
    let tail = stat.get(stat.iter().rposition(|byte| *byte == b')')? + 1..)?;
    let mut fields = std::str::from_utf8(tail).ok()?.split_whitespace();
    if fields.nth(2)?.parse::<u32>().ok()? != group {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn disappearing_proc_entries_are_not_monitor_failures() {
        for code in [libc::ENOENT, libc::ESRCH, libc::EACCES] {
            assert!(vanished_or_inaccessible(
                &std::io::Error::from_raw_os_error(code)
            ));
        }
        assert!(!vanished_or_inaccessible(
            &std::io::Error::from_raw_os_error(libc::EIO)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_non_utf8_process_name_does_not_abort_rss_monitoring() {
        use std::{
            io::{BufRead, BufReader},
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let mut child = Command::new("/usr/bin/python3")
            .args(["-c", "import ctypes,time; ctypes.CDLL(None).prctl(15,ctypes.c_char_p(b'worker\\xff'),0,0,0); data=bytearray(16*1024*1024); print('ready',flush=True); time.sleep(30)"])
            .process_group(0).stdout(Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let result = group_rss(child.id());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(ready.trim(), "ready");
        assert!(result.unwrap() >= 16 * 1024 * 1024);
    }

    #[test]
    fn stat_comm_may_contain_spaces_and_parentheses() {
        let mut fields = vec!["0"; 22];
        fields[0] = "S";
        fields[2] = "42";
        fields[21] = "123";
        let stat = format!("1 (a ) strange name) {}", fields.join(" "));
        assert_eq!(rss_pages(stat.as_bytes(), 42), Some(123));
        assert_eq!(rss_pages(stat.as_bytes(), 1), None);
        assert_eq!(rss_pages(b"truncated", 42), None);
    }
}
