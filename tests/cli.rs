use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn help_exposes_metadata_only_chapters_command() {
    let output = Fixture::new().command().arg("--help").output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["result"]["help"]
            .as_str()
            .unwrap()
            .contains("agent-youtube chapters URL")
    );
}

#[test]
fn chapters_rejects_job_options_and_bad_urls_before_network_access() {
    for arguments in [
        vec!["chapters"],
        vec!["chapters", "file:///tmp/input"],
        vec![
            "chapters",
            "https://youtu.be/a2hbJlPxj4M",
            "--work-dir",
            "unused",
        ],
        vec!["chapters", "https://youtu.be/a2hbJlPxj4M", "--force"],
    ] {
        let fixture = Fixture::new();
        let output = fixture.command().args(arguments).output().unwrap();
        assert!(!output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_arguments");
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
    }
}

#[test]
fn public_help_uses_independent_application_identity() {
    let output = Fixture::new().command().arg("--help").output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = value.to_string();
    assert!(text.contains("agent-youtube transcribe"));
}

#[cfg(unix)]
#[test]
fn unavailable_recognizer_has_structured_spawn_cause() {
    use std::os::unix::fs::PermissionsExt;
    for reason in ["not_found", "permission_denied"] {
        let fixture = Fixture::new();
        let program = fixture.0.join("recognizer");
        if reason == "permission_denied" {
            fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let output = fixture
            .command()
            .args(["doctor", "--work-dir", "work", "--gigastt"])
            .arg(&program)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["error"]["code"], "process_spawn_failed");
        assert_eq!(result["error"]["retryable"], false);
        assert_eq!(result["error"]["details"]["reason"], reason);
        assert_eq!(
            result["error"]["details"]["program"],
            program.to_str().unwrap()
        );
        assert!(result["error"]["details"]["os_error"].is_number());
        assert!(!fixture.0.join("work/transcript.json").exists());
        for line in String::from_utf8(output.stderr).unwrap().lines() {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(event["event"].is_string());
        }
    }
}

#[test]
fn proxy_parent_fixture() {
    if std::env::var_os("AGENT_YOUTUBE_PROXY_FIXTURE").is_none() {
        return;
    }
    let _ = agent_youtube::download::Downloader::youtube()
        .unwrap()
        .metadata("of4B3aw7MXo", &std::sync::atomic::AtomicBool::new(false));
}

#[test]
fn parent_and_attestation_worker_use_the_configured_proxy() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let (seen, requests_seen) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut requests = Vec::new();
        while requests.len() < 2 && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    requests.push(first);
                    let _ = seen.send(());
                    stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("proxy listener failed: {error}"),
            }
        }
        requests
    });
    let route = |command: &mut Command| {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            command.env_remove(key);
        }
        command
            .env("ALL_PROXY", &proxy)
            .env("XDG_CACHE_HOME", &fixture.0);
    };
    let mut parent = Command::new(std::env::current_exe().unwrap());
    parent
        .args(["--exact", "proxy_parent_fixture"])
        .env("AGENT_YOUTUBE_PROXY_FIXTURE", "1");
    route(&mut parent);
    parent.stdout(Stdio::null()).stderr(Stdio::null());
    let mut parent = parent.spawn().unwrap();
    let admitted = requests_seen.recv_timeout(Duration::from_secs(5));
    // Stop the owned fixture before its normal transient-network retry delay.
    let _ = parent.kill();
    parent.wait().unwrap();
    admitted.unwrap();
    let mut worker = fixture.command();
    worker
        .arg("--internal-web-runtime")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    route(&mut worker);
    let mut worker = worker.spawn().unwrap();
    worker
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"operation\":\"Mint\",\"video\":\"of4B3aw7MXo\",\"refresh\":true}\n")
        .unwrap();
    let worker = worker.wait_with_output().unwrap();
    assert!(worker.status.success());
    let response: serde_json::Value = serde_json::from_slice(&worker.stdout).unwrap();
    assert_eq!(response["failed"], true);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|line| line == "CONNECT www.youtube.com:443 HTTP/1.1\r\n"),
        "{requests:?}"
    );
}

