//! Observe non-atomic ASR writes without repeatedly listing completed files.
use crate::Result;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct OutputWatch {
    directory: PathBuf,
    pending: BTreeSet<PathBuf>,
    rescan: bool,
    #[cfg(target_os = "linux")]
    events: Option<fs::File>,
    checked: std::time::Instant,
}
impl OutputWatch {
    pub fn new(directory: &Path) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Self::with_notifications(directory, || Self::notifications(directory))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::initial(directory)
        }
    }
    fn initial(directory: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("ASR output must be a real directory".into());
        }
        // A notification resource error must not hide an inaccessible directory.
        fs::read_dir(directory)?;
        Ok(Self {
            directory: directory.into(),
            pending: BTreeSet::new(),
            rescan: true,
            #[cfg(target_os = "linux")]
            events: None,
            checked: std::time::Instant::now(),
        })
    }
    #[cfg(target_os = "linux")]
    fn with_notifications(
        directory: &Path,
        setup: impl FnOnce() -> std::io::Result<fs::File>,
    ) -> Result<Self> {
        let mut watch = Self::initial(directory)?;
        match setup() {
            Ok(events) => watch.events = Some(events),
            Err(error)
                if error.raw_os_error().is_some_and(|code| {
                    matches!(
                        code,
                        libc::EMFILE
                            | libc::ENFILE
                            | libc::ENOSPC
                            | libc::ENOMEM
                            | libc::ENOSYS
                            | libc::EPERM
                            | libc::EACCES
                            | libc::ENODEV
                            | libc::EOPNOTSUPP
                    )
                }) =>
            {
                crate::protocol::event("ASR filesystem notifications unavailable; using polling");
            }
            Err(error) => return Err(error.into()),
        }
        Ok(watch)
    }
    #[cfg(target_os = "linux")]
    #[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
    fn notifications(directory: &Path) -> std::io::Result<fs::File> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        let name = std::ffi::CString::new(directory.as_os_str().as_bytes())
            .map_err(std::io::Error::other)?;
        // Own the descriptor immediately so watch setup failures close it.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let events = unsafe { fs::File::from_raw_fd(fd) };
        let mask = libc::IN_CREATE
            | libc::IN_MODIFY
            | libc::IN_CLOSE_WRITE
            | libc::IN_MOVED_TO
            | libc::IN_DELETE
            | libc::IN_ATTRIB
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        if unsafe { libc::inotify_add_watch(events.as_raw_fd(), name.as_ptr(), mask) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(events)
    }
    #[cfg(target_os = "linux")]
    fn observe(&mut self, buffer: &[u8]) -> Result<()> {
        use std::os::unix::ffi::OsStringExt;
        let count = buffer.len();
        let mut offset = 0;
        while offset < count {
            let header = buffer
                .get(offset..offset + 16)
                .filter(|_| offset + 16 <= count)
                .ok_or("truncated ASR output event")?;
            let mask = u32::from_ne_bytes(header[4..8].try_into()?);
            let length = u32::from_ne_bytes(header[12..16].try_into()?) as usize;
            let end = offset
                .checked_add(16)
                .and_then(|n| n.checked_add(length))
                .filter(|n| *n <= count)
                .ok_or("invalid ASR output event length")?;
            if mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                return Err("ASR output directory watch invalidated".into());
            }
            if mask & libc::IN_Q_OVERFLOW != 0 {
                self.rescan = true;
            }
            let name = &buffer[offset + 16..end];
            let length = name.iter().position(|b| *b == 0).unwrap_or(name.len());
            if length > 0 {
                self.pending.insert(
                    self.directory
                        .join(std::ffi::OsString::from_vec(name[..length].to_vec())),
                );
            }
            offset = end;
        }
        Ok(())
    }
    pub fn retry(&mut self, path: PathBuf) {
        self.pending.insert(path);
    }
    pub fn candidates(&mut self) -> Result<Vec<PathBuf>> {
        #[cfg(target_os = "linux")]
        if self.events.is_some() {
            use std::io::Read;
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = match self
                    .events
                    .as_mut()
                    .ok_or("missing ASR event reader")?
                    .read(&mut buffer)
                {
                    Ok(0) => return Err("ASR output watch closed".into()),
                    Ok(count) => count,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                };
                self.observe(&buffer[..count])?;
            }
        }
        #[cfg(target_os = "linux")]
        let polling = self.events.is_none();
        #[cfg(not(target_os = "linux"))]
        let polling = true;
        if polling && self.checked.elapsed() >= std::time::Duration::from_millis(200) {
            self.rescan = true;
            self.checked = std::time::Instant::now();
        }
        if self.rescan {
            for entry in fs::read_dir(&self.directory)? {
                self.pending.insert(entry?.path());
            }
            self.rescan = false;
        }
        let mut candidates = Vec::new();
        for path in std::mem::take(&mut self.pending) {
            match fs::symlink_metadata(&path) {
                Ok(_) => candidates.push(path),
                // Notifications describe past changes, including removed or renamed files.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(candidates)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn vanished_notification_candidates_are_not_asr_outputs() {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-watch-vanished-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let mut watch = OutputWatch::new(&root).unwrap();
        let temporary = root.join("pending");
        let output = root.join("000000.json");
        fs::write(&temporary, b"{}").unwrap();
        fs::rename(&temporary, &output).unwrap();
        watch.retry(root.join("deleted.json"));
        assert_eq!(watch.candidates().unwrap(), vec![output]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn observes_changes_and_retries_with_notification_or_polling_semantics() {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("000000.json");
        let mut watch = OutputWatch::new(&root).unwrap();
        let notifications = watch.events.is_some();
        let advance_poll = |watch: &mut OutputWatch| {
            if !notifications {
                // Polling intentionally reconciles only after its interval.
                watch.checked -= std::time::Duration::from_millis(200);
            }
        };
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
        watch.retry(path.clone());
        assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
        std::fs::write(&path, b"{}").unwrap();
        advance_poll(&mut watch);
        assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
        assert!(watch.candidates().unwrap().is_empty());
        std::fs::write(root.join("pending"), b"{}").unwrap();
        std::fs::rename(root.join("pending"), root.join("000001.json")).unwrap();
        advance_poll(&mut watch);
        assert!(
            watch
                .candidates()
                .unwrap()
                .contains(&root.join("000001.json"))
        );
        // Overflow loses names, so reconcile the directory exactly once.
        let mut overflow = [0u8; 16];
        overflow[4..8].copy_from_slice(&libc::IN_Q_OVERFLOW.to_ne_bytes());
        watch.observe(&overflow).unwrap();
        assert!(watch.candidates().unwrap().contains(&path));
        assert!(watch.candidates().unwrap().is_empty());
        for index in 2..4096 {
            std::fs::write(root.join(format!("{index:06}.json")), b"{}").unwrap();
        }
        advance_poll(&mut watch);
        // Notifications supply only new names; a polling scan sees all files.
        assert_eq!(
            watch.candidates().unwrap().len(),
            if notifications { 4094 } else { 4096 }
        );
        for _ in 0..100 {
            assert!(watch.candidates().unwrap().is_empty());
        }
        assert!(watch.observe(&[0; 3]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unavailable_notifications_poll_new_and_partial_results_and_reject_bad_directories() {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-fallback-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        for code in [libc::EMFILE, libc::ENOSPC, libc::ENOSYS, libc::EPERM] {
            let mut watch = OutputWatch::with_notifications(&root, || {
                Err(std::io::Error::from_raw_os_error(code))
            })
            .unwrap();
            assert!(watch.events.is_none());
            assert!(watch.candidates().unwrap().is_empty());
            let path = root.join("000000.json");
            std::fs::write(&path, b"{").unwrap();
            watch.checked -= std::time::Duration::from_millis(200);
            assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
            watch.retry(path.clone());
            assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
            std::fs::write(&path, b"{}").unwrap();
            watch.checked -= std::time::Duration::from_millis(200);
            assert_eq!(watch.candidates().unwrap(), vec![path.clone()]);
            assert_eq!(std::fs::read(&path).unwrap(), b"{}");
            std::fs::remove_file(path).unwrap();
        }
        assert!(
            OutputWatch::with_notifications(&root, || Err(std::io::Error::from_raw_os_error(
                libc::EIO
            )))
            .is_err()
        );
        let linked = root.with_extension("linked");
        std::os::unix::fs::symlink(&root, &linked).unwrap();
        assert!(
            OutputWatch::with_notifications(&linked, || Err(std::io::Error::from_raw_os_error(
                libc::ENOSPC
            )))
            .is_err()
        );
        std::fs::remove_file(linked).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            OutputWatch::with_notifications(&root, || Err(std::io::Error::from_raw_os_error(
                libc::ENOSPC
            )))
            .is_err()
        );
    }
    #[test]
    fn polling_progress_resets_short_asr_deadlines() {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-fallback-deadline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let output = root.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let mut watch = OutputWatch::with_notifications(&output, || {
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        })
        .unwrap();
        let runner = crate::process::Runner::new(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        ))
        .with_timeout(std::time::Duration::from_secs(1));
        let args = vec!["-c".into(), "sleep 0.65; printf '{}' > \"$1/000000.json\"; sleep 0.65; printf '{}' > \"$1/000001.json\"; sleep 0.1".into(), "fixture".into(), output.as_os_str().into()];
        let mut seen = BTreeSet::new();
        runner
            .run_with_progress(
                Path::new("/bin/sh"),
                &args,
                &root.join("stdout"),
                &root.join("stderr"),
                || {
                    let before = seen.len();
                    for path in watch.candidates()? {
                        if serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?).is_ok() {
                            seen.insert(path);
                        } else {
                            watch.retry(path);
                        }
                    }
                    Ok(seen.len() > before)
                },
            )
            .unwrap();
        assert!(!seen.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
