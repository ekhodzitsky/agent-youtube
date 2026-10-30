//! Per-user request spacing and shared server cooldown, without session secrets.
use super::http::{check, pause, rate_limited};
use crate::{Result, storage};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

const SPACING_MS: u64 = 5_000;

pub(super) struct Gate(PathBuf);

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    next_source_ms: u64,
    cooldown_until_ms: u64,
}

struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

impl Gate {
    pub(super) fn user() -> Result<Self> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .filter(|p| Path::new(p).is_absolute())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|p| Path::new(p).is_absolute())
                    .map(|p| PathBuf::from(p).join(".cache"))
            })
            .ok_or("YouTube request coordination requires XDG_CACHE_HOME or HOME")?;
        fs::create_dir_all(&cache)?;
        Self::at(cache.join("agent-youtube"))
    }

    #[allow(unsafe_code)] // geteuid has no pointer arguments or preconditions.
    pub(super) fn at(dir: PathBuf) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let metadata = fs::symlink_metadata(&dir)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("YouTube coordination directory must be a real directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(
                    "YouTube coordination directory must be private and owned by this user".into(),
                );
            }
        }
        Ok(Self(dir))
    }

    fn locked(&self, cancelled: &AtomicBool) -> Result<Guard> {
        let path = self.0.join("lock");
        storage::reject_nonregular(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err("YouTube coordination lock must be a regular file".into());
        }
        loop {
            check(cancelled)?;
            match file.try_lock() {
                Ok(()) => return Ok(Guard(file)),
                Err(TryLockError::WouldBlock) => pause(50, cancelled)?,
                Err(TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    }

    fn read(&self) -> Result<State> {
        let path = self.0.join("state.json");
        storage::reject_nonregular(&path)?;
        match storage::read_text(&path, 1024) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(State::default())
            }
            Err(e) => Err(e),
        }
    }

    fn write(&self, state: &State) -> Result<()> {
        storage::atomic_write(
            &self.0.join("state.json"),
            &serde_json::to_vec(state)?,
            true,
        )
    }

    pub(super) fn before(&self, source: bool, cancelled: &AtomicBool) -> Result<()> {
        loop {
            let guard = self.locked(cancelled)?;
            let mut state = self.read()?;
            let now = now_ms()?;
            if state.cooldown_until_ms > now {
                let seconds = (state.cooldown_until_ms - now).div_ceil(1000);
                return Err(rate_limited(
                    Some(&seconds.to_string()),
                    "YouTube shared cooldown is active; retry later",
                ));
            }
            if !source {
                return Ok(());
            }
            if state.next_source_ms <= now {
                state.next_source_ms = now.saturating_add(SPACING_MS);
                self.write(&state)?;
                return Ok(());
            }
            // A clock adjustment must not create an unbounded local wait.
            if state.next_source_ms - now > SPACING_MS {
                return Err(
                    "system clock moved backwards during YouTube request coordination".into(),
                );
            }
            drop(guard);
            pause(50, cancelled)?;
        }
    }

    pub(super) fn defer(&self, seconds: u64, cancelled: &AtomicBool) -> Result<()> {
        let _guard = self.locked(cancelled)?;
        let mut state = self.read()?;
        state.cooldown_until_ms = state
            .cooldown_until_ms
            .max(now_ms()?.saturating_add(seconds.saturating_mul(1000)));
        self.write(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "agent-youtube-gate-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn gate(&self) -> Gate {
            Gate::at(self.0.clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn separate_instances_share_cooldown_and_never_shorten_it() {
        let fixture = Fixture::new();
        let first = fixture.gate();
        let second = fixture.gate();
        let cancelled = AtomicBool::new(false);
        first.defer(100, &cancelled).unwrap();
        let deadline = first.read().unwrap().cooldown_until_ms;
        second.defer(1, &cancelled).unwrap();
        assert_eq!(first.read().unwrap().cooldown_until_ms, deadline);
        for source in [true, false] {
            let error = second.before(source, &cancelled).unwrap_err();
            let typed = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(typed.code, "youtube_rate_limited");
            assert!(typed.retry_after_seconds().unwrap() >= 99);
        }
    }

    #[test]
    fn gate_process_fixture() {
        if let Some(path) = std::env::var_os("AGENT_YOUTUBE_TEST_GATE") {
            let gate = Gate::at(PathBuf::from(path)).unwrap();
            gate.before(true, &AtomicBool::new(false)).unwrap();
            fs::write(gate.0.join("claimed"), now_ms().unwrap().to_string()).unwrap();
            gate.defer(80, &AtomicBool::new(false)).unwrap();
        }
    }

    #[test]
    fn independent_process_observes_spacing_and_publishes_shared_cooldown() {
        let fixture = Fixture::new();
        let gate = fixture.gate();
        gate.before(true, &AtomicBool::new(false)).unwrap();
        let next = gate.read().unwrap().next_source_ms;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "download::gate::tests::gate_process_fixture"])
            .env("AGENT_YOUTUBE_TEST_GATE", &fixture.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("coordinated child did not finish");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert!(status.success());
        let claimed: u64 = fs::read_to_string(fixture.0.join("claimed"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(claimed >= next);
        assert!(gate.before(false, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn spacing_and_lock_waits_are_cancellable_but_media_is_not_spaced() {
        let fixture = Fixture::new();
        let first = fixture.gate();
        let second = fixture.gate();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        first.before(true, &cancelled).unwrap();
        second.before(false, &cancelled).unwrap();
        let signal = cancelled.clone();
        let thread = std::thread::spawn(move || second.before(true, &signal));
        std::thread::sleep(std::time::Duration::from_millis(100));
        cancelled.store(true, Ordering::SeqCst);
        assert!(
            thread
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("interrupted")
        );
        cancelled.store(false, Ordering::SeqCst);
        let guard = first.locked(&cancelled).unwrap();
        let second = fixture.gate();
        let signal = cancelled.clone();
        let thread = std::thread::spawn(move || second.before(false, &signal));
        std::thread::sleep(std::time::Duration::from_millis(100));
        cancelled.store(true, Ordering::SeqCst);
        assert!(thread.join().unwrap().is_err());
        drop(guard);
    }

    #[test]
    fn expired_cooldown_is_released_and_invalid_state_is_not_overwritten() {
        let fixture = Fixture::new();
        let gate = fixture.gate();
        gate.write(&State {
            next_source_ms: 0,
            cooldown_until_ms: now_ms().unwrap() - 1,
        })
        .unwrap();
        gate.before(true, &AtomicBool::new(false)).unwrap();
        assert!(gate.read().unwrap().next_source_ms > now_ms().unwrap());
        fs::write(fixture.0.join("state.json"), b"broken").unwrap();
        assert!(gate.before(false, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(fixture.0.join("state.json")).unwrap(), b"broken");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_directory_lock_and_state_without_changing_targets() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let gate = fixture.gate();
        let target = fixture.0.join("target");
        fs::write(&target, b"preserved").unwrap();
        for name in ["lock", "state.json"] {
            symlink(&target, fixture.0.join(name)).unwrap();
            assert!(gate.before(true, &AtomicBool::new(false)).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"preserved");
            fs::remove_file(fixture.0.join(name)).unwrap();
        }
        let link = fixture.0.join("linked-directory");
        symlink(&fixture.0, &link).unwrap();
        assert!(Gate::at(link).is_err());
    }
}
