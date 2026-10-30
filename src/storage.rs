use crate::Result;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
thread_local! { pub(crate) static HASH_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
thread_local! { static TEST_SPACE: std::cell::Cell<Option<DiskSpace>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_space<T>(space: DiskSpace, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<DiskSpace>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_SPACE.set(self.0);
        }
    }
    let _restore = Restore(TEST_SPACE.replace(Some(space)));
    run()
}

/// Create new owned artifacts without granting group/other access via umask.
pub(crate) fn private_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Read-only inputs may be links, but must resolve to regular files, never pipes/devices.
pub(crate) fn open_regular_input(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !fs::metadata(path)?.is_file() {
        return Err("input must be a regular file".into());
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err("input must be a regular file".into());
    }
    Ok(file)
}

/// Only designated work directories are hardened; existing file contents are untouched.
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
pub(crate) fn private_directory(path: &Path, recursive: bool) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        if let Ok(metadata) = fs::symlink_metadata(path) {
            use std::os::unix::fs::MetadataExt;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err(crate::protocol::failure_details(
                    "work_directory_not_private",
                    Some(false),
                    "work directory must be a real directory owned by the current user",
                    serde_json::json!({"path":path,"preserved":true}),
                ));
            }
        }
    }
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = File::open(path)?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("work directory ownership changed; existing evidence preserved".into());
        }
        if metadata.mode() & 0o777 != 0o700 {
            directory.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub(crate) struct DiskSpace {
    pub available_bytes: u64,
    pub allocation_unit: u64,
    pub available_files: Option<u64>,
}
impl DiskSpace {
    pub fn require_files(&self, path: &Path, required_files: u64, stage: &str) -> Result<()> {
        if self
            .available_files
            .is_some_and(|available| required_files > available)
        {
            return Err(crate::protocol::failure_details(
                "insufficient_disk_space",
                Some(true),
                "insufficient free filesystem inodes; existing evidence preserved",
                serde_json::json!({"path":path,"stage":stage,"required_files":required_files,
                    "available_files":self.available_files,"preserved":true}),
            ));
        }
        Ok(())
    }

    pub fn allocation(&self, bytes: u64) -> Result<u64> {
        if self.allocation_unit == 0 {
            return Err("filesystem allocation unit must be positive".into());
        }
        bytes
            .div_ceil(self.allocation_unit)
            .checked_mul(self.allocation_unit)
            .ok_or_else(|| "chunk allocation estimate overflow".into())
    }

    pub fn require(&self, path: &Path, required_bytes: u64) -> Result<()> {
        self.require_at(path, required_bytes, "prepare_chunks")
    }
    pub fn require_at(&self, path: &Path, required_bytes: u64, stage: &str) -> Result<()> {
        if required_bytes > self.available_bytes {
            return Err(crate::protocol::failure_details(
                "insufficient_disk_space",
                Some(true),
                format!(
                    "pending audio chunks require {required_bytes} bytes; {} bytes available; existing media preserved",
                    self.available_bytes
                ),
                serde_json::json!({"path":path.to_string_lossy(),"required_bytes":required_bytes,"available_bytes":self.available_bytes,"allocation_unit_bytes":self.allocation_unit,"stage":stage}),
            ));
        }
        Ok(())
    }
}

pub(crate) fn disk_space(path: &Path) -> Result<DiskSpace> {
    disk_space_at(path, "prepare_chunks")
}
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
pub(crate) fn disk_space_at(path: &Path, stage: &str) -> Result<DiskSpace> {
    #[cfg(test)]
    if let Some(space) = TEST_SPACE.get() {
        return Ok(space);
    }
    let query = || -> Result<DiskSpace> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            let mut result = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            // statvfs initializes the complete structure only on success. f_bavail
            // excludes reserved blocks unavailable to an ordinary process.
            if unsafe { libc::statvfs(path.as_ptr(), result.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let result = unsafe { result.assume_init() };
            let allocation_unit = u64::try_from(u128::from(result.f_frsize))?;
            if allocation_unit == 0 {
                return Err("filesystem reported a zero allocation unit".into());
            }
            let available_bytes =
                u64::try_from(u128::from(result.f_bavail) * u128::from(allocation_unit))?;
            Ok(DiskSpace {
                available_bytes,
                allocation_unit,
                available_files: (result.f_files > 0)
                    .then_some(u64::try_from(u128::from(result.f_favail))?),
            })
        }
        #[cfg(not(unix))]
        Err("filesystem capacity checks are supported on Unix only".into())
    };
    query().map_err(|error| {
        crate::protocol::failure_details(
            "disk_space_unavailable",
            None,
            format!("cannot query filesystem capacity: {error}; existing evidence preserved"),
            serde_json::json!({"path":path.to_string_lossy(),"stage":stage}),
        )
    })
}

/// Capacity can change after preflight; preserve typed write failures for agents.
pub(crate) fn audio_write_error(
    error: Box<dyn std::error::Error + Send + Sync>,
    path: &Path,
    stage: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    // Serde exposes io::Error::source(), which omits ordinary OS errors.
    // Its consuming conversion preserves the original errno without parsing text.
    let error: Box<dyn std::error::Error + Send + Sync> =
        match error.downcast::<serde_json::Error>() {
            Ok(error) if error.is_io() => Box::new(std::io::Error::from(*error)),
            Ok(error) => error,
            Err(error) => error,
        };
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
    while let Some(current) = cause {
        if current.downcast_ref::<std::io::Error>().is_some_and(|e| {
            e.raw_os_error() == Some(libc::ENOSPC) || e.raw_os_error() == Some(libc::EDQUOT)
        }) {
            return crate::protocol::failure_details(
                "insufficient_disk_space",
                Some(true),
                format!("storage write failed: {error}; existing evidence preserved"),
                serde_json::json!({"path":path.to_string_lossy(),"stage":stage}),
            );
        }
        cause = current.source();
    }
    error
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlink: {}", path.display()).into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn reject_nonregular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(format!("refusing nonregular artifact {}", path.display()).into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Publish complete bytes in the destination directory without partial files.
pub fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let space = disk_space_at(parent, "publication")?;
        space.require_at(path, space.allocation(bytes.len() as u64)?, "publication")?;
    }
    atomic_write_with(path, replace, |file| Ok(file.write_all(bytes)?))
}

