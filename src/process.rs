use crate::Result;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[cfg(test)]
thread_local! {
    static TEST_SHUTDOWN: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    static TEST_CAPTURE_FAIL: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static TEST_UNVERIFIED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_CHILD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
pub struct Runner {
    pub cancelled: Arc<AtomicBool>,
    lock: Option<Arc<File>>,
    timeout: Duration,
    memory_bytes: Option<u64>,
}
impl Runner {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            lock: None,
            timeout: Duration::from_secs(1800),
            memory_bytes: None,
        }
    }

    pub fn with_lock(&self, file: File) -> Self {
        Self {
            cancelled: self.cancelled.clone(),
            lock: Some(Arc::new(file)),
            timeout: self.timeout,
            memory_bytes: self.memory_bytes,
        }
    }

    pub fn with_timeout(&self, timeout: Duration) -> Self {
        let mut runner = self.clone();
        runner.timeout = timeout;
        runner
    }

    /// Poll the combined resident set of the Linux worker process group.
    /// This is an observed budget, not an OS reservation or hard allocation limit.
    pub fn with_memory_budget(&self, bytes: Option<u64>) -> Result<Self> {
        if bytes == Some(0) {
            return Err("ASR memory budget must be positive".into());
        }
        if bytes.is_some() && !cfg!(target_os = "linux") {
            return Err("ASR resident-memory monitoring requires Linux".into());
        }
        let mut runner = self.clone();
        runner.memory_bytes = bytes;
        Ok(runner)
    }

    pub fn run(
        &self,
        program: &Path,
        args: &[OsString],
        stdout: &Path,
        stderr: &Path,
    ) -> Result<()> {
        self.run_impl(
            program,
            args,
            stdout,
            stderr,
            self.lock.as_deref(),
            &mut || Ok(false),
        )
    }

    pub fn run_with_lock(
        &self,
        program: &Path,
        args: &[OsString],
        stdout: &Path,
        stderr: &Path,
        lock: &File,
    ) -> Result<()> {
        self.run_impl(program, args, stdout, stderr, Some(lock), &mut || Ok(false))
    }

    pub fn run_with_progress(
        &self,
        program: &Path,
        args: &[OsString],
        stdout: &Path,
        stderr: &Path,
        mut progress: impl FnMut() -> Result<bool>,
    ) -> Result<()> {
        self.run_impl(
            program,
            args,
            stdout,
            stderr,
            self.lock.as_deref(),
            &mut progress,
        )
    }

    #[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
    fn run_impl(
        &self,
        program: &Path,
        args: &[OsString],
        stdout: &Path,
        stderr: &Path,
        lock: Option<&File>,
        progress: &mut dyn FnMut() -> Result<bool>,
    ) -> Result<()> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("operation cancelled".into());
        }
        let output = crate::storage::private_file().open(stdout)?;
        let errors = crate::storage::private_file().open(stderr)?;
        // Keeping the same locked open-file description on stdin protects the run
        // after abrupt parent death. Tools must preserve stdin for this safeguard.
        let input = match lock {
            Some(file) => Stdio::from(file.try_clone()?),
            None => Stdio::null(),
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Ambient options must not change recognition or enable destructive batch policies.
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|n| n.starts_with("GIGASTT_")) {
                command.env_remove(name);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            // Only this child changes its umask; avoid process-global changes in the host.
            unsafe {
                command.pre_exec(|| {
                    libc::umask(0o077);
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().map_err(|error| {
            let (reason, retryable) = match error.kind() {
                std::io::ErrorKind::NotFound => ("not_found", Some(false)),
                std::io::ErrorKind::PermissionDenied => ("permission_denied", Some(false)),
                std::io::ErrorKind::InvalidInput => ("invalid_input", Some(false)),
                _ => ("other", None),
            };
            crate::protocol::failure_details(
                "process_spawn_failed",
                retryable,
                format!(
                    "cannot launch {}: {error}; stderr log: {}",
                    program.display(),
                    stderr.display()
                ),
                serde_json::json!({"program":program,"reason":reason,"os_error":error.raw_os_error(),"stderr_log":stderr}),
            )
        })?;
        #[cfg(test)]
        TEST_CHILD.set(child.id());
        let overflow = Arc::new(AtomicBool::new(false));
        let stop_readers = Arc::new(AtomicBool::new(false));
        let mut readers = [None, None];
        let setup = (|| -> Result<()> {
            let out = child.stdout.take().ok_or("missing stdout pipe")?;
            let err = child.stderr.take().ok_or("missing stderr pipe")?;
            #[cfg(unix)]
            {
                nonblocking(&out)?;
                nonblocking(&err)?;
            }
            readers[0] = Some(
                capture_log(out, output, overflow.clone(), stop_readers.clone()).map_err(|e| {
                    crate::memory::allocation_error(program, "subprocess_log_thread", e)
                })?,
            );
            readers[1] = Some(
                capture_log(err, errors, overflow.clone(), stop_readers.clone()).map_err(|e| {
                    crate::memory::allocation_error(program, "subprocess_log_thread", e)
                })?,
            );
            Ok(())
        })();
        if let Err(error) = setup {
            let deadline = shutdown_deadline();
            let stopped = terminate(&mut child, deadline);
            stop_readers.store(true, Ordering::SeqCst);
            let _ = finish_readers(&mut readers, &stop_readers, deadline, [stdout, stderr]);
            stopped?;
            return Err(error);
        }
        let mut started = std::time::Instant::now();
        let mut memory = crate::process_memory::Monitor::new(self.memory_bytes);
        let outcome = loop {
            if self.cancelled.load(Ordering::SeqCst) {
                break Err(crate::protocol::failure(
                    "interrupted",
                    Some(true),
                    format!(
                        "{} cancelled; stderr log: {}",
                        program.display(),
                        stderr.display()
                    ),
                ));
            }
            if let Err(error) = memory.check(child.id(), program) {
                break Err(error);
            }
            match progress() {
                Ok(true) => started = std::time::Instant::now(),
                Ok(false) => {}
                Err(error) => break Err(error),
            }
            if started.elapsed() >= self.timeout {
                break Err(crate::protocol::failure(
                    "process_timeout",
                    Some(true),
                    format!(
                        "{} exceeded {} seconds; stderr log: {}",
                        program.display(),
                        self.timeout.as_secs_f64(),
                        stderr.display()
                    ),
                ));
            }
            if overflow.load(Ordering::SeqCst) {
                break Err(crate::protocol::failure(
                    "diagnostic_limit",
                    Some(false),
                    "subprocess diagnostic output exceeded 1 MiB per stream",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => break Err(error.into()),
            }
        };
        let deadline = shutdown_deadline();
        let stopped = terminate(&mut child, deadline);
        memory.report();
        let logs = finish_readers(&mut readers, &stop_readers, deadline, [stdout, stderr]);
        stopped?;
        logs?;
        let status = outcome?;
        if overflow.load(Ordering::SeqCst) {
            return Err(crate::protocol::failure(
                "diagnostic_limit",
                Some(false),
                "subprocess diagnostic output exceeded 1 MiB per stream",
            ));
        }
        if status.success() {
            return Ok(());
        }
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            status.signal()
        };
        #[cfg(not(unix))]
        let signal: Option<i32> = None;
        Err(crate::protocol::failure_details(
            "process_failed",
            None,
            format!(
                "{} failed ({status}); private stderr log: {}",
                program.display(),
                stderr.display()
            ),
            serde_json::json!({"path":program,"exit_code":status.code(),"signal":signal,
                "stderr":stderr,"preserved":true}),
        ))
    }
}

type LogReader = std::thread::JoinHandle<std::io::Result<()>>;

fn shutdown_deadline() -> std::time::Instant {
    #[cfg(test)]
    if let Some(duration) = TEST_SHUTDOWN.get() {
        return std::time::Instant::now() + duration;
    }
    std::time::Instant::now() + Duration::from_secs(5)
}

#[cfg(unix)]
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
pub(crate) fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    // Child diagnostic pipes are owned by this invocation; preserve existing flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn capture_log<R: Read + Send + 'static>(
    mut pipe: R,
    mut file: File,
    overflow: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<LogReader> {
    #[cfg(test)]
    {
        let fail = TEST_CAPTURE_FAIL.get();
        if let Some(left) = fail {
            TEST_CAPTURE_FAIL.set(left.checked_sub(1));
            if left == 0 {
                return Err(std::io::Error::from_raw_os_error(libc::EAGAIN));
            }
        }
    }
    std::thread::Builder::new()
        .name("process-log".into())
        .spawn(move || {
            use std::io::Write;
            let mut kept = 0;
            let mut buffer = [0_u8; 8192];
            while !stop.load(Ordering::SeqCst) {
                let n = match pipe.read(&mut buffer) {
                    Ok(n) => n,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if n == 0 || stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let write = n.min((1024 * 1024_usize).saturating_sub(kept));
                file.write_all(&buffer[..write])?;
                kept += write;
                if write < n {
                    overflow.store(true, Ordering::SeqCst);
                }
            }
            Ok(())
        })
}

fn finish_readers(
    readers: &mut [Option<LogReader>; 2],
    stop: &AtomicBool,
    deadline: std::time::Instant,
    paths: [&Path; 2],
) -> Result<()> {
    while readers.iter().flatten().any(|reader| !reader.is_finished())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let timed_out = readers.iter().flatten().any(|reader| !reader.is_finished());
    stop.store(true, Ordering::SeqCst);
    // Nonblocking Unix readers need at most one polling interval to observe stop.
    let close_deadline = std::time::Instant::now() + Duration::from_millis(50);
    while readers.iter().flatten().any(|reader| !reader.is_finished())
        && std::time::Instant::now() < close_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut failure = None;
    for (reader, path) in readers.iter_mut().zip(paths) {
        if let Some(reader) = reader.take()
            && reader.is_finished()
        {
            let result = reader
                .join()
                .map_err(|_| {
                    crate::protocol::failure(
                        "process_log_failed",
                        None,
                        "diagnostic reader panicked",
                    )
                })
                .and_then(|result| {
                    result.map_err(|e| {
                        crate::storage::audio_write_error(e.into(), path, "subprocess_log")
                    })
                });
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
            // A blocked OS read/write is not joinable within a userspace deadline.
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if timed_out {
        return Err(crate::protocol::failure_details(
            "process_log_timeout",
            None,
            "worker diagnostics did not close within shutdown deadline; media preserved",
            serde_json::json!({"stdout":paths[0],"stderr":paths[1],"preserved":true}),
        ));
    }
    Ok(())
}

fn teardown_failure(
    group: u32,
    message: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        "process_teardown_failed",
        None,
        message,
        serde_json::json!({"process_group":group,"group_stopped":false,"preserved":true}),
    )
}

#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
fn terminate(child: &mut Child, deadline: std::time::Instant) -> Result<()> {
    #[cfg(unix)]
    {
        let group = i32::try_from(child.id())
            .ok()
            .filter(|group| *group > 1)
            .ok_or_else(|| teardown_failure(child.id(), "invalid worker process group"))?;
        // Signal the owned group directly, without launching kill/ps helpers.
        if unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    loop {
        let exited = child
            .try_wait()
            .map_err(|e| teardown_failure(child.id(), e))?
            .is_some();
        #[cfg(unix)]
        let running = group_is_running(child.id()).map_err(|e| teardown_failure(child.id(), e))?;
        #[cfg(not(unix))]
        let running = !exited;
        if exited && !running {
            #[cfg(test)]
            if TEST_UNVERIFIED.get() {
                return Err(teardown_failure(child.id(), "injected unverified teardown"));
            }
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(teardown_failure(
                child.id(),
                "worker process group did not stop within shutdown deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
fn group_is_running(group: u32) -> Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let stat = match std::fs::read(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if crate::process_memory::vanished_or_inaccessible(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        if let Some(end) = stat.iter().rposition(|byte| *byte == b')') {
            let mut fields = std::str::from_utf8(&stat[end + 1..])?.split_whitespace();
            let state = fields.next();
            let member = fields.nth(1).and_then(|n| n.parse::<u32>().ok());
            if member == Some(group) && state.is_some_and(|s| s != "Z" && s != "X") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
#[cfg(all(unix, not(target_os = "linux")))]
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
fn group_is_running(group: u32) -> Result<bool> {
    let group = i32::try_from(group)?;
    if group <= 1 {
        return Err("invalid worker process group".into());
    }
    if unsafe { libc::kill(-group, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error.into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn temp() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-youtube-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn detached_pipe_holder_cannot_block_shutdown() {
        let dir = temp();
        let script = "import os,time,sys\npid=os.fork()\nif pid==0:\n os.setsid();open(sys.argv[1],'w').write(str(os.getpid()));time.sleep(2);os._exit(0)\nwhile not os.path.exists(sys.argv[1]):time.sleep(.005)\nos._exit(0)";
        TEST_SHUTDOWN.set(Some(Duration::from_millis(80)));
        let started = Instant::now();
        let result = Runner::new(Arc::new(AtomicBool::new(false))).run(
            Path::new("/usr/bin/python3"),
            &[
                "-c".into(),
                script.into(),
                dir.join("escaped-pid").into_os_string(),
            ],
            &dir.join("out"),
            &dir.join("err"),
        );
        TEST_SHUTDOWN.set(None);
        let elapsed = started.elapsed();
        if let Ok(pid) = fs::read_to_string(dir.join("escaped-pid")) {
            unsafe {
                libc::kill(pid.parse::<i32>().unwrap(), libc::SIGKILL);
            }
        }
        assert!(
            elapsed < Duration::from_millis(750),
            "shutdown waited {elapsed:?}"
        );
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "process_log_timeout"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn capture_thread_failure_stops_partial_setup_and_allows_retry() {
        let dir = temp();
        let runner = Runner::new(Arc::new(AtomicBool::new(false)));
        for fail_after in [0, 1] {
            TEST_CAPTURE_FAIL.set(Some(fail_after));
            let result = runner.run(
                Path::new("/bin/sh"),
                &["-c".into(), "printf partial; sleep .1".into()],
                &dir.join(format!("out-{fail_after}")),
                &dir.join(format!("err-{fail_after}")),
            );
            TEST_CAPTURE_FAIL.set(None);
            assert!(!group_is_running(TEST_CHILD.get()).unwrap());
            let error = result.unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "resource_limit"
            );
        }
        runner
            .run(
                Path::new("/bin/true"),
                &[],
                &dir.join("retry-out"),
                &dir.join("retry-err"),
            )
            .unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_private_logs_without_copying_child_content_to_errors() {
        let dir = temp();
        let runner = Runner::new(Arc::new(AtomicBool::new(false)));
        let error = runner
            .run(
                Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "printf output; head -c 20000 /dev/zero >&2; printf AUDIT_FAKE_SECRET >&2; exit 7".into(),
                ],
                &dir.join("stdout"),
                &dir.join("stderr"),
            )
            .unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "process_failed");
        assert_eq!(failure.retryable, None);
        let error = error.to_string();
        assert_eq!(fs::read(dir.join("stdout")).unwrap(), b"output");
        assert!(error.contains('7') && error.contains("stderr"));
        assert!(!error.contains("AUDIT_FAKE_SECRET"));
        assert!(
            fs::read_to_string(dir.join("stderr"))
                .unwrap()
                .contains("AUDIT_FAKE_SECRET")
        );
        assert!(error.len() < 10000);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cancellation_terminates_descendants() {
        let dir = temp();
        let cancelled = Arc::new(AtomicBool::new(false));
        let runner = Runner::new(cancelled.clone());
        let pid_path = dir.join("descendant");
        let script = "sleep 60 & child=$!; printf '%s' \"$child\" > \"$1\"; wait";
        let args = vec![
            "-c".into(),
            script.into(),
            "fixture".into(),
            pid_path.clone().into_os_string(),
        ];
        let thread_dir = dir.clone();
        let handle = std::thread::spawn(move || {
            runner.run(
                Path::new("/bin/sh"),
                &args,
                &thread_dir.join("stdout"),
                &thread_dir.join("stderr"),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = fs::read_to_string(&pid_path).unwrap();
        cancelled.store(true, Ordering::SeqCst);
        assert!(
            handle
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancel")
        );
        // A killed child may briefly remain a zombie; it must never remain running.
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "descendant still running: {state}"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn leader_exit_never_leaves_a_background_writer() {
        for code in [0, 7] {
            let dir = temp();
            let runner = Runner::new(Arc::new(AtomicBool::new(false)));
            let result = runner.run(
                Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "(sleep 0.3; printf late > \"$1\") & exit \"$2\"".into(),
                    "fixture".into(),
                    dir.join("marker").into_os_string(),
                    code.to_string().into(),
                ],
                &dir.join("stdout"),
                &dir.join("stderr"),
            );
            std::thread::sleep(Duration::from_millis(500));
            let wrote_after_exit = dir.join("marker").exists();
            fs::remove_dir_all(dir).unwrap();
            assert_eq!(result.is_ok(), code == 0, "leader outcome changed");
            assert!(
                !wrote_after_exit,
                "background writer outlived leader exit {code}"
            );
        }
    }
    #[test]
    fn deadline_and_log_budget_stop_children() {
        for (script, timeout, code) in [
            ("sleep 60", Duration::from_millis(80), "process_timeout"),
            (
                "head -c 2000000 /dev/zero; sleep 60",
                Duration::from_secs(3),
                "diagnostic_limit",
            ),
        ] {
            let dir = temp();
            let runner = Runner::new(Arc::new(AtomicBool::new(false))).with_timeout(timeout);
            let started = Instant::now();
            let error = runner
                .run(
                    Path::new("/bin/sh"),
                    &["-c".into(), script.into()],
                    &dir.join("out"),
                    &dir.join("err"),
                )
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                code
            );
            assert!(started.elapsed() < Duration::from_secs(5));
            assert!(fs::metadata(dir.join("out")).unwrap().len() <= 1024 * 1024);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn cancelled_runner_does_not_spawn() {
        let dir = temp();
        let runner = Runner::new(Arc::new(AtomicBool::new(true)));
        assert!(
            runner
                .run(
                    Path::new("/bin/true"),
                    &[],
                    &dir.join("stdout"),
                    &dir.join("stderr")
                )
                .is_err()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn refuses_existing_and_symlinked_logs() {
        let dir = temp();
        let runner = Runner::new(Arc::new(AtomicBool::new(false)));
        let target = dir.join("source");
        fs::write(&target, b"keep").unwrap();
        let stdout = dir.join("stdout");
        std::os::unix::fs::symlink(&target, &stdout).unwrap();
        assert!(
            runner
                .run(Path::new("/bin/true"), &[], &stdout, &dir.join("stderr"))
                .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn locked_parent_process_fixture() {
        if let Some(directory) = std::env::var_os("AGENT_YOUTUBE_LOCKED_PARENT_TEST_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let lock = crate::storage::Lock::acquire(&directory).unwrap();
            let runner = Runner::new(Arc::new(AtomicBool::new(false)))
                .with_lock(lock.child_handle().unwrap());
            runner
                .run(
                    Path::new("/bin/sh"),
                    &[
                        "-c".into(),
                        "printf '%s' \"$$\" > \"$1\"; exec sleep 60".into(),
                        "fixture".into(),
                        directory.join("worker-pid").into_os_string(),
                    ],
                    &directory.join("stdout"),
                    &directory.join("stderr"),
                )
                .unwrap();
        }
    }

    #[test]
    fn child_keeps_lock_when_parent_is_killed() {
        let dir = temp();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::tests::locked_parent_process_fixture",
                "--nocapture",
            ])
            .env("AGENT_YOUTUBE_LOCKED_PARENT_TEST_DIR", &dir)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid_path = dir.join("worker-pid");
        let pid = loop {
            if let Ok(value) = fs::read_to_string(&pid_path)
                && value.trim().parse::<u32>().is_ok()
            {
                break Ok(value);
            }
            if Instant::now() >= deadline {
                break Err("worker PID was not published");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        parent.kill().unwrap();
        parent.wait().unwrap();
        let pid = pid.unwrap();
        let retained = crate::storage::Lock::acquire(&dir).is_err();
        Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", pid.trim())])
            .status()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(lock) = crate::storage::Lock::acquire(&dir) {
                drop(lock);
                break;
            }
            assert!(Instant::now() < deadline, "worker lock did not release");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            retained,
            "worker failed to retain exclusive lock after parent death"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