#[test]
fn json_export_does_not_invent_legacy_coverage() {
    let fixture = Fixture::new();
    fixture.input("input.json", 0.0, 1.0);
    let result = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "json",
            "--output",
            "out.json",
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.0.join("out.json")).unwrap()).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["coverage"]["status"], "unavailable");
    assert_eq!(value["passages"][0]["text"], "hello");
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "agent-youtube-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn input(&self, name: &str, start: f64, end: f64) {
        let transcript = serde_json::json!({
            "schema_version": 1,
            "source": {"url": "https://youtu.be/a2hbJlPxj4M", "video_id": "a2hbJlPxj4M", "title": null},
            "language": "ru", "duration": end,
            "engine": {"name": "fixture", "version": "1", "model_variant": "test"},
            "segments": [{"start": start, "end": end, "text": "hello", "timing": "chunk", "words": [], "speaker": null}]
        });
        fs::write(self.0.join(name), serde_json::to_vec(&transcript).unwrap()).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-youtube"));
        command.current_dir(&self.0);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fifo_transcript_fails_promptly_and_preserves_existing_export() {
    use std::{
        os::unix::ffi::OsStrExt,
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let fifo = fixture.0.join("input.json");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    fs::write(fixture.0.join("out.json"), b"owned by user").unwrap();
    let mut child = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "json",
            "--output",
            "out.json",
            "--force",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!timed_out, "export blocked opening a FIFO transcript");
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "export_failed");
    assert_eq!(
        fs::read(fixture.0.join("out.json")).unwrap(),
        b"owned by user"
    );
}

#[test]
fn help_option_value_is_a_literal_output_path() {
    let fixture = Fixture::new();
    fixture.input("input.json", 0.0, 1.0);
    let output = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "txt",
            "--output",
            "--help",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(fixture.0.join("--help")).unwrap(),
        "[00:00:00.000] hello\n"
    );
}

#[test]
fn positional_delimiter_allows_a_transcript_named_help() {
    let fixture = Fixture::new();
    fixture.input("--help", 0.0, 1.0);
    let output = fixture
        .command()
        .args([
            "export", "--format", "txt", "--output", "out.txt", "--", "--help",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(fixture.0.join("out.txt").is_file());
}

#[test]
fn genuine_help_requests_print_usage() {
    let fixture = Fixture::new();
    for args in [
        vec!["--help"],
        vec!["export", "--help"],
        vec!["export", "-h"],
    ] {
        let output = fixture.command().args(args).output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
}

#[test]
fn overflowing_subtitle_timestamps_publish_no_output() {
    let fixture = Fixture::new();
    fixture.input("input.json", 1e20, 2e20);
    let output = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "srt",
            "--output",
            "out.srt",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.0.join("out.srt").exists());
}

#[test]
fn late_streamed_subtitle_failure_never_replaces_an_existing_export() {
    let fixture = Fixture::new();
    fixture.input("input.json", 0.0, 2.0);
    let path = fixture.0.join("input.json");
    let mut t: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    t["segments"][0]["end"] = serde_json::json!(1.0);
    let mut last = t["segments"][0].clone();
    last["start"] = serde_json::json!(2.0);
    last["end"] = serde_json::json!(2.0);
    t["segments"].as_array_mut().unwrap().push(last);
    fs::write(&path, serde_json::to_vec(&t).unwrap()).unwrap();
    fs::write(fixture.0.join("out.srt"), b"owner data").unwrap();
    let result = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "srt",
            "--output",
            "out.srt",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(fixture.0.join("out.srt")).unwrap(), b"owner data");
    assert!(!fs::read_dir(&fixture.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".agent-youtube-tmp-")
    }));
}

#[test]
fn private_web_worker_solves_requests_without_network_or_external_runtimes() {
    use std::io::Write;
    use std::process::Stdio;
    let source = r#"var namespace={};(function(g){var window=this;function U(){this.values={}}U.prototype.set=function(k,v){this.values[k]=v};U.prototype.get=function(k){return this.values[k]};U.prototype.transform=function(){this.values.n+='x'};function C(){var u=new U;u.set('alr','yes');return u}unwantedStartup();})(namespace);"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .arg("--internal-web-runtime")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for challenge in ["abc", "def"] {
        writeln!(
            input,
            "{}",
            serde_json::json!({"operation":"N","player":source,"challenge":challenge})
        )
        .unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["solution"], "abcx");
    assert_eq!(lines[1]["solution"], "defx");
    assert_eq!(lines[0]["failed"], false);
}

#[test]
fn private_worker_observes_another_process_cooldown_before_attestation() {
    use std::{
        io::Write,
        process::Stdio,
        time::{SystemTime, UNIX_EPOCH},
    };
    let fixture = Fixture::new();
    let cache = fixture.0.join("cache");
    let gate = cache.join("agent-youtube");
    fs::create_dir_all(&gate).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gate, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    fs::write(
        gate.join("state.json"),
        serde_json::json!({"next_source_ms":0,"cooldown_until_ms":now+120000}).to_string(),
    )
    .unwrap();
    let mut child = fixture
        .command()
        .arg("--internal-web-runtime")
        .env("XDG_CACHE_HOME", &cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"operation":"Mint","video":"a2hbJlPxj4M","refresh":true})
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["failure"]["code"], "youtube_rate_limited");
    assert_eq!(response["failure"]["retryable"], true);
    assert!(response["failure"]["retry_after_seconds"].as_u64().unwrap() >= 110);
    assert!(response.get("token").is_none());
}

