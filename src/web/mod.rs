//! A bounded private Rust worker for Web attestation and player challenges.
mod page;
mod player;
mod runtime;

use crate::{Result, download::http::check};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.10 Safari/605.1.1";

const INPUT_LIMIT: u64 = 8 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
#[cfg(test)]
thread_local! {
    static TEST_REQUEST_TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    static TEST_READER_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_STARTED_CHILD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static TEST_EXECUTABLE: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", deny_unknown_fields)]
enum Request {
    Mint { video: String, refresh: bool },
    N { player: String, challenge: String },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    token: Vec<u8>,
    #[serde(default)]
    expires_in: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    solution: String,
    #[serde(default)]
    failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<WorkerFailure>,
}

// Only these structured fields cross IPC. Script exceptions, response bodies,
// signed URLs and token state must never become application diagnostics.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerFailure {
    code: String,
    retryable: Option<bool>,
    http_status: Option<u64>,
    retry_after: Option<String>,
    retry_after_seconds: Option<u64>,
}
fn worker_code(code: &str) -> Option<&'static str> {
    match code {
        "http_transport" => Some("http_transport"),
        "youtube_rate_limited" => Some("youtube_rate_limited"),
        "youtube_temporary_failure" => Some("youtube_temporary_failure"),
        "youtube_http_error" => Some("youtube_http_error"),
        "youtube_attestation_failed" => Some("youtube_attestation_failed"),
        "youtube_player_challenge_failed" => Some("youtube_player_challenge_failed"),
        _ => None,
    }
}
impl WorkerFailure {
    fn capture(error: &(dyn std::error::Error + 'static), mint: bool) -> Self {
        let typed = error
            .downcast_ref::<crate::protocol::Failure>()
            .filter(|failure| worker_code(failure.code).is_some());
        Self {
            code: typed
                .map_or(
                    if mint {
                        "youtube_attestation_failed"
                    } else {
                        "youtube_player_challenge_failed"
                    },
                    |failure| failure.code,
                )
                .into(),
            retryable: typed.and_then(|failure| failure.retryable),
            http_status: typed.and_then(|failure| failure.http_status()),
            retry_after: typed
                .and_then(|failure| failure.retry_after())
                .filter(|header| header.len() <= 256)
                .map(str::to_owned),
            retry_after_seconds: typed.and_then(|failure| failure.retry_after_seconds()),
        }
    }
    fn into_error(self) -> Box<dyn std::error::Error + Send + Sync> {
        let Some(code) = worker_code(&self.code) else {
            return crate::protocol::failure(
                "web_runtime_protocol",
                Some(false),
                "invalid Web runtime error code",
            );
        };
        crate::protocol::failure_details(
            code,
            self.retryable,
            "YouTube Web operation failed",
            serde_json::json!({"http_status":self.http_status,"retry_after":self.retry_after,"retry_after_seconds":self.retry_after_seconds}),
        )
    }
}
pub(crate) struct Token {
    pub(crate) bytes: Vec<u8>,
    pub(crate) expires: Instant,
}
pub(crate) struct Worker {
    child: Child,
    input: ChildStdin,
    responses: mpsc::Receiver<std::result::Result<Response, &'static str>>,
    shutdown_attempted: bool,
}
impl Worker {
    #[cfg(all(test, unix))]
    pub(crate) fn idle_fixture() -> (Self, u32) {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let id = child.id();
        let input = child.stdin.take().unwrap();
        let (_, responses) = mpsc::channel();
        (
            Self {
                child,
                input,
                responses,
                shutdown_attempted: false,
            },
            id,
        )
    }

    #[cfg(all(test, unix))]
    pub(crate) fn failure_fixture(code: &'static str) -> Self {
        let (mut worker, _) = Self::idle_fixture();
        let (sender, responses) = mpsc::channel();
        worker.responses = responses;
        let error = crate::protocol::failure(code, Some(true), "fixture failure");
        sender
            .send(Ok(Response {
                token: vec![],
                expires_in: 0,
                solution: String::new(),
                failed: true,
                failure: Some(WorkerFailure::capture(error.as_ref(), true)),
            }))
            .unwrap();
        worker
    }

    #[cfg(all(test, unix))]
    pub(crate) fn solution_fixture() -> Self {
        let (mut worker, _) = Self::idle_fixture();
        let (sender, responses) = mpsc::channel();
        worker.responses = responses;
        sender
            .send(Ok(Response {
                token: vec![],
                expires_in: 0,
                solution: "solved".into(),
                failed: false,
                failure: None,
            }))
            .unwrap();
        worker
    }