/// Stream an artifact into an unpublished file; callback failures preserve the destination.
pub fn atomic_write_with(
    path: &Path,
    replace: bool,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    atomic_write_impl(path, replace, write).map_err(|e| audio_write_error(e, path, "publication"))
}
fn atomic_write_impl(
    path: &Path,
    replace: bool,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    reject_symlink(path)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    #[cfg(unix)]
    {
        let space = disk_space_at(parent, "publication")?;
        space.require_files(parent, 1, "publication")?;
    }
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(
            ".agent-youtube-tmp-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match private_file().open(&candidate) {
            Ok(file) => break (Temporary(candidate), file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    write(&mut file)?;
    file.sync_all()?;
    drop(file);
    reject_symlink(path)?;
    if replace {
        fs::rename(&temporary.0, path)?;
    } else {
        fs::hard_link(&temporary.0, path)?;
        fs::remove_file(&temporary.0)?;
    }
    sync_parent(path)
}

pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        #[cfg(test)]
        HASH_BYTES.set(HASH_BYTES.get() + count as u64);
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// The persistent inode is intentional: deleting it would permit split locks.
pub struct Lock {
    _file: File,
    unlock_on_drop: bool,
}
impl Drop for Lock {
    fn drop(&mut self) {
        // Release explicitly even during the brief fork-before-exec window of a child.
        if self.unlock_on_drop {
            let _ = self._file.unlock();
        }
    }
}
impl Lock {
    /// Observe an existing lock without creating files or changing permissions.
    /// Some holds a shared idle observation; None means a writer owns the job.
    pub(crate) fn observe(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(".agent-youtube.lock");
        reject_nonregular(&path)?;
        let file = open_regular_input(&path)?;
        match file.try_lock_shared() {
            Ok(()) => Ok(Some(Self {
                _file: file,
                unlock_on_drop: true,
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    /// On unverified worker teardown, let inherited handles retain the lock.
    pub fn leave_locked_for_child(&mut self) {
        self.unlock_on_drop = false;
    }
    /// Clone the locked open-file description for a child to retain across exec.
    /// Keep this lock alive until the child finishes; normal Drop explicitly unlocks.
    pub fn child_handle(&self) -> Result<File> {
        Ok(self._file.try_clone()?)
    }
    pub(crate) fn control(dir: &Path) -> Result<Self> {
        Self::acquire_path(dir.join(".agent-youtube-control.lock"))
    }
    pub fn acquire(dir: &Path) -> Result<Self> {
        Self::acquire_path(dir.join(".agent-youtube.lock"))
    }
    fn acquire_path(path: PathBuf) -> Result<Self> {
        reject_symlink(&path)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("refusing non-regular lock: {}", path.display()).into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let file = match private_file().read(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                OpenOptions::new().read(true).write(true).open(&path)?
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!("refusing non-regular lock: {}", path.display()).into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = file.metadata()?;
            if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
                return Err("lock path changed during acquisition".into());
            }
        }
        file.try_lock().map_err(|error| {
            let busy = matches!(error, fs::TryLockError::WouldBlock);
            crate::protocol::failure_details(
                if busy {
                    "work_directory_busy"
                } else {
                    "work_directory_lock_failed"
                },
                if busy { Some(true) } else { None },
                format!(
                    "cannot lock {} (another invocation may be active): {error}",
                    path.display()
                ),
                serde_json::json!({"path":path,"preserved":true}),
            )
        })?;
        Ok(Self {
            _file: file,
            unlock_on_drop: true,
        })
    }
}

pub fn read_text(path: &Path, max_bytes: u64) -> Result<String> {
    use std::io::Read;
    let mut text = String::new();
    fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(format!(
            "{} exceeds supported size {max_bytes} bytes",
            path.display()
        )
        .into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn concurrent_idle_observers_share_the_lock_and_exclude_writers() {
        let dir = temp();
        drop(Lock::acquire(&dir).unwrap());
        let first = Lock::observe(&dir).unwrap().unwrap();
        let second = Lock::observe(&dir).unwrap().unwrap();
        assert!(Lock::acquire(&dir).is_err());
        drop(first);
        assert!(Lock::acquire(&dir).is_err());
        drop(second);
        assert!(Lock::acquire(&dir).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn busy_lock_has_structured_retry_guidance() {
        let dir = temp();
        let held = Lock::acquire(&dir).unwrap();
        let error = Lock::acquire(&dir).err().unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "work_directory_busy");
        assert_eq!(failure.retryable, Some(true));
        drop(held);
        assert!(Lock::acquire(&dir).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
    fn temp() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-youtube-storage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }
    #[cfg(unix)]
    #[test]
    fn private_work_directory_preserves_unknown_files_and_rejects_foreign_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let unknown = dir.join("unknown");
        fs::write(&unknown, b"untouched").unwrap();
        fs::set_permissions(&unknown, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        private_directory(&dir, true).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&unknown).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"untouched");
        if unsafe { libc::geteuid() } != 0 {
            let before = fs::metadata("/tmp").unwrap().permissions().mode();
            assert!(private_directory(Path::new("/tmp"), true).is_err());
            assert_eq!(fs::metadata("/tmp").unwrap().permissions().mode(), before);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deferred_unlock_stays_with_inherited_open_description() {
        let dir = temp();
        let mut lock = Lock::acquire(&dir).unwrap();
        let child_handle = lock.child_handle().unwrap();
        lock.leave_locked_for_child();
        drop(lock);
        assert!(Lock::acquire(&dir).is_err());
        drop(child_handle);
        assert!(Lock::acquire(&dir).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disk_space_uses_the_target_filesystem_and_reports_query_errors() {
        let dir = temp();
        let space = disk_space(&dir).unwrap();
        assert!(space.allocation_unit > 0);
        assert!(space.available_bytes > 0);
        let error = disk_space(&dir.join("missing-directory")).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "disk_space_unavailable"
        );
        assert!(disk_space(Path::new("invalid\0path")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let invalid = Path::new(std::ffi::OsStr::from_bytes(b"missing-\xff"));
            assert!(disk_space(invalid).is_err());
            assert!(space.require(invalid, u64::MAX).is_err());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn serde_writer_quota_failures_keep_typed_publication_error() {
        struct Full(i32);
        impl std::io::Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(self.0))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dir = temp();
        let destination = dir.join("existing.json");
        fs::write(&destination, b"original").unwrap();
        for code in [libc::ENOSPC, libc::EDQUOT] {
            let error = atomic_write_with(&destination, true, |file| {
                file.write_all(b"partial")?;
                serde_json::to_writer(Full(code), &serde_json::json!({"data":1}))?;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "insufficient_disk_space"
            );
            assert_eq!(fs::read(&destination).unwrap(), b"original");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inode_exhaustion_and_publication_quota_preserve_destination() {
        let dir = temp();
        let space = DiskSpace {
            available_bytes: u64::MAX,
            allocation_unit: 4096,
            available_files: Some(0),
        };
        let error = space.require_files(&dir, 1, "recognize").unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "insufficient_disk_space"
        );
        let destination = dir.join("existing.json");
        fs::write(&destination, b"original").unwrap();
        for code in [libc::ENOSPC, libc::EDQUOT] {
            let error = atomic_write_with(&destination, true, |file| {
                use std::io::Write;
                file.write_all(b"partial")?;
                Err(std::io::Error::from_raw_os_error(code).into())
            })
            .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "insufficient_disk_space"
            );
            assert_eq!(fs::read(&destination).unwrap(), b"original");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disk_estimate_rounds_each_file_and_checks_capacity_and_overflow() {
        let space = DiskSpace {
            available_bytes: 8192,
            allocation_unit: 4096,
            available_files: None,
        };
        assert_eq!(space.allocation(44).unwrap(), 4096);
        assert_eq!(space.allocation(4096).unwrap(), 4096);
        assert_eq!(space.allocation(4097).unwrap(), 8192);
        assert!(space.require(Path::new("chunks"), 8192).is_ok());
        let error = space.require(Path::new("chunks"), 8193).unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "insufficient_disk_space");
        assert_eq!(failure.retryable, Some(true));
        assert!(space.allocation(u64::MAX).is_err());
        assert!(
            DiskSpace {
                available_bytes: 0,
                allocation_unit: 0,
                available_files: None,
            }
            .allocation(44)
            .is_err()
        );
    }
    #[test]
    fn publishes_without_clobber_and_replaces_checkpoint() {
        let dir = temp();
        let path = dir.join("artifact");
        atomic_write(&path, b"original", false).unwrap();
        assert!(atomic_write(&path, b"replacement", false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        atomic_write(&path, b"checkpoint", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"checkpoint");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn streamed_publication_failure_preserves_existing_output() {
        let dir = temp();
        let path = dir.join("output");
        fs::write(&path, b"original").unwrap();
        let result = atomic_write_with(&path, true, |file| {
            file.write_all(b"partial replacement")?;
            Err("late validation failure".into())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hashes_file_contents() {
        let dir = temp();
        let path = dir.join("file");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            digest(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = temp();
        let first = Lock::acquire(&dir).unwrap();
        assert!(Lock::acquire(&dir).is_err());
        drop(first);
        assert!(Lock::acquire(&dir).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink_targets() {
        let dir = temp();
        let target = dir.join("outside");
        fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("link")).unwrap();
        assert!(atomic_write(&dir.join("link"), b"changed", true).is_err());
        std::os::unix::fs::symlink(&target, dir.join(".agent-youtube.lock")).unwrap();
        assert!(Lock::acquire(&dir).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn lock_process_fixture() {
        if let Some(directory) = std::env::var_os("AGENT_YOUTUBE_LOCK_TEST_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let _lock = Lock::acquire(&directory).unwrap();
            fs::write(directory.join("ready"), b"ready").unwrap();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    #[test]
    fn operating_system_releases_lock_after_process_death() {
        let dir = temp();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::tests::lock_process_fixture",
                "--nocapture",
            ])
            .env("AGENT_YOUTUBE_LOCK_TEST_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !dir.join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = dir.join("ready").exists();
        let exclusive = Lock::acquire(&dir).is_err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready, "child never acquired lock");
        assert!(exclusive);
        let lock = Lock::acquire(&dir).unwrap();
        drop(lock);
        fs::remove_dir_all(dir).unwrap();
    }
}