#[test]
fn public_cli_always_returns_versioned_json() {
    let fixture = Fixture::new();
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["unknown"],
        vec!["transcribe"],
        vec!["doctor", "--unknown", "x"],
    ] {
        let output = fixture.command().args(&args).output().unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}; {:?}", output));
        assert_eq!(result["schema_version"], 1);
        assert_eq!(result["ok"], output.status.success());
        if !output.status.success() {
            assert_eq!(result["error"]["code"], "invalid_arguments");
        }
        for line in String::from_utf8(output.stderr).unwrap().lines() {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }
    fixture.input("input.json", 0.0, 1.0);
    let output = fixture
        .command()
        .args([
            "export",
            "input.json",
            "--format",
            "txt",
            "--output",
            "out.txt",
        ])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["command"], "export");
    assert_eq!(result["result"]["output"], "out.txt");
}

#[cfg(target_os = "linux")]
#[test]
fn oversized_transcript_returns_json_under_process_memory_limit() {
    use std::{
        io::{Read, Write},
        os::unix::process::CommandExt,
    };
    let fixture = Fixture::new();
    fixture.input("large.json", 0.0, 1.0);
    let bytes = fs::read(fixture.0.join("large.json")).unwrap();
    let offset = bytes.windows(5).position(|part| part == b"hello").unwrap();
    let mut file = fs::File::create(fixture.0.join("large.json")).unwrap();
    file.write_all(&bytes[..offset]).unwrap();
    std::io::copy(&mut std::io::repeat(b'x').take(96 * 1024 * 1024), &mut file).unwrap();
    file.write_all(&bytes[offset + 5..]).unwrap();
    drop(file);
    let mut command = fixture.command();
    command.args([
        "export",
        "large.json",
        "--format",
        "txt",
        "--output",
        "large.txt",
    ]);
    // Only the child changes its address-space limit; no allocation in pre_exec.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 256 * 1024 * 1024,
                rlim_max: 256 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["code"], "resource_limit");
    assert_eq!(json["error"]["details"]["preserved"], true);
    assert!(!fixture.0.join("large.txt").exists());
    assert_eq!(
        fs::metadata(fixture.0.join("large.json")).unwrap().len(),
        (bytes.len() - 5 + 96 * 1024 * 1024) as u64
    );
}

#[test]
fn setup_reports_missing_models_without_publishing_a_runtime() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "setup",
            "--runtime-dir",
            "runtime",
            "--model-dir",
            "absent-models",
            "--archive",
            "absent.tar.gz",
        ])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!output.status.success());
    assert_eq!(result["error"]["code"], "setup_models_invalid");
    assert_eq!(result["error"]["retryable"], false);
    assert!(!fixture.0.join("runtime/runtime.json").exists());
}

#[test]
fn incomplete_default_runtime_never_falls_back_to_path() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("tools-local/runtime")).unwrap();
    let output = fixture
        .command()
        .args(["doctor", "--work-dir", "work"])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "runtime_invalid");
    assert!(!fixture.0.join("work").exists());
}

#[cfg(unix)]
#[test]
fn saved_resume_ignores_broken_default_runtime() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("tools-local/runtime")).unwrap();
    let job = fixture.0.join("job");
    fs::create_dir(&job).unwrap();
    let options = agent_youtube::pipeline::Options {
        url: "https://www.youtube.com/watch?v=of4B3aw7MXo".into(),
        work_dir: job.clone(),
        gigastt: "/bin/false".into(),
        model_dir: fixture.0.join("models"),
        model_variant: Default::default(),
        chunk_seconds: 30,
        asr_timeout_seconds: 30,
        boundary_window_ms: 0,
        asr_memory_mib: None,
        encoder_threads: None,
        retry_window_seconds: 0,
    };
    fs::write(
        job.join("job.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"working_directory":fixture.0,"options":options
        }))
        .unwrap(),
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["resume", "--work-dir", "job"])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "process_failed");
    assert!(!fixture.0.join("tools-local/runtime/runtime.json").exists());
}

#[cfg(unix)]
#[test]
fn explicit_recognizer_bypasses_broken_default_runtime() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("tools-local/runtime")).unwrap();
    let output = fixture
        .command()
        .args(["doctor", "--gigastt", "/bin/false"])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "process_failed");
}

#[test]
fn offline_setup_in_an_empty_directory_reports_missing_assets_without_network() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["setup", "--runtime-dir", "runtime", "--offline"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "setup_offline_missing");
    assert!(!fixture.0.join("runtime/runtime.json").exists());
    for line in String::from_utf8(output.stderr).unwrap().lines() {
        let _: serde_json::Value = serde_json::from_str(line).unwrap();
    }
}