    pub(crate) fn start() -> Result<Self> {
        #[cfg(test)]
        if let Some(executable) = TEST_EXECUTABLE.with(|path| path.borrow().clone()) {
            return Self::start_command(Command::new(executable));
        }
        #[cfg(target_os = "linux")]
        let executable = std::path::PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let executable = std::env::current_exe()?;
        Self::start_command(Command::new(executable))
    }
    #[cfg(test)]
    pub(crate) fn test_executable(executable: &std::path::Path) -> Result<Self> {
        TEST_EXECUTABLE.with(|path| *path.borrow_mut() = Some(executable.to_path_buf()));
        Self::start_command(Command::new(executable))
    }
    fn start_command(mut command: Command) -> Result<Self> {
        command
            .arg("--internal-web-runtime")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|error| {
            crate::protocol::failure_details(
                "web_runtime_start_failed",
                None,
                "could not start the Rust Web runtime",
                serde_json::json!({"os_error":error.raw_os_error(),"preserved":true}),
            )
        })?;
        #[cfg(test)]
        TEST_STARTED_CHILD.set(child.id());
        let Some(input) = child.stdin.take() else {
            stop_child(&mut child)?;
            return Err("Web runtime input pipe unavailable".into());
        };
        let Some(output) = child.stdout.take() else {
            stop_child(&mut child)?;
            return Err("Web runtime output pipe unavailable".into());
        };
        let (tx, responses) = mpsc::channel();
        let reader = response_reader(output, tx);
        if let Err(error) = reader {
            stop_child(&mut child)?;
            return Err(crate::memory::allocation_error(
                std::path::Path::new("web-response"),
                "web_response_thread",
                error,
            ));
        }
        Ok(Self {
            child,
            input,
            responses,
            shutdown_attempted: false,
        })
    }
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.shutdown_attempted {
            return Ok(());
        }
        self.shutdown_attempted = true;
        stop_child(&mut self.child)
    }
    fn request(&mut self, request: &Request, cancelled: &AtomicBool) -> Result<Response> {
        check(cancelled)?;
        let timeout = REQUEST_TIMEOUT;
        #[cfg(test)]
        let timeout = TEST_REQUEST_TIMEOUT.get().unwrap_or(timeout);
        let deadline = Instant::now() + timeout;
        let mut input = serde_json::to_vec(request)?;
        if input.len() as u64 + 1 > INPUT_LIMIT {
            return Err("Web runtime request exceeds its byte limit".into());
        }
        input.push(b'\n');
        deliver(&mut self.input, &input, cancelled, deadline)?;
        loop {
            check(cancelled)?;
            request_deadline(cancelled, deadline)?;
            match self.responses.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(response)) if !response.failed => return Ok(response),
                Ok(Ok(response)) => {
                    return Err(response
                        .failure
                        .map(WorkerFailure::into_error)
                        .unwrap_or_else(|| {
                            crate::protocol::failure(
                                "web_runtime_protocol",
                                Some(false),
                                "Web runtime omitted failure details",
                            )
                        }));
                }
                Ok(Err(message)) => return Err(message.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let timed_out = self
                        .child
                        .try_wait()?
                        .is_some_and(|status| status.code() == Some(124));
                    return Err(crate::protocol::failure(
                        if timed_out {
                            "web_runtime_timeout"
                        } else {
                            "web_runtime_failed"
                        },
                        Some(true),
                        "Web runtime exited before completing the operation",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    pub(crate) fn mint(
        &mut self,
        video: &str,
        refresh: bool,
        cancelled: &AtomicBool,
    ) -> Result<Token> {
        let response = self.request(
            &Request::Mint {
                video: video.into(),
                refresh,
            },
            cancelled,
        )?;
        if !(64..=4096).contains(&response.token.len())
            || !(1..=86_400).contains(&response.expires_in)
        {
            return Err("Web runtime returned invalid token metadata".into());
        }
        Ok(Token {
            bytes: response.token,
            expires: Instant::now() + Duration::from_secs(response.expires_in),
        })
    }
    pub(crate) fn solve_n(
        &mut self,
        player: String,
        challenge: String,
        cancelled: &AtomicBool,
    ) -> Result<String> {
        let response = self.request(&Request::N { player, challenge }, cancelled)?;
        if !valid_solution(&response.solution) {
            return Err("Web runtime returned an invalid player solution".into());
        }
        Ok(response.solution)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn response_reader(
    output: std::process::ChildStdout,
    tx: mpsc::Sender<std::result::Result<Response, &'static str>>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if TEST_READER_FAILURE.replace(false) {
        return Err(std::io::Error::from_raw_os_error(libc::EAGAIN));
    }
    std::thread::Builder::new()
        .name("web-response".into())
        .spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let response = match Read::take(&mut reader, OUTPUT_LIMIT + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(_) if line.len() as u64 <= OUTPUT_LIMIT && line.last() == Some(&b'\n') => {
                        serde_json::from_slice(&line).map_err(|_| "invalid Web runtime response")
                    }
                    _ => Err("Web runtime response exceeds its protocol limit"),
                };
                let failed = response.is_err();
                if tx.send(response).is_err() || failed {
                    break;
                }
            }
        })
}
fn stop_child(child: &mut Child) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let _ = child.kill();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                return Err(crate::protocol::failure_details(
                    "web_runtime_teardown_failed",
                    None,
                    "Web worker termination could not be verified",
                    serde_json::json!({"process_id":child.id(),"preserved":true}),
                ));
            }
        }
    }
}
fn request_deadline(cancelled: &AtomicBool, deadline: Instant) -> Result<()> {
    check(cancelled)?;
    if Instant::now() >= deadline {
        return Err(crate::protocol::failure(
            "web_runtime_timeout",
            Some(true),
            "Web runtime request deadline exceeded",
        ));
    }
    Ok(())
}
#[cfg(unix)]
fn deliver(
    input: &mut ChildStdin,
    mut bytes: &[u8],
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<()> {
    crate::process::nonblocking(input)?;
    while !bytes.is_empty() {
        request_deadline(cancelled, deadline)?;
        match input.write(bytes) {
            Ok(0) => {
                return Err(crate::protocol::failure(
                    "web_runtime_input_failed",
                    Some(true),
                    "Web runtime input closed",
                ));
            }
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => {
                return Err(crate::protocol::failure(
                    "web_runtime_input_failed",
                    Some(true),
                    "Web runtime input failed",
                ));
            }
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn deliver(_: &mut ChildStdin, _: &[u8], _: &AtomicBool, _: Instant) -> Result<()> {
    Err(crate::protocol::failure(
        "web_runtime_unsupported",
        Some(false),
        "Bounded Web IPC requires Unix",
    ))
}
fn valid_solution(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 4096
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

/// Run only in the executable's private subprocess, before installing CLI signals.
#[allow(unsafe_code)] // Audited OS boundary; keep unsafe operations local.
pub(crate) fn run_worker() -> Result<()> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // Only this child lowers its own address-space soft and hard limits.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_AS, &mut limit) != 0 {
                return Err("cannot inspect Web runtime memory limit".into());
            }
            limit.rlim_max = limit.rlim_max.min(4 * 1024 * 1024 * 1024);
            limit.rlim_cur = limit.rlim_cur.min(limit.rlim_max);
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err("cannot bound Web runtime memory".into());
            }
        }
    }
    let worker = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| worker_loop().map_err(|_| "Web runtime failed"))?;
    worker
        .join()
        .map_err(|_| "Web runtime panicked")?
        .map_err(Into::into)
}
fn worker_loop() -> Result<()> {
    let mut runtime = runtime::Runtime::default();
    let started = Instant::now();
    let active = std::sync::Arc::new(AtomicU64::new(0));
    let monitor = active.clone();
    std::thread::Builder::new()
        .name("web-deadline".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(25));
                let request = monitor.load(Ordering::SeqCst);
                if request != 0
                    && started.elapsed().as_millis().saturating_add(1)
                        >= u128::from(request) + 90_000
                    && monitor.load(Ordering::SeqCst) == request
                {
                    std::process::exit(124);
                }
            }
        })?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = Vec::new();
        let size = Read::take(&mut input, INPUT_LIMIT + 1).read_until(b'\n', &mut line)?;
        if size == 0 {
            return Ok(());
        }
        if size as u64 > INPUT_LIMIT || line.last() != Some(&b'\n') {
            return Err("invalid Web runtime request length".into());
        }
        let request: Request =
            serde_json::from_slice(&line).map_err(|_| "invalid Web runtime request")?;
        active.store(started.elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
        let mint = matches!(request, Request::Mint { .. });
        let response = runtime.handle(request).unwrap_or_else(|error| Response {
            token: Vec::new(),
            expires_in: 0,
            solution: String::new(),
            failed: true,
            failure: Some(WorkerFailure::capture(error.as_ref(), mint)),
        });
        active.store(0, Ordering::SeqCst);
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn response_reader_resource_failure_stops_the_started_worker() {
        TEST_READER_FAILURE.set(true);
        let error = Worker::start().err().unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "resource_limit");
        assert_eq!(failure.details()["stage"], "web_response_thread");
        assert_eq!(
            unsafe { libc::kill(TEST_STARTED_CHILD.get() as i32, 0) },
            -1
        );
    }
    #[cfg(unix)]
    #[test]
    fn request_deadline_includes_a_stalled_pipe_writer() {
        let (mut worker, pid) = Worker::idle_fixture();
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
        let completed = std::sync::Arc::new(AtomicBool::new(false));
        let observed = completed.clone();
        let rescue = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            if !observed.load(Ordering::SeqCst) {
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
            }
        });
        TEST_REQUEST_TIMEOUT.set(Some(Duration::from_millis(80)));
        let started = Instant::now();
        let result = worker.request(
            &Request::N {
                player: "x".repeat(2 * 1024 * 1024),
                challenge: "abc".into(),
            },
            &AtomicBool::new(false),
        );
        let elapsed = started.elapsed();
        completed.store(true, Ordering::SeqCst);
        TEST_REQUEST_TIMEOUT.set(None);
        drop(worker);
        rescue.join().unwrap();
        assert!(elapsed < Duration::from_millis(300), "{elapsed:?}");
        assert_eq!(
            result
                .err()
                .unwrap()
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "web_runtime_timeout"
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
    #[cfg(unix)]
    #[test]
    fn stalled_delivery_is_cancellable_and_closed_pipes_are_typed() {
        let (mut worker, pid) = Worker::idle_fixture();
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let task = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            signal.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = worker.request(
            &Request::N {
                player: "x".repeat(2 * 1024 * 1024),
                challenge: "abc".into(),
            },
            &cancelled,
        );
        task.join().unwrap();
        worker.close().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(result.err().unwrap().to_string().contains("interrupted"));
        let (mut worker, _) = Worker::idle_fixture();
        worker.child.kill().unwrap();
        worker.child.wait().unwrap();
        let result = worker.request(
            &Request::Mint {
                video: "a2hbJlPxj4M".into(),
                refresh: false,
            },
            &AtomicBool::new(false),
        );
        assert_eq!(
            result
                .err()
                .unwrap()
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "web_runtime_input_failed"
        );
    }
    #[cfg(unix)]
    #[test]
    fn typed_http_failure_survives_ipc_without_script_or_body_data() {
        for (code, status, retryable) in [
            ("youtube_rate_limited", 429, true),
            ("youtube_temporary_failure", 502, true),
            ("youtube_http_error", 403, false),
        ] {
            let error = crate::protocol::failure_details(
                code,
                Some(retryable),
                "secret-token-and-url",
                serde_json::json!({"http_status":status,"retry_after":"120","retry_after_seconds":120,"body":"secret-body"}),
            );
            let failure = WorkerFailure::capture(error.as_ref(), true);
            let encoded = serde_json::to_vec(&failure).unwrap();
            assert!(!String::from_utf8_lossy(&encoded).contains("secret"));
            let decoded = serde_json::from_slice(&encoded).unwrap();
            let (mut worker, _) = Worker::idle_fixture();
            let (sender, receiver) = mpsc::channel();
            worker.responses = receiver;
            sender
                .send(Ok(Response {
                    token: vec![],
                    expires_in: 0,
                    solution: String::new(),
                    failed: true,
                    failure: Some(decoded),
                }))
                .unwrap();
            let error = worker
                .request(
                    &Request::Mint {
                        video: "a2hbJlPxj4M".into(),
                        refresh: false,
                    },
                    &AtomicBool::new(false),
                )
                .err()
                .unwrap();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, code);
            assert_eq!(failure.retryable, Some(retryable));
            assert_eq!(failure.http_status(), Some(status));
            assert_eq!(failure.retry_after_seconds(), Some(120));
            assert!(!error.to_string().contains("secret"));
        }
        let error: Box<dyn std::error::Error + Send + Sync> = "secret script exception".into();
        let failure = WorkerFailure::capture(error.as_ref(), true).into_error();
        assert_eq!(
            failure
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "youtube_attestation_failed"
        );
        assert!(!failure.to_string().contains("secret"));
    }
    #[test]
    fn rejects_protocol_extensions_and_invalid_solutions() {
        assert!(
            serde_json::from_str::<Request>(
                r#"{"operation":"Mint","video":"a2hbJlPxj4M","refresh":false,"cookie":"x"}"#
            )
            .is_err()
        );
        assert!(valid_solution("a-b_C123"));
        for s in ["", "a&pot=leak", "../path", "a%26"] {
            assert!(!valid_solution(s));
        }
    }
}
