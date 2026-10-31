#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
#[path = "support/contract.rs"]
mod contract;
#[path = "support/fmp4.rs"]
mod fmp4;
const URL: &str = "https://www.youtube.com/watch?v=a2hbJlPxj4M";

#[test]
fn emitted_results_events_and_artifacts_match_the_machine_contract() {
    let fixture = Fixture::new();
    let mut documents = Vec::new();
    let output = fixture
        .run_command(URL)
        .env("FAKE_RETRY_COOLDOWN", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    contract::collect(&output, &mut documents);
    for mode in ["compact", "metadata", "full"] {
        let output = fixture
            .run_command(URL)
            .args(["--response", mode])
            .output()
            .unwrap();
        assert!(output.status.success());
        contract::collect(&output, &mut documents);
    }
    for command in ["status", "cancel"] {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .args([command, "--work-dir"])
            .arg(fixture.path(""))
            .output()
            .unwrap();
        assert!(output.status.success());
        contract::collect(&output, &mut documents);
    }
    for command in ["--help", "--version", "unknown-command"] {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .arg(command)
            .output()
            .unwrap();
        contract::collect(&output, &mut documents);
    }
    for name in [
        "transcript.json",
        "manifest.json",
        "coverage.json",
        "raw/000000.json",
    ] {
        documents.push(serde_json::from_slice(&fs::read(fixture.path(name)).unwrap()).unwrap());
    }
    let exported = fixture.root.join("export.json");
    let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .arg("export")
        .arg(fixture.path("transcript.json"))
        .args(["--format", "json", "--output"])
        .arg(&exported)
        .output()
        .unwrap();
    assert!(output.status.success());
    contract::collect(&output, &mut documents);
    documents.push(serde_json::from_slice(&fs::read(exported).unwrap()).unwrap());
    let expired = Fixture::new();
    let output = expired
        .run_command(URL)
        .args(["--retry-window-seconds", "1"])
        .env("FAKE_RETRY_COOLDOWN", "10")
        .output()
        .unwrap();
    assert!(!output.status.success());
    contract::collect(&output, &mut documents);
    let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["status", "--work-dir"])
        .arg(expired.path(""))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["result"]["state"],
        "deadline_exceeded"
    );
    contract::collect(&output, &mut documents);
    contract::validate(&documents);
}

struct Fixture {
    root: PathBuf,
    preserve: bool,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-pipeline-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self::at(root, false)
    }
    fn at(root: PathBuf, preserve: bool) -> Self {
        fs::create_dir_all(root.join("models")).unwrap();
        for name in [
            "v3_e2e_rnnt_encoder_int8.onnx",
            "v3_e2e_rnnt_decoder.onnx",
            "v3_e2e_rnnt_joint.onnx",
            "v3_e2e_rnnt_vocab.txt",
        ] {
            fs::write(root.join("models").join(name), b"fixture model").unwrap();
        }
        let path = root.join("gigastt");
        fs::write(&path, FAKE_TOOL).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        static AAC: OnceLock<Vec<u8>> = OnceLock::new();
        let aac = AAC.get_or_init(|| aac_fixture(48_000, 12 * 48_000));
        fs::write(root.join("input.m4a"), aac).unwrap();
        let wrapper = root.join("agent-youtube-test");
        fs::write(&wrapper, format!("#!/usr/bin/python3\nimport os,sys,json\nos.environ['AGENT_YOUTUBE_TEST_ARGS']=json.dumps(sys.argv[1:])\nos.execv({0:?},[{0:?},'--exact','pipeline_cli_fixture','--nocapture'])\n", std::env::current_exe().unwrap().to_str().unwrap())).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, preserve }
    }

    fn run_command(&self, url: &str) -> Command {
        self.run_command_in(url, &self.root.join("run"))
    }

    fn run_command_in(&self, url: &str, work_dir: &Path) -> Command {
        // Read the fresh fixture as Python input instead of execve-ing its inode.
        // Some filesystems transiently return ETXTBSY immediately after creation.
        let mut command = Command::new("/usr/bin/python3");
        command.arg(self.root.join("agent-youtube-test"));
        command
            .env("AGENT_YOUTUBE_TEST_ROOT", &self.root)
            .arg("transcribe")
            .arg(url)
            .arg("--work-dir")
            .arg(work_dir)
            .arg("--model-dir")
            .arg(self.root.join("models"))
            .args(["--chunk-seconds", "10"]);
        command.arg("--gigastt").arg(self.root.join("gigastt"));
        command
    }

    fn run(&self) -> Output {
        self.run_command(URL).output().unwrap()
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join("run").join(path)
    }

    fn calls(&self, tool: &str, argument: &str) -> Vec<serde_json::Value> {
        fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|call| {
                call["tool"] == tool
                    && call["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|arg| arg == argument)
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn failure(output: Output, message: &str) {
    assert!(!output.status.success(), "unexpected successful command");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "expected {message:?}, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn failure_code(output: &Output, code: &str, path: &Path) {
    assert!(!output.status.success());
    let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
    let record: serde_json::Value = serde_json::from_slice(&output.stdout[start..]).unwrap();
    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["ok"], false);
    assert_eq!(record["error"]["code"], code);
    assert_eq!(record["error"]["retryable"], false);
    assert_eq!(
        record["error"]["details"]["path"],
        path.to_string_lossy().as_ref()
    );
    assert_eq!(record["error"]["details"]["preserved"], true);
}

fn export(input: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .arg("export")
        .arg(input)
        .args(["--format", "txt", "--output"])
        .arg(output)
        .env("PATH", "/nonexistent-agent-youtube-test-path")
        .output()
        .unwrap()
}

#[test]
fn full_pipeline_removes_media_but_keeps_raw_words_and_exports_without_any_tools() {
    let fixture = Fixture::new();
    success(fixture.run());
    let transcript: agent_youtube::transcript::Transcript =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    transcript.validate().unwrap();
    assert_eq!(transcript.source.video_id, "a2hbJlPxj4M");
    assert_eq!(transcript.source.title.as_deref(), Some("Русская запись"));
    assert_eq!(transcript.duration, 12.0);
    assert_eq!(transcript.segments.len(), 2);
    assert_eq!(transcript.segments[1].start, 10.0);
    assert_eq!(transcript.segments[1].text, "Привет, мир!");
    assert_eq!(transcript.segments[1].words[0].speaker, Some(0));
    assert_eq!(transcript.segments[1].words[0].confidence, Some(0.9));
    for path in ["raw/000000.json", "raw/000001.json"] {
        assert!(fixture.path(path).is_file(), "missing {path}");
    }
    for path in [
        "media/source.audio",
        "media/video.mkv",
        "media/audio.wav",
        "chunks/000000.wav",
        "chunks/000001.wav",
        "media/download.webm",
    ] {
        assert!(
            !fixture.path(path).exists(),
            "retained temporary media: {path}"
        );
    }
    for directory in ["media", "chunks"] {
        assert!(
            fs::read_dir(fixture.path(directory))
                .unwrap()
                .next()
                .is_none(),
            "retained files in {directory}"
        );
    }
    assert!(fixture.path("source.json").is_file());
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert!(
        state["artifacts"]
            .as_object()
            .unwrap()
            .keys()
            .all(|name| !name.starts_with("media/") && !name.starts_with("chunks/"))
    );
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("raw/000000.json")).unwrap()).unwrap();
    assert_eq!(raw["upstream_extra"], "retain me");
    let output = fixture.root.join("transcript.txt");
    success(export(&fixture.path("transcript.json"), &output));
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        "[00:00:00.000] Привет, мир!\n[00:00:10.000] Привет, мир!\n"
    );
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    let download = fixture.calls("http", "/audio");
    assert_eq!(download.len(), 1);
    assert_eq!(
        download[0]["range"],
        format!(
            "bytes=0-{}",
            fs::metadata(fixture.root.join("input.m4a")).unwrap().len() - 1
        )
    );
}

#[test]
fn agent_response_is_compact_versioned_and_legacy_is_explicit() {
    let fixture = Fixture::new();
    let result = fixture.run();
    assert!(result.status.success());
    let begin = result.stdout.iter().position(|b| *b == b'{').unwrap();
    let record: serde_json::Value = serde_json::Deserializer::from_slice(&result.stdout[begin..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(record["schema_version"], 2);
    let view = &record["result"]["transcript"];
    let events: Vec<serde_json::Value> = String::from_utf8(result.stderr.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .filter(|v| v["event"] == "progress")
            .all(|v| v["stage"].is_string())
    );
    assert!(
        events.iter().any(|v| v["stage"] == "recognize"
            && v["durable"] == true
            && v["completed_chunks"] == 2)
    );
    assert!(
        events
            .iter()
            .any(|v| v["stage"] == "download" && v["downloaded_bytes"].is_number())
    );
    assert_eq!(view["coverage"]["status"], "complete");
    assert_eq!(view["coverage"]["scope"], "audio_processed");
    assert_eq!(view["speech_coverage"]["status"], "unverified");
    assert_eq!(view["speech_coverage"]["method"], "asr_timestamp_gaps");
    assert_eq!(view["coverage"]["frames"], 192000);
    assert_eq!(view["recognizer_language"], "ru");
    assert_eq!(view["source_language"], serde_json::Value::Null);
    assert_eq!(view["accuracy"], "unverified");
    assert!(view.get("segments").is_none());
    assert_eq!(view["passages"][1]["text"], "Привет, мир!");
    let original = fs::read(fixture.path("transcript.json")).unwrap();
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let result = fixture
        .run_command(URL)
        .args(["--response", "full"])
        .output()
        .unwrap();
    let begin = result.stdout.iter().position(|b| *b == b'{').unwrap();
    let record: serde_json::Value = serde_json::Deserializer::from_slice(&result.stdout[begin..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(record["schema_version"], 1);
    assert_eq!(
        record["result"]["transcript"],
        serde_json::from_slice::<serde_json::Value>(&original).unwrap()
    );
    let result = fixture
        .run_command(URL)
        .args(["--response", "metadata"])
        .output()
        .unwrap();
    let begin = result.stdout.iter().position(|b| *b == b'{').unwrap();
    let record: serde_json::Value = serde_json::Deserializer::from_slice(&result.stdout[begin..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        record["result"]["transcript"]["coverage"]["status"],
        "complete"
    );
    assert!(record["result"]["transcript"].get("passages").is_none());
    assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
}

#[test]
fn doctor_reports_preflight_scope_and_rejects_missing_invoked_flags() {
    let fixture = Fixture::new();
    let run = |missing: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agent-youtube"));
        cmd.arg("doctor")
            .arg("--work-dir")
            .arg(fixture.root.join("doctor"))
            .arg("--model-dir")
            .arg(fixture.root.join("models"))
            .arg("--gigastt")
            .arg(fixture.root.join("gigastt"))
            .args(["--encoder-threads", "1"]);
        if let Some(flag) = missing {
            cmd.env("FAKE_MISSING_FLAG", flag);
        }
        cmd.output().unwrap()
    };
    let good = run(None);
    assert!(good.status.success(), "{:?}", good);
    let value: serde_json::Value = serde_json::from_slice(&good.stdout).unwrap();
    assert_eq!(value["result"]["verified"], false);
    assert_eq!(value["result"]["preflight_passed"], true);
    assert_eq!(value["result"]["checks"]["inference"], "not_checked");
    for flag in [
        "--punctuation",
        "--itn",
        "--log-level",
        "--model-dir",
        "--encoder-intra-threads",
    ] {
        let failed = run(Some(flag));
        assert!(!failed.status.success(), "accepted missing {flag}");
    }
}

#[test]
fn late_cancellation_finishes_one_committed_response() {
    use std::io::Read;
    use std::process::Stdio;
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .env("FAKE_TEXT_SIZE", "262144")
            .output()
            .unwrap(),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .arg("transcribe")
        .arg(URL)
        .arg("--work-dir")
        .arg(fixture.path(""))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut prefix = [0; 32];
    stdout.read_exact(&mut prefix).unwrap();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let mut bytes = prefix.to_vec();
    stdout.read_to_end(&mut bytes).unwrap();
    assert!(child.wait().unwrap().success());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["result"]["transcript"]["passages"][1]["text"]
            .as_str()
            .unwrap()
            .len(),
        262144
    );
}

#[test]
#[ignore = "opt-in real-media duration gate qualification with retained research fixtures"]
fn research_real_media_duration_gate() {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let digest = |path: &Path| {
        let mut input = fs::File::open(path).unwrap();
        let mut hash = Sha256::new();
        let mut bytes = [0; 65536];
        loop {
            let count = input.read(&mut bytes).unwrap();
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        format!("{:x}", hash.finalize())
    };
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_DURATION_PROBE_DIR").unwrap());
    assert!(!directory.exists());
    assert!(
        directory
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .starts_with(
                std::env::current_dir()
                    .unwrap()
                    .join("runs")
                    .canonicalize()
                    .unwrap()
            )
    );
    let media = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_DURATION_MEDIA").unwrap())
        .canonicalize()
        .unwrap();
    let seconds: u64 = std::env::var("AGENT_YOUTUBE_DURATION_SECONDS")
        .unwrap()
        .parse()
        .unwrap();
    let id = std::env::var("AGENT_YOUTUBE_DURATION_VIDEO").unwrap();
    let url = format!("https://www.youtube.com/watch?v={id}");
    fs::create_dir(&directory).unwrap();
    let mut cases = Vec::new();
    for mismatch in [false, true] {
        let fixture = Fixture::at(
            directory.join(if mismatch { "mismatch" } else { "valid" }),
            true,
        );
        let expected = seconds + if mismatch { 7 } else { 0 };
        let output = fixture
            .run_command(&url)
            .env("FAKE_MEDIA", &media)
            .env("FAKE_VIDEO_ID", &id)
            .env("FAKE_DURATION", expected.to_string())
            .output()
            .unwrap();
        if mismatch {
            assert!(!output.status.success());
            let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
            let record: serde_json::Value =
                serde_json::from_slice(&output.stdout[start..]).unwrap();
            assert_eq!(record["error"]["code"], "audio_duration_mismatch");
            assert!(fixture.calls("gigastt", "--output").is_empty());
            assert_eq!(digest(&fixture.path("media/source.audio")), digest(&media));
            cases.push(serde_json::json!({"scenario":"declared duration mismatch","error":record["error"],"media_retained":true,"asr_invocations":0}));
        } else {
            success(output);
            let transcript: agent_youtube::transcript::Transcript =
                serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap())
                    .unwrap();
            assert!((transcript.duration - seconds as f64).abs() <= 2.0);
            cases.push(serde_json::json!({"scenario":"matching duration","decoded_seconds":transcript.duration,"transcript_published":true,"cleanup_complete":!fixture.path("media/source.audio").exists(),"asr_simulated":true}));
        }
    }
    let report = serde_json::json!({"application_version":env!("CARGO_PKG_VERSION"),"source_video_id":id,"source_sha256":digest(&media),"real_media_injected_through_loopback_fixture":true,"real_speech_recognition":false,"cases":cases});
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn declared_duration_mismatch_preserves_audio_without_recognition_or_publication() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .env("FAKE_DURATION", "60")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "short complete container was accepted: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout[start..]).unwrap();
    assert_eq!(value["error"]["code"], "audio_duration_mismatch");
    assert_eq!(value["error"]["details"]["expected_seconds"], 60);
    assert!(fixture.path("media/source.audio").is_file());
    assert!(!fixture.path("transcript.json").exists());
    assert!(fixture.calls("gigastt", "--output").is_empty());
    let before = fs::read(fixture.path("media/source.audio")).unwrap();
    failure(fixture.run(), "duration");
    assert_eq!(
        fs::read(fixture.path("media/source.audio")).unwrap(),
        before
    );
    assert!(fixture.calls("gigastt", "--output").is_empty());
    // A legacy full-WAV checkpoint must not bypass the same coverage gate.
    use sha2::{Digest, Sha256};
    let wav = ryf::encode_s16(&vec![0; 12 * 16000 * 2], 16000).unwrap();
    fs::write(fixture.path("media/audio.wav"), &wav).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    state["schema_version"] = 1.into();
    state["journal"] = serde_json::Value::Null;
    state["artifacts"]["media/audio.wav"] = format!("{:x}", Sha256::digest(&wav)).into();
    fs::write(
        fixture.path("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    failure(fixture.run(), "duration");
    assert_eq!(fs::read(fixture.path("media/audio.wav")).unwrap(), wav);
    assert!(fixture.calls("gigastt", "--output").is_empty());
}

#[test]
fn completed_job_duration_mismatch_never_authorizes_status_or_cleanup() {
    use sha2::{Digest, Sha256};
    for command in ["transcribe", "status", "recover_pending"] {
        let fixture = Fixture::new();
        success(fixture.run());
        let original = fs::read(fixture.path("transcript.json")).unwrap();
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("source.json")).unwrap()).unwrap();
        metadata["videoDetails"]["lengthSeconds"] = "60".into();
        let metadata = serde_json::to_vec(&metadata).unwrap();
        fs::write(fixture.path("source.json"), &metadata).unwrap();
        let mut download: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("download.json")).unwrap()).unwrap();
        download.as_object_mut().unwrap().remove("duration_seconds");
        let download = serde_json::to_vec(&download).unwrap();
        fs::write(fixture.path("download.json"), &download).unwrap();
        let media = b"owned media awaiting cleanup";
        fs::write(fixture.path("media/source.audio"), media).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
        // Model a self-consistent legacy completed snapshot that predates this gate.
        state["schema_version"] = 1.into();
        state["journal"] = serde_json::Value::Null;
        state["artifacts"]["download.json"] = format!("{:x}", Sha256::digest(&download)).into();
        state["artifacts"]["source.json"] = format!("{:x}", Sha256::digest(&metadata)).into();
        state["artifacts"]["media/source.audio"] = format!("{:x}", Sha256::digest(media)).into();
        fs::write(
            fixture.path("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        if command == "recover_pending" {
            journal_transcript(&fixture);
            fs::remove_file(fixture.path("transcript.json")).unwrap();
        }
        let output = if command != "status" {
            fixture.run()
        } else {
            Command::new("/usr/bin/python3")
                .arg(fixture.root.join("agent-youtube-test"))
                .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
                .arg("status")
                .arg("--work-dir")
                .arg(fixture.path(""))
                .output()
                .unwrap()
        };
        assert!(
            !output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout[start..]).unwrap();
        assert_eq!(result["error"]["code"], "audio_duration_mismatch");
        if command == "recover_pending" {
            assert!(
                !fixture.path("transcript.json").exists(),
                "unvalidated pending transcript was linked"
            );
            assert_eq!(
                fs::read(fixture.path(".agent-youtube-111-222-333-transcript.json")).unwrap(),
                original
            );
        } else {
            assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
        }
        assert_eq!(fs::read(fixture.path("media/source.audio")).unwrap(), media);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn invalid_or_missing_source_duration_never_starts_recognition() {
    for duration in [
        "absent",
        "0",
        "-1",
        "12.5",
        "invalid",
        "18446744073709551616",
    ] {
        let fixture = Fixture::new();
        let output = fixture
            .run_command(URL)
            .env("FAKE_DURATION", duration)
            .output()
            .unwrap();
        failure_code(
            &output,
            "source_duration_unavailable",
            &fixture.path("source.json"),
        );
        assert!(!fixture.path("transcript.json").exists());
        assert!(fixture.calls("gigastt", "--output").is_empty());
    }
}

#[test]
fn declared_duration_allows_integer_rounding_and_legacy_numeric_metadata() {
    for duration in ["10", "14", "legacy"] {
        let fixture = Fixture::new();
        success(
            fixture
                .run_command(URL)
                .env("FAKE_DURATION", duration)
                .output()
                .unwrap(),
        );
        assert!(fixture.path("transcript.json").is_file());
    }
}

#[test]
fn selected_duration_cannot_override_conflicting_recording_metadata() {
    use sha2::{Digest, Sha256};
    for (initial, selected, error) in [
        (60, 12, Some("source_duration_conflict")),
        (9, 11, Some("audio_duration_mismatch")),
        (10, 12, None),
    ] {
        let fixture = Fixture::new();
        success(fixture.run());
        let original = fs::read(fixture.path("transcript.json")).unwrap();
        let mut source: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("source.json")).unwrap()).unwrap();
        source["videoDetails"]["lengthSeconds"] = initial.to_string().into();
        let source = serde_json::to_vec(&source).unwrap();
        fs::write(fixture.path("source.json"), &source).unwrap();
        let mut download: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("download.json")).unwrap()).unwrap();
        download["duration_seconds"] = selected.into();
        let download = serde_json::to_vec(&download).unwrap();
        fs::write(fixture.path("download.json"), &download).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
        state["schema_version"] = 1.into();
        state["journal"] = serde_json::Value::Null;
        state["artifacts"]["source.json"] = format!("{:x}", Sha256::digest(&source)).into();
        state["artifacts"]["download.json"] = format!("{:x}", Sha256::digest(&download)).into();
        fs::write(
            fixture.path("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let output = fixture.run();
        if let Some(code) = error {
            assert!(
                !output.status.success(),
                "conflicting duration was silently accepted"
            );
            let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
            let record: serde_json::Value =
                serde_json::from_slice(&output.stdout[start..]).unwrap();
            assert_eq!(record["error"]["code"], code);
        } else {
            success(output);
        }
        assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
        assert_eq!(fixture.calls("http", "/youtubei/v1/player").len(), 1);
    }
}

#[test]
fn unverified_legacy_duration_cannot_report_a_completed_job() {
    use sha2::{Digest, Sha256};
    for fault in [
        "missing_duration",
        "changed_representation",
        "missing_download",
    ] {
        let fixture = Fixture::new();
        success(fixture.run());
        let original = fs::read(fixture.path("transcript.json")).unwrap();
        let mut source: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("source.json")).unwrap()).unwrap();
        if fault == "missing_duration" {
            source["videoDetails"]
                .as_object_mut()
                .unwrap()
                .remove("lengthSeconds");
        }
        if fault == "changed_representation" {
            source["streamingData"]["adaptiveFormats"][0]["lastModified"] =
                "unknown revision".into();
        }
        let source = serde_json::to_vec(&source).unwrap();
        fs::write(fixture.path("source.json"), &source).unwrap();
        let mut download: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("download.json")).unwrap()).unwrap();
        download.as_object_mut().unwrap().remove("duration_seconds");
        let download = serde_json::to_vec(&download).unwrap();
        fs::write(fixture.path("download.json"), &download).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
        state["schema_version"] = 1.into();
        state["journal"] = serde_json::Value::Null;
        state["artifacts"]["source.json"] = format!("{:x}", Sha256::digest(&source)).into();
        state["artifacts"]["download.json"] = format!("{:x}", Sha256::digest(&download)).into();
        if fault == "missing_download" {
            state["artifacts"]
                .as_object_mut()
                .unwrap()
                .remove("download.json");
        }
        fs::write(
            fixture.path("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        failure_code(
            &fixture.run(),
            "source_duration_unavailable",
            &fixture.path("source.json"),
        );
        assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn recovery_default_finishes_one_cli_job_after_a_preserved_prefix_fault() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let source = fixture.root.join("input.m4a");
    let mut bytes = fs::read(&source).unwrap();
    // An ignored MP4 free box makes two real ranged requests without extending audio.
    bytes.extend_from_slice(&(1024u32 * 1024 + 8).to_be_bytes());
    bytes.extend_from_slice(b"free");
    bytes.resize(bytes.len() + 1024 * 1024, 0);
    fs::write(&source, &bytes).unwrap();
    success(
        fixture
            .run_command(URL)
            .env("FAKE_RETRY_AFTER_PREFIX", "1")
            .output()
            .unwrap(),
    );
    let calls = fixture.calls("http", "/audio");
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["range"], "bytes=0-1048575");
    assert!(
        calls[1]["range"]
            .as_str()
            .unwrap()
            .starts_with("bytes=1048576-")
    );
    assert_eq!(calls[1]["range"], calls[2]["range"]);
    assert!(calls[2]["at_ms"].as_u64().unwrap() - calls[1]["at_ms"].as_u64().unwrap() >= 1000);
    let fault = fixture.calls("fault", "retained-prefix");
    assert_eq!(fault.len(), 1);
    assert_eq!(fault[0]["bytes"], 1048576);
    assert_eq!(
        fault[0]["sha256"],
        format!("{:x}", Sha256::digest(&bytes[..1048576]))
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["evidence"]["download"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    let retry: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(retry["schema_version"], 2);
    assert_eq!(retry["window_seconds"], 3600);
    assert_eq!(retry["attempts"], 1);
    assert_eq!(retry["complete"], true);
    assert!(retry["deadline"].is_null());
    assert!(!fixture.path("media/source.audio").exists());
}

#[test]
#[ignore = "requires retained real source and explicitly selected local GigaSTT/models"]
fn research_default_retry_with_real_gigastt() {
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_RECOVERY_PROBE_DIR").unwrap());
    assert!(!directory.exists());
    assert!(
        directory
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .starts_with(
                std::env::current_dir()
                    .unwrap()
                    .join("runs")
                    .canonicalize()
                    .unwrap()
            )
    );
    let source = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_RECOVERY_MEDIA").unwrap())
        .canonicalize()
        .unwrap();
    assert!(source.metadata().unwrap().len() <= 32 * 1024 * 1024);
    let fixture = Fixture::at(directory, true);
    fs::copy(&source, fixture.root.join("input.m4a")).unwrap();
    let id = "of4B3aw7MXo";
    let url = format!("https://www.youtube.com/watch?v={id}");
    let output = Command::new("/usr/bin/python3")
        .arg(fixture.root.join("agent-youtube-test"))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .args(["transcribe", &url, "--work-dir"])
        .arg(fixture.path(""))
        .arg("--model-dir")
        .arg(std::env::var_os("AGENT_YOUTUBE_RECOVERY_MODELS").unwrap())
        .arg("--gigastt")
        .arg(std::env::var_os("AGENT_YOUTUBE_RECOVERY_GIGASTT").unwrap())
        .args(["--chunk-seconds", "10"])
        .env("FAKE_VIDEO_ID", id)
        .env("FAKE_DURATION", "53")
        .env("FAKE_SABR", "1")
        .env("FAKE_SABR_STALL_UNTIL_RETRY", "1")
        .output()
        .unwrap();
    fs::write(fixture.root.join("cli.stdout"), &output.stdout).unwrap();
    fs::write(fixture.root.join("cli.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelopes: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.find('{')
                .and_then(|at| serde_json::from_str::<serde_json::Value>(&line[at..]).ok())
        })
        .filter(|value| value["command"] == "transcribe")
        .collect();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["ok"], true);
    let transcript: agent_youtube::transcript::Transcript =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    transcript.validate().unwrap();
    assert!((transcript.duration - 53.0).abs() < 2.0);
    let retry: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(retry["attempts"], 1);
    assert_eq!(retry["last_error_code"], "download_no_progress");
    assert_eq!(retry["complete"], true);
    assert_eq!(retry["window_seconds"], 3600);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("manifest.json")).unwrap()).unwrap();
    let source_sha = agent_youtube::storage::digest(&source).unwrap();
    assert_eq!(manifest["evidence"]["download"]["sha256"], source_sha);
    assert!(!fixture.path("media/source.audio").exists());
    let raw_count = fs::read_dir(fixture.path("raw"))
        .unwrap()
        .filter(|p| {
            p.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "json")
        })
        .count();
    assert_eq!(raw_count, 6);
    fs::write(fixture.root.join("report.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "version":env!("CARGO_PKG_VERSION"),"controlled_loopback":true,"real_gigastt":true,"video_id":id,
        "one_cli_invocation":true,"success_json_documents":envelopes.len(),"transcript_validated":true,
        "source_sha256":source_sha,"duration_seconds":transcript.duration,"raw_asr_chunks":raw_count,
        "automatic_retries":1,"cause_code":"download_no_progress","cleanup_complete":true,
        "remaining_seconds":retry["remaining_seconds"]
    })).unwrap()).unwrap();
}

#[test]
fn recovery_default_finishes_a_stalled_sabr_job_in_one_invocation() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .env("FAKE_SABR", "1")
            .env("FAKE_SABR_STALL_UNTIL_RETRY", "1")
            .output()
            .unwrap(),
    );
    let retry: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(retry["attempts"], 1);
    assert_eq!(retry["last_error_code"], "download_no_progress");
    assert_eq!(retry["complete"], true);
    assert!(retry["remaining_seconds"].as_u64().unwrap() < 3600);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["evidence"]["download"]["sha256"],
        agent_youtube::storage::digest(&fixture.root.join("input.m4a")).unwrap()
    );
    let control: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-control.json")).unwrap()).unwrap();
    assert_eq!(control["generation"], 1);
}

#[test]
fn recovery_saved_policy_and_missing_enabled_schedule_are_not_reset() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .args(["--retry-window-seconds", "20"])
            .output()
            .unwrap(),
    );
    success(fixture.run());
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job.json")).unwrap()).unwrap();
    assert_eq!(saved["options"]["retry_window_seconds"], 20);
    let transcript = fs::read(fixture.path("transcript.json")).unwrap();
    fs::remove_file(fixture.path("job-retry.json")).unwrap();
    failure(fixture.run(), "job_retry_invalid");
    assert_eq!(
        fs::read(fixture.path("transcript.json")).unwrap(),
        transcript
    );
    assert!(!fixture.path("job-retry.json").exists());
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn recovery_legacy_expired_schedule_keeps_its_deadline_without_new_defaults() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("")).unwrap();
    let legacy = serde_json::to_vec(&serde_json::json!({"schema_version":1,"video_id":"a2hbJlPxj4M","window_seconds":17,"deadline":1,"next_retry_at":null,"attempts":0,"last_error_code":null,"complete":false})).unwrap();
    fs::write(fixture.path("job-retry.json"), &legacy).unwrap();
    failure(fixture.run(), "job_deadline_exceeded");
    assert_eq!(fs::read(fixture.path("job-retry.json")).unwrap(), legacy);
    assert!(fixture.calls("http", "/audio").is_empty());
}

#[test]
fn recovery_explicit_zero_and_legacy_missing_option_stay_disabled() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "0"])
        .env("FAKE_RETRY_COOLDOWN", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    assert!(!fixture.path("job-retry.json").exists());
    let path = fixture.path("job.json");
    let mut saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    saved["options"]
        .as_object_mut()
        .unwrap()
        .remove("retry_window_seconds");
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    success(fixture.run());
    assert!(!fixture.path("job-retry.json").exists());
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["options"]["retry_window_seconds"], 0);
}

#[test]
fn recovery_budget_never_cancels_healthy_asr_even_after_a_retry() {
    for fail_first in [false, true] {
        let fixture = Fixture::new();
        let mut command = fixture.run_command(URL);
        command
            .args(["--retry-window-seconds", "3"])
            .env("FAKE_SLEEP", "2");
        if fail_first {
            command.env("FAKE_RETRY_COOLDOWN", "1");
        }
        success(command.output().unwrap());
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], 2);
        assert!(saved["deadline"].is_null());
        assert_eq!(saved["complete"], true);
        assert_eq!(saved["remaining_seconds"].as_u64().unwrap() < 3, fail_first);
    }
}

#[test]
fn cooldown_beyond_the_job_deadline_never_triggers_an_early_request() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "2"])
        .env("FAKE_RETRY_COOLDOWN", "120")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("job_deadline_exceeded"));
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert!(state["next_retry_at"].as_u64().unwrap() > state["deadline"].as_u64().unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["status", "--work-dir"])
        .arg(fixture.path(""))
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["result"]["state"], "deadline_exceeded");
}

#[test]
fn legacy_retry_window_still_bounds_active_asr_and_permanent_errors_are_not_retried() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "20"])
        .env("FAKE_WRONG_ID", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(fixture.calls("http", "/audio").is_empty());
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(state["attempts"], 0);
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("")).unwrap();
    fs::write(fixture.path("job-retry.json"), serde_json::to_vec(&serde_json::json!({
        "schema_version":1,"video_id":"a2hbJlPxj4M","window_seconds":2,
        "deadline":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()+3,
        "next_retry_at":null,"attempts":0,"last_error_code":null,"complete":false
    })).unwrap()).unwrap();
    let output = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "2"])
        .env("FAKE_SLEEP", "10")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("job_deadline_exceeded"));
    assert!(fixture.path("media/source.audio").exists());
    assert!(!fixture.path("transcript.json").exists());
}

#[test]
fn waiting_job_can_be_cancelled_and_resumed_without_resetting_its_schedule() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let child = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "30"])
        .env("FAKE_RETRY_COOLDOWN", "3")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let limit = Instant::now() + Duration::from_secs(10);
    let schedule = loop {
        if let Ok(bytes) = fs::read(fixture.path("job-retry.json")) {
            let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if state["next_retry_at"].is_u64() {
                break state;
            }
        }
        assert!(Instant::now() < limit, "job never entered waiting");
        std::thread::sleep(Duration::from_millis(20));
    };
    let control = |name: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .args([name, "--work-dir"])
            .arg(fixture.path(""))
            .output()
            .unwrap();
        assert!(output.status.success());
        let mut documents = Vec::new();
        contract::collect(&output, &mut documents);
        contract::validate(&documents);
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let status = control("status");
    assert_eq!(status["result"]["state"], "waiting");
    assert_eq!(status["result"]["next_retry_at"], schedule["next_retry_at"]);
    assert_eq!(control("cancel")["result"]["cancel_requested"], true);
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(130));
    assert_ne!(control("status")["result"]["state"], "waiting");
    let paused: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(paused["deadline"], schedule["deadline"]);
    assert_eq!(paused["remaining_seconds"], schedule["remaining_seconds"]);
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    success(
        Command::new("/usr/bin/python3")
            .arg(fixture.root.join("agent-youtube-test"))
            .args(["resume", "--work-dir"])
            .arg(fixture.path(""))
            .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
            .output()
            .unwrap(),
    );
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert!(after["deadline"].is_null());
    assert!(after["remaining_seconds"].as_u64().unwrap() < 30);
    assert_eq!(after["attempts"], 1);
    let calls = fixture.calls("http", "/audio");
    assert_eq!(calls.len(), 2);
    assert!(calls[1]["at_ms"].as_u64().unwrap() - calls[0]["at_ms"].as_u64().unwrap() >= 3000);
    assert_eq!(control("status")["result"]["state"], "completed");
}

#[test]
fn automatic_retry_honors_cooldown_and_publishes_once() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .args(["--retry-window-seconds", "20"])
            .env("FAKE_RETRY_COOLDOWN", "1")
            .output()
            .unwrap(),
    );
    let calls = fixture.calls("http", "/audio");
    assert_eq!(calls.len(), 2);
    assert!(calls[1]["at_ms"].as_u64().unwrap() - calls[0]["at_ms"].as_u64().unwrap() >= 1000);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
    assert_eq!(state["complete"], true);
    assert_eq!(state["attempts"], 1);
    assert_eq!(state["next_retry_at"], serde_json::Value::Null);
}

#[test]
fn retry_window_deadline_is_durable_and_does_not_restart_on_resume() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--retry-window-seconds", "1"])
        .env("FAKE_FAIL_DOWNLOAD", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("job_deadline_exceeded"), "{text}");
    assert!(!fixture.path("transcript.json").exists());
    let before = fs::read(fixture.path("job-retry.json")).unwrap();
    let calls = fixture.calls("http", "/audio").len();
    let output = Command::new("/usr/bin/python3")
        .arg(fixture.root.join("agent-youtube-test"))
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("job_deadline_exceeded"));
    assert_eq!(fixture.calls("http", "/audio").len(), calls);
    assert_eq!(fs::read(fixture.path("job-retry.json")).unwrap(), before);
}

#[test]
fn completed_publication_is_reconciled_before_expired_retry_deadline() {
    for cleanup_pending in [false, true] {
        let fixture = Fixture::new();
        success(
            fixture
                .run_command(URL)
                .args(["--retry-window-seconds", "60"])
                .output()
                .unwrap(),
        );
        let original = fs::read(fixture.path("transcript.json")).unwrap();
        let mut retry: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
        retry["complete"] = false.into();
        // A valid expired v2 wait must not defeat durable publication recovery.
        retry["waiting_since"] = 1.into();
        retry["deadline"] = (retry["remaining_seconds"].as_u64().unwrap() + 2).into();
        retry["next_retry_at"] = 2.into();
        retry["attempts"] = 1.into();
        retry["last_error_code"] = "download_no_progress".into();
        fs::write(
            fixture.path("job-retry.json"),
            serde_json::to_vec(&retry).unwrap(),
        )
        .unwrap();
        if cleanup_pending {
            let media = fixture.path("media/source.audio");
            fs::write(&media, b"owned media").unwrap();
            let mut state: serde_json::Value =
                serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
            state["artifacts"]["media/source.audio"] =
                agent_youtube::storage::digest(&media).unwrap().into();
            fs::write(
                fixture.path("state.json"),
                serde_json::to_vec(&state).unwrap(),
            )
            .unwrap();
        }
        success(
            fixture
                .run_command(URL)
                .args(["--retry-window-seconds", "60"])
                .output()
                .unwrap(),
        );
        assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
        assert_eq!(fixture.calls("http", "/audio").len(), 1);
        let retry: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("job-retry.json")).unwrap()).unwrap();
        assert_eq!(retry["complete"], true);
        assert!(!fixture.path("media/source.audio").exists());
    }
}

#[test]
fn completed_resume_skips_download_extraction_and_recognition() {
    let fixture = Fixture::new();
    success(fixture.run());
    let original = fs::read(fixture.path("transcript.json")).unwrap();
    // A published transcript can be reused without the producing media backend.
    let path = fixture.path("state.json");
    let mut state: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["config"]["media_backend"] = "previous-media-backend".into();
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    success(fixture.run());
    assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    assert_eq!(fixture.calls("http", "/youtubei/v1/player").len(), 1);
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    assert!(fixture.calls("ffmpeg", "-i").is_empty());
    assert!(fixture.calls("ffprobe", "-version").is_empty());
}

#[test]
fn changed_acquisition_keeps_prior_attempt_until_successful_transcript_resume() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .args(["--retry-window-seconds", "0"])
            .env("FAKE_FAIL_DOWNLOAD", "1")
            .output()
            .unwrap(),
        "503",
    );
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    let old = state["download_prefix"].as_str().unwrap();
    let receipt = fixture.path(&format!("{old}.json"));
    let media = fixture.path(&format!("{old}.m4a"));
    let partial = fs::read(fixture.root.join("input.m4a")).unwrap()[..1024].to_vec();
    let mut retained: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    retained["progress"]["bytes"] = partial.len().into();
    retained["progress"]["digest"] = format!("{:x}", Sha256::digest(&partial)).into();
    fs::write(&media, &partial).unwrap();
    fs::write(&receipt, serde_json::to_vec(&retained).unwrap()).unwrap();
    let saved = fs::read(&receipt).unwrap();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_LAST_MODIFIED", "67890")
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    assert_eq!(fs::read(&receipt).unwrap(), saved);
    assert_eq!(fs::read(&media).unwrap(), partial);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert_eq!(state["retired_downloads"], serde_json::json!([old]));
    assert_ne!(state["download_prefix"], old);
    success(
        fixture
            .run_command(URL)
            .env("FAKE_LAST_MODIFIED", "67890")
            .output()
            .unwrap(),
    );
    assert!(!receipt.exists());
    assert!(!media.exists());
    assert!(
        fs::read_dir(fixture.path("media"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn failed_second_chunk_preserves_first_and_resume_only_retries_incomplete_asr() {
    let fixture = Fixture::new();
    let first = fixture
        .run_command(URL)
        .env("FAKE_FAIL_CHUNK", "000001")
        .output()
        .unwrap();
    failure(first, "process_failed");
    for name in [
        "media/source.audio",
        "media/audio.json",
        "chunks/000000.wav",
        "chunks/000001.wav",
    ] {
        assert!(fixture.path(name).is_file(), "failure discarded {name}");
    }
    assert!(!fixture.path("media/audio.wav").exists());
    let saved = fs::read(fixture.path("raw/000000.json")).unwrap();
    assert!(!fixture.path("raw/000001.json").exists());
    assert!(!fixture.path("transcript.json").exists());
    success(fixture.run());
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), saved);
    let calls = fixture.calls("gigastt", "--output");
    assert_eq!(calls.len(), 3);
    let first_chunk_calls = calls
        .iter()
        .filter(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg.as_str().is_some_and(|s| s.ends_with("/000000.wav")))
        })
        .count();
    assert_eq!(first_chunk_calls, 1);
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    for name in [
        "media/source.audio",
        "media/audio.json",
        "chunks/000000.wav",
        "chunks/000001.wav",
    ] {
        assert!(
            !fixture.path(name).exists(),
            "successful resume retained {name}"
        );
    }
}

#[test]
fn completed_resume_finishes_interrupted_cleanup_without_touching_unowned_files() {
    let fixture = Fixture::new();
    success(fixture.run());
    let transcript = fs::read(fixture.path("transcript.json")).unwrap();
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    // Simulate a crash after some media was deleted but before receipts were retired.
    for name in ["media/source.audio", "media/audio.wav", "chunks/000000.wav"] {
        fs::write(fixture.path(name), b"owned media").unwrap();
        state["artifacts"][name] =
            serde_json::json!(agent_youtube::storage::digest(&fixture.path(name)).unwrap());
    }
    state["artifacts"]["chunks/000001.wav"] = serde_json::json!("already removed");
    let scratch = "media/.agent-youtube-111-222-333-audio.partial.wav";
    fs::write(fixture.path(scratch), b"unfinished conversion").unwrap();
    state["scratch"] = serde_json::json!([scratch]);
    state["download_started"] = serde_json::json!(true);
    state["download_prefix"] = serde_json::json!("media/download.111.222.333");
    fs::write(
        fixture.path("media/download.111.222.333.webm.part"),
        b"download fragment",
    )
    .unwrap();
    fs::write(fixture.path("media/user-note.txt"), b"owner media note").unwrap();
    fs::write(fixture.path("media/download.notes"), b"owner download note").unwrap();
    fs::write(fixture.path("chunks/user-note.wav"), b"owner audio").unwrap();
    fs::write(
        fixture.path("media/.agent-youtube-444-555-666-audio.partial.wav"),
        b"unregistered scratch",
    )
    .unwrap();
    fs::write(fixture.path("transcript.txt"), b"owner export").unwrap();
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    success(fixture.run());
    for name in [
        "media/source.audio",
        "media/audio.wav",
        "chunks/000000.wav",
        "chunks/000001.wav",
        scratch,
        "media/download.111.222.333.webm.part",
    ] {
        assert!(!fixture.path(name).exists(), "cleanup retained {name}");
    }
    for (name, expected) in [
        ("media/user-note.txt", &b"owner media note"[..]),
        ("media/download.notes", &b"owner download note"[..]),
        ("chunks/user-note.wav", &b"owner audio"[..]),
        (
            "media/.agent-youtube-444-555-666-audio.partial.wav",
            &b"unregistered scratch"[..],
        ),
        ("transcript.txt", &b"owner export"[..]),
    ] {
        assert_eq!(
            fs::read(fixture.path(name)).unwrap(),
            expected,
            "cleanup changed {name}"
        );
    }
    assert_eq!(
        fs::read(fixture.path("transcript.json")).unwrap(),
        transcript
    );
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    let state: serde_json::Value = serde_json::from_slice(&fs::read(state_path).unwrap()).unwrap();
    assert!(
        state["artifacts"]
            .as_object()
            .unwrap()
            .keys()
            .all(|name| !name.starts_with("media/") && !name.starts_with("chunks/"))
    );
    assert!(state["scratch"].as_array().unwrap().is_empty());
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    success(fixture.run());
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn invalid_transcript_never_authorizes_media_cleanup() {
    for valid_digest in [false, true] {
        let fixture = Fixture::new();
        success(fixture.run());
        let media = fixture.path("media/source.audio");
        fs::write(&media, b"retained until a valid transcript exists").unwrap();
        let state_path = fixture.path("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        state["artifacts"]["media/source.audio"] =
            serde_json::json!(agent_youtube::storage::digest(&media).unwrap());
        let transcript_path = fixture.path("transcript.json");
        let mut transcript: serde_json::Value =
            serde_json::from_slice(&fs::read(&transcript_path).unwrap()).unwrap();
        transcript["schema_version"] = serde_json::json!(99);
        fs::write(&transcript_path, serde_json::to_vec(&transcript).unwrap()).unwrap();
        if valid_digest {
            state["artifacts"]["transcript.json"] =
                serde_json::json!(agent_youtube::storage::digest(&transcript_path).unwrap());
        }
        fs::write(state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
        failure(
            fixture.run(),
            if valid_digest {
                "unsupported transcript schema"
            } else {
                "checkpoint integrity mismatch"
            },
        );
        assert_eq!(
            fs::read(media).unwrap(),
            b"retained until a valid transcript exists"
        );
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn cleanup_preflights_all_media_before_deleting_any_candidate() {
    for symlink in [false, true] {
        let fixture = Fixture::new();
        success(fixture.run());
        let valid = fixture.path("chunks/000000.wav");
        let invalid = fixture.path("media/source.audio");
        fs::write(&valid, b"valid owned chunk").unwrap();
        fs::write(&invalid, b"original source").unwrap();
        let state_path = fixture.path("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        for (name, path) in [
            ("chunks/000000.wav", &valid),
            ("media/source.audio", &invalid),
        ] {
            state["artifacts"][name] =
                serde_json::json!(agent_youtube::storage::digest(path).unwrap());
        }
        let target = fixture.root.join("owner-source");
        fs::write(&target, b"owner data").unwrap();
        if symlink {
            fs::remove_file(&invalid).unwrap();
            std::os::unix::fs::symlink(&target, &invalid).unwrap();
        } else {
            fs::write(&invalid, b"changed source").unwrap();
        }
        let journal = serde_json::to_vec_pretty(&state).unwrap();
        fs::write(&state_path, &journal).unwrap();
        failure(
            fixture.run(),
            if symlink {
                "refusing nonregular artifact"
            } else {
                "cleanup integrity mismatch"
            },
        );
        assert_eq!(fs::read(valid).unwrap(), b"valid owned chunk");
        assert!(fs::symlink_metadata(invalid).is_ok());
        assert_eq!(fs::read(target).unwrap(), b"owner data");
        assert_eq!(fs::read(state_path).unwrap(), journal);
        assert!(fixture.path("transcript.json").is_file());
        assert!(fixture.path("raw/000000.json").is_file());
    }
}

#[test]
fn media_url_cannot_redirect_downloads_outside_the_allowed_origin() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_UNTRUSTED_URL", "1")
            .output()
            .unwrap(),
        "untrusted media",
    );
    assert!(!fixture.path("media/source.audio").exists());
    assert!(!fixture.path("transcript.json").exists());
    assert!(fixture.calls("gigastt", "--output").is_empty());
}

#[test]
fn completed_run_cleans_media_without_tools_or_models() {
    let fixture = Fixture::new();
    success(fixture.run());
    let calls = fs::read(fixture.root.join("calls.jsonl")).unwrap();
    let media = fixture.path("media/source.audio");
    fs::write(&media, b"owned source awaiting cleanup").unwrap();
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["artifacts"]["media/source.audio"] =
        serde_json::json!(agent_youtube::storage::digest(&media).unwrap());
    state["config"]
        .as_object_mut()
        .unwrap()
        .remove("media_backend");
    fs::write(state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    fs::remove_file(fixture.root.join("gigastt")).unwrap();
    fs::remove_dir_all(fixture.root.join("models")).unwrap();
    success(
        fixture
            .run_command(URL)
            .env("PATH", "/nonexistent-agent-youtube-test-path")
            .output()
            .unwrap(),
    );
    assert!(!media.exists());
    assert_eq!(fs::read(fixture.root.join("calls.jsonl")).unwrap(), calls);
    assert!(fixture.path("transcript.json").is_file());
    assert!(fixture.path("raw/000000.json").is_file());
}

#[test]
fn legacy_video_checkpoint_is_reused_then_removed_after_success() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    let source = fixture.path("media/source.audio");
    let legacy = fixture.path("media/video.mkv");
    if source.exists() {
        fs::rename(source, &legacy).unwrap();
        let digest = state["artifacts"]
            .as_object_mut()
            .unwrap()
            .remove("media/source.audio")
            .unwrap();
        state["artifacts"]["media/video.mkv"] = digest;
    }
    state.as_object_mut().unwrap().remove("scratch");
    state.as_object_mut().unwrap().remove("download_started");
    state.as_object_mut().unwrap().remove("download_prefix");
    fs::write(state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    success(fixture.run());
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 3);
    assert!(!legacy.exists());
    assert!(!fixture.path("media/audio.wav").exists());
    assert!(fixture.path("raw/000000.json").exists());
}

#[test]
fn incompatible_dependency_fails_before_downloading_media() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_INCOMPATIBLE", "1")
            .output()
            .unwrap(),
        "incompatible gigastt",
    );
    assert!(fixture.calls("http", "/audio").is_empty());
    assert!(fixture.calls("gigastt", "--output").is_empty());
    assert!(!fixture.path("media/video.mkv").exists());
}

#[test]
fn corrupted_checkpoint_is_rejected_without_repeating_recognition() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    fs::write(fixture.path("raw/000000.json"), b"tampered raw data").unwrap();
    let output = fixture.run();
    failure_code(
        &output,
        "checkpoint_integrity_mismatch",
        &fixture.path("raw/000000.json"),
    );
    failure(output, "checkpoint integrity mismatch");
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    assert_eq!(
        fs::read(fixture.path("raw/000000.json")).unwrap(),
        b"tampered raw data"
    );
}

#[test]
fn existing_user_export_is_preserved() {
    let fixture = Fixture::new();
    success(fixture.run());
    let output = fixture.root.join("existing.txt");
    fs::write(&output, b"owner data").unwrap();
    let attempt = export(&fixture.path("transcript.json"), &output);
    assert!(!attempt.status.success());
    assert_eq!(fs::read(output).unwrap(), b"owner data");
}

#[test]
fn unexpected_metadata_id_cannot_become_a_source() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_WRONG_ID", "1")
            .output()
            .unwrap(),
        "unexpected video id",
    );
    assert!(!fixture.path("source.json").exists());
    assert!(fixture.calls("http", "/audio").is_empty());
}

#[test]
fn invalid_urls_and_option_injection_never_launch_dependencies() {
    let fixture = Fixture::new();
    for input in [
        "--exec=touch /tmp/agent-youtube-injected",
        "https://youtube.com.evil/watch?v=a2hbJlPxj4M",
        "file:///etc/passwd",
        "https://youtu.be/../../file",
        "https://youtu.be/a2hbJlPxj4M;touch /tmp/agent-youtube-injected",
    ] {
        assert!(
            !fixture
                .run_command(input)
                .output()
                .unwrap()
                .status
                .success(),
            "accepted {input}"
        );
    }
    assert!(!fixture.root.join("calls.jsonl").exists());
    assert!(!fixture.path("source.json").exists());
}

fn journal_transcript(fixture: &Fixture) -> Vec<u8> {
    let original = fs::read(fixture.path("transcript.json")).unwrap();
    fs::write(
        fixture.path(".agent-youtube-111-222-333-transcript.json"),
        &original,
    )
    .unwrap();
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    let digest = state["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove("transcript.json")
        .unwrap();
    state["publications"] = serde_json::json!({
        "transcript.json": {"pending": ".agent-youtube-111-222-333-transcript.json", "digest": digest}
    });
    fs::write(state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    original
}

#[test]
fn publication_journal_recovers_before_and_after_final_link_without_recognition() {
    for final_exists in [false, true] {
        let fixture = Fixture::new();
        success(fixture.run());
        let original = journal_transcript(&fixture);
        if !final_exists {
            fs::remove_file(fixture.path("transcript.json")).unwrap();
        }
        success(fixture.run());
        assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
        assert!(
            !fixture
                .path(".agent-youtube-111-222-333-transcript.json")
                .exists()
        );
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
        assert!(state["publications"].as_object().unwrap().is_empty());
        assert!(state["artifacts"]["transcript.json"].is_string());
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn publication_journal_never_replaces_conflicting_final_file() {
    let fixture = Fixture::new();
    success(fixture.run());
    let pending = journal_transcript(&fixture);
    fs::write(
        fixture.path("transcript.json"),
        b"owner conflicting content",
    )
    .unwrap();
    let output = fixture.run();
    failure_code(
        &output,
        "checkpoint_publication_conflict",
        &fixture.path("transcript.json"),
    );
    failure(output, "publication conflicts with existing");
    assert_eq!(
        fs::read(fixture.path("transcript.json")).unwrap(),
        b"owner conflicting content"
    );
    assert_eq!(
        fs::read(fixture.path(".agent-youtube-111-222-333-transcript.json")).unwrap(),
        pending
    );
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert!(state["publications"]["transcript.json"].is_object());
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn publication_journal_rejects_corrupted_pending_file() {
    let fixture = Fixture::new();
    success(fixture.run());
    journal_transcript(&fixture);
    fs::remove_file(fixture.path("transcript.json")).unwrap();
    fs::write(
        fixture.path(".agent-youtube-111-222-333-transcript.json"),
        b"damaged pending data",
    )
    .unwrap();
    let output = fixture.run();
    failure_code(
        &output,
        "checkpoint_pending_invalid",
        &fixture.path(".agent-youtube-111-222-333-transcript.json"),
    );
    failure(output, "pending publication failed integrity verification");
    assert!(!fixture.path("transcript.json").exists());
    assert_eq!(
        fs::read(fixture.path(".agent-youtube-111-222-333-transcript.json")).unwrap(),
        b"damaged pending data"
    );
}

#[test]
fn resume_preserves_unowned_deterministic_partial_file() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let partial = fixture.path("raw/000001.partial.json");
    fs::write(&partial, b"unowned user partial").unwrap();
    success(fixture.run());
    assert_eq!(fs::read(partial).unwrap(), b"unowned user partial");
    assert!(fixture.path("transcript.json").is_file());
}

#[test]
fn explicit_model_path_works_when_home_is_unset() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .env_remove("HOME")
            .output()
            .unwrap(),
    );
    assert!(fixture.path("transcript.json").is_file());
}

#[test]
fn malicious_journal_cannot_claim_an_unrelated_user_file() {
    let fixture = Fixture::new();
    success(fixture.run());
    let original = fs::read(fixture.path("transcript.json")).unwrap();
    let note = fixture.path("user-note.txt");
    // Identical bytes reproduce the dangerous case where a digest check alone
    // would let a forged intent delete an unrelated owner file.
    fs::write(&note, &original).unwrap();
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    let digest = state["artifacts"]["transcript.json"].clone();
    state["publications"] = serde_json::json!({
        "transcript.json": {"pending": "user-note.txt", "digest": digest}
    });
    let forged = serde_json::to_vec_pretty(&state).unwrap();
    fs::write(&state_path, &forged).unwrap();
    failure(fixture.run(), "publication scratch lacks ownership prefix");
    assert_eq!(fs::read(note).unwrap(), original);
    assert_eq!(fs::read(state_path).unwrap(), forged);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn journal_refuses_symlinked_pending_and_final_files_without_changing_targets() {
    for pending_is_link in [true, false] {
        let fixture = Fixture::new();
        success(fixture.run());
        let original = journal_transcript(&fixture);
        let target = fixture.root.join("owner-outside-run.json");
        fs::write(&target, &original).unwrap();
        let link = if pending_is_link {
            fixture.path(".agent-youtube-111-222-333-transcript.json")
        } else {
            fixture.path("transcript.json")
        };
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        failure(fixture.run(), "refusing nonregular artifact");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(target).unwrap(), original);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn journal_rejects_unknown_namespaces_before_entering_symlinked_directory() {
    let fixture = Fixture::new();
    success(fixture.run());
    let target = fixture.root.join("owner-directory");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join(".agent-youtube-111-222-333-transcript.json"),
        b"owner data",
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, fixture.path("extra")).unwrap();
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["publications"] = serde_json::json!({
        "extra/transcript.json": {"pending": "extra/.agent-youtube-111-222-333-transcript.json", "digest": "untrusted"}
    });
    fs::write(state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    failure(fixture.run(), "unsupported publication artifact");
    assert_eq!(
        fs::read(target.join(".agent-youtube-111-222-333-transcript.json")).unwrap(),
        b"owner data"
    );
    assert!(!target.join("transcript.json").exists());
}

#[test]
fn checkpoint_state_and_source_symlinks_are_rejected_even_with_matching_contents() {
    for name in ["state.json", "source.json"] {
        let fixture = Fixture::new();
        failure(
            fixture
                .run_command(URL)
                .env("FAKE_FAIL_CHUNK", "000001")
                .output()
                .unwrap(),
            "process_failed",
        );
        let original = fs::read(fixture.path(name)).unwrap();
        let target = fixture.root.join("owner-checkpoint.json");
        fs::write(&target, &original).unwrap();
        fs::remove_file(fixture.path(name)).unwrap();
        std::os::unix::fs::symlink(&target, fixture.path(name)).unwrap();
        failure(fixture.run(), "refusing nonregular artifact");
        assert!(
            fs::symlink_metadata(fixture.path(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(target).unwrap(), original);
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn resumed_download_rejects_linked_partial_files_before_launch() {
    for hardlink in [false, true] {
        let fixture = Fixture::new();
        failure(
            fixture
                .run_command(URL)
                .args(["--retry-window-seconds", "0"])
                .env("FAKE_FAIL_DOWNLOAD", "1")
                .output()
                .unwrap(),
            "download_http_temporary",
        );
        let owner = fixture.root.join("owner-data");
        fs::write(&owner, b"owner data").unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
        let prefix = state["download_prefix"].as_str().unwrap();
        let partial = fixture.path(&format!("{prefix}.webm.part"));
        if hardlink {
            fs::hard_link(&owner, &partial).unwrap();
        } else {
            std::os::unix::fs::symlink(&owner, &partial).unwrap();
        }
        failure(fixture.run(), "unsafe download scratch");
        assert_eq!(fs::read(&owner).unwrap(), b"owner data");
        assert_eq!(fixture.calls("http", "/audio").len(), 4);
    }
}

#[test]
fn resume_rejects_changed_ambient_asr_settings() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("GIGASTT_VAD", "1")
            .output()
            .unwrap(),
        "unsupported ambient ASR setting",
    );
    failure(
        fixture
            .run_command(URL)
            .env("GIGASTT_BATCH_DELETE_SOURCE", "1")
            .output()
            .unwrap(),
        "unsupported ambient ASR setting",
    );
    assert!(fixture.calls("gigastt", "--output").is_empty());
}

#[test]
fn rounded_asr_duration_does_not_break_chunk_assembly() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .env("FAKE_DURATION_OFFSET", "0.001")
            .output()
            .unwrap(),
    );
    let transcript: agent_youtube::transcript::Transcript =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    assert_eq!(transcript.duration, 12.0);
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("raw/000000.json")).unwrap()).unwrap();
    assert!((raw["duration"].as_f64().unwrap() - 10.001).abs() < 1e-9);
}

#[test]
fn work_directory_metacharacters_are_literal_not_flags_or_templates() {
    let fixture = Fixture::new();
    let path = fixture.root.join("-run 50% $AGENT_YOUTUBE_PATH_COMPONENT");
    success(
        fixture
            .run_command_in(URL, &path)
            .env("AGENT_YOUTUBE_PATH_COMPONENT", "unexpected-expansion")
            .output()
            .unwrap(),
    );
    assert!(path.join("transcript.json").is_file());
    assert!(!fixture.root.join("-run 50% unexpected-expansion").exists());
}

#[test]
fn transcribes_with_no_ffmpeg_or_ffprobe_on_path() {
    let fixture = Fixture::new();
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    std::os::unix::fs::symlink(
        String::from_utf8(python.stdout).unwrap().trim(),
        fixture.root.join("python3"),
    )
    .unwrap();
    success(
        fixture
            .run_command(URL)
            .env("PATH", &fixture.root)
            .output()
            .unwrap(),
    );
    assert!(fixture.path("transcript.json").is_file());
}

#[test]
fn interruption_during_native_decode_retains_source_and_resumes() {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let fixture = Fixture::new();
    let mut child = fixture
        .run_command(URL)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut errors = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            errors.read_line(&mut line).unwrap() > 0,
            "pipeline stopped before native decoding"
        );
        if serde_json::from_str::<serde_json::Value>(&line).unwrap()["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("[2/4]")
        {
            break;
        }
    }
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let mut remaining_errors = String::new();
    errors.read_to_string(&mut remaining_errors).unwrap();
    assert_eq!(
        child.wait().unwrap().code(),
        Some(130),
        "{remaining_errors}"
    );
    assert!(fixture.path("media/source.audio").is_file());
    assert!(!fixture.path("media/audio.wav").exists());
    assert!(!fixture.path("raw/000000.json").exists());
    success(fixture.run());
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    assert!(fixture.path("transcript.json").is_file());
}

fn aac_fixture(rate: u32, frames: usize) -> Vec<u8> {
    let left: Vec<f32> = (0..frames)
        .map(|i| 0.25 * (i as f32 * 0.07).sin())
        .collect();
    let right: Vec<f32> = left.iter().map(|sample| sample * 0.5).collect();
    syom::encode_with(
        &[&left, &right],
        rate,
        &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
    )
    .unwrap()
}

#[test]
fn rust_media_path_preserves_pcm_at_chunk_boundaries_and_drains_fractional_tail() {
    let fixture = Fixture::new();
    // A 44.1 kHz edit length that cannot be divided into whole 16 kHz samples.
    let input = aac_fixture(44_100, 12 * 44_100 + 1);
    fs::write(fixture.root.join("input.m4a"), &input).unwrap();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    assert!(!fixture.path("media/audio.wav").exists());
    let decoded = syom::decode(&input).unwrap();
    let from = bandlimit::Rate::new(44100).unwrap();
    let to = bandlimit::Rate::new(16000).unwrap();
    let quantum = bandlimit::Ratio::reduce(from, to).numerator() as usize;
    let mut pcm = decoded.channels[0].clone();
    pcm.resize(pcm.len().div_ceil(quantum) * quantum, 0.0);
    let converted = bandlimit::convert(&pcm, from, to).unwrap();
    let reference = fixture.root.join("reference.wav");
    ryf::write_s16(&reference, &ryf::f32_to_s16le(&converted[..192001]), 16000).unwrap();
    let audio = ryf::read(reference).unwrap();
    assert_eq!(
        (audio.sample_rate, audio.num_channels(), audio.frames()),
        (16_000, 1, 192_001)
    );
    let first = ryf::read(fixture.path("chunks/000000.wav")).unwrap();
    let last = ryf::read(fixture.path("chunks/000001.wav")).unwrap();
    assert_eq!(first.frames(), 160_000);
    assert_eq!(last.frames(), 32_001);
    assert_eq!(first.channels[0], audio.channels[0][..160_000]);
    assert_eq!(last.channels[0], audio.channels[0][160_000..]);
    assert!(audio.channels[0].iter().any(|x| x.abs() > 0.05));
    success(fixture.run());
    let transcript: agent_youtube::transcript::Transcript =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    assert_eq!(transcript.duration, 192_001.0 / 16_000.0);
}

#[test]
fn unsupported_or_truncated_source_keeps_media_without_publishing_audio_or_asr() {
    for bytes in [
        b"not an AAC file".as_slice(),
        &aac_fixture(48_000, 48_000)[..64],
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("input.m4a"), bytes).unwrap();
        let output = fixture.run();
        failure_code(
            &output,
            "audio_decode_invalid",
            &fixture.path("media/source.audio"),
        );
        assert_eq!(fs::read(fixture.path("media/source.audio")).unwrap(), bytes);
        assert!(!fixture.path("media/audio.wav").exists());
        assert!(!fixture.path("transcript.json").exists());
        assert!(fixture.calls("gigastt", "--output").is_empty());
    }
}

#[test]
fn unfinished_legacy_preprocessing_is_rejected_without_modifying_saved_results() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let state_path = fixture.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["config"]
        .as_object_mut()
        .unwrap()
        .remove("media_backend");
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let source = fs::read(fixture.path("media/source.audio")).unwrap();
    failure(fixture.run(), "use a new --work-dir");
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(
        fs::read(fixture.path("media/source.audio")).unwrap(),
        source
    );
    assert_eq!(fixture.calls("http", "/audio").len(), 1);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn permissive_umask_cannot_expose_owned_work_artifacts() {
    use std::os::unix::process::CommandExt;
    let fixture = Fixture::new();
    let mut command = fixture.run_command(URL);
    command.env("FAKE_FAIL_CHUNK", "000001");
    unsafe {
        command.pre_exec(|| {
            libc::umask(0);
            Ok(())
        });
    }
    failure(command.output().unwrap(), "process_failed");
    fn check(path: &Path) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            if metadata.is_dir() { 0o700 } else { 0o600 },
            "{}",
            path.display()
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                check(&entry.unwrap().path());
            }
        }
    }
    check(&fixture.path(""));
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    success(fixture.run());
    check(&fixture.path(""));
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
}

#[test]
fn canonical_assembly_keeps_integrity_error_after_asr_changes_retained_raw() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let original = fs::read(fixture.path("raw/000000.json")).unwrap();
    let output = fixture
        .run_command(URL)
        .env("FAKE_CHANGE_RETAINED_RAW", "1")
        .output()
        .unwrap();
    failure_code(
        &output,
        "checkpoint_integrity_mismatch",
        &fixture.path("raw/000000.json"),
    );
    assert_eq!(
        fs::read(fixture.path("raw/000000.json")).unwrap(),
        [original.as_slice(), b" "].concat()
    );
    assert!(fixture.path("raw/000001.json").is_file());
    assert!(fixture.path("media/source.audio").is_file());
    assert!(fixture.path("chunks/000000.wav").is_file());
    assert!(!fixture.path("transcript.json").exists());
    assert_eq!(fixture.calls("gigastt", "fixture-file").len(), 3);
}

#[test]
fn pause_boundaries_preserve_pcm_raw_and_resume_identity() {
    let fixture = Fixture::new();
    let rate = 48000;
    let samples: Vec<f32> = (0..12 * rate)
        .map(|i| {
            if (10.15..10.65).contains(&(i as f64 / rate as f64)) {
                0.0
            } else {
                0.3 * (i as f32 * 0.07).sin()
            }
        })
        .collect();
    fs::write(
        fixture.root.join("input.m4a"),
        syom::encode_with(
            &[&samples],
            rate as u32,
            &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
        )
        .unwrap(),
    )
    .unwrap();
    failure(
        fixture
            .run_command(URL)
            .args(["--boundary-window-ms", "1000"])
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let plan_bytes = fs::read(fixture.path("recognition.json")).unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&plan_bytes).unwrap();
    let edge = plan["edges"][1].as_u64().unwrap();
    assert!(edge > 160000 && edge < 176000);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert!(
        state["config"]["media_backend"]
            .as_str()
            .unwrap()
            .ends_with("-quiet-boundaries-v1")
    );
    let batch = state["batch"]["namespace"].as_str().unwrap();
    let mut input = Vec::new();
    let mut fixed = Vec::new();
    for i in 0..2 {
        input.extend(
            ryf::read_s16(fixture.path(&format!("{batch}/input/{i:06}.wav")))
                .unwrap()
                .1,
        );
        fixed.extend(
            ryf::read_s16(fixture.path(&format!("chunks/{i:06}.wav")))
                .unwrap()
                .1,
        );
    }
    assert_eq!(
        input, fixed,
        "boundary shifts must retain every PCM sample exactly once"
    );
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    failure(fixture.run(), "checkpoint_settings_mismatch");
    fs::write(fixture.path("recognition.json"), b"corrupt plan").unwrap();
    failure(
        fixture
            .run_command(URL)
            .args(["--boundary-window-ms", "1000"])
            .output()
            .unwrap(),
        "checkpoint_integrity_mismatch",
    );
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert!(fixture.path("media/source.audio").exists());
    fs::write(fixture.path("recognition.json"), &plan_bytes).unwrap();
    success(
        fixture
            .run_command(URL)
            .args(["--boundary-window-ms", "1000"])
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(
        fs::read(fixture.path("recognition.json")).unwrap(),
        plan_bytes
    );
    let transcript: agent_youtube::transcript::Transcript =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    transcript.validate().unwrap();
    assert_eq!(transcript.segments[1].start, edge as f64 / 16000.0);
    assert!(transcript.segments[0].end <= transcript.segments[1].start);
    assert_eq!(transcript.duration, 12.0);
    assert!(
        fs::read_dir(fixture.path("chunks"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(fixture.path("media"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn asr_memory_stop_adopts_completed_results_and_resumes() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--asr-memory-mib", "32"])
        .env("FAKE_MEMORY_AFTER_FIRST", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let record: serde_json::Value = serde_json::from_slice(
        &output.stdout[output.stdout.iter().position(|b| *b == b'{').unwrap()..],
    )
    .unwrap();
    assert_eq!(record["error"]["code"], "resource_limit");
    assert_eq!(record["error"]["details"]["stage"], "recognize");
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    assert!(fixture.path("media/source.audio").exists());
    assert!(!fixture.path("transcript.json").exists());
    success(fixture.run());
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(fixture.calls("gigastt", "fixture-file").len(), 2);
}

const FAKE_TOOL: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys, wave
root = pathlib.Path(__file__).parent
name = pathlib.Path(__file__).name
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as log:
    log.write(json.dumps({'tool': name, 'args': args}) + '\n')
if '--version' in args or '-version' in args:
    print(name + ' 2.22.0')
elif '--help' in args:
    flags = '--format --output --model-variant' if os.environ.get('FAKE_INCOMPATIBLE') else '--format --output --offline --model-variant --pool-size --retries --punctuation --itn --log-level --model-dir --encoder-intra-threads'
    print(' '.join(f for f in flags.split() if f != os.environ.get('FAKE_MISSING_FLAG')))
elif name == 'gigastt':
    if os.environ.get('FAKE_ASR_EXCLUSIVE'):
        import fcntl
        slot = (root / 'asr-exclusive.lock').open('w')
        fcntl.flock(slot, fcntl.LOCK_EX | fcntl.LOCK_NB)
    batch = args[0] == 'transcribe-batch'
    files = sorted(pathlib.Path(args[1]).glob('*.wav')) if batch else [pathlib.Path(args[1])]
    failed = False
    for audio in files:
        target = pathlib.Path(args[2]) / (audio.stem + '.json') if batch else pathlib.Path(args[args.index('--output') + 1])
        if batch:
            with (root / 'calls.jsonl').open('a') as log:
                log.write(json.dumps({'tool': name, 'args': ['fixture-file', str(audio), '--output', str(target)]}) + '\n')
        if audio.stem == os.environ.get('FAKE_FAIL_CHUNK'):
            print('fixture recognition failure', file=sys.stderr)
            failed = True
            continue
        with wave.open(str(audio), 'rb') as wav:
            assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) == (1, 2, 16000)
            duration = wav.getnframes() / wav.getframerate()
        result = {'duration': duration + float(os.environ.get('FAKE_DURATION_OFFSET', '0')), 'text': ('x' * int(os.environ['FAKE_TEXT_SIZE'])) if os.environ.get('FAKE_TEXT_SIZE') else 'Привет, мир!', 'upstream_extra': 'retain me', 'words': [{'word': 'Привет,', 'start': 0.0, 'end': 0.5, 'confidence': 0.9, 'speaker': 0}, {'word': 'мир!', 'start': 0.5, 'end': min(1.0, duration), 'confidence': 0.8, 'speaker': 0}]}
        if os.environ.get('FAKE_CAPTURE_PCM'):
            import hashlib
            with wave.open(str(audio), 'rb') as wav:
                result['fixture_pcm_sha256'] = hashlib.sha256(wav.readframes(wav.getnframes())).hexdigest()
        if os.environ.get('FAKE_SLEEP'):
            import time
            time.sleep(float(os.environ['FAKE_SLEEP']))
        if audio.stem == os.environ.get('FAKE_INVALID_CHUNK'):
            target.write_text('{incomplete JSON')
        else:
            target.write_text(json.dumps(result, ensure_ascii=False))
        if os.environ.get('FAKE_MEMORY_AFTER_FIRST'):
            import time
            pressure = bytearray(96 * 1024 * 1024)
            time.sleep(10)
    if os.environ.get('FAKE_EXTRA_OUTPUT'):
        (pathlib.Path(args[2]) / 'unexpected.json').write_text('{}')
    if os.environ.get('FAKE_OVERSIZED_OUTPUT'):
        with (pathlib.Path(args[2]) / '000001.json').open('wb') as output:
            output.truncate(16 * 1024 * 1024 + 1)
    if os.environ.get('FAKE_CHANGE_RETAINED_RAW'):
        with (root / 'run' / 'raw' / '000000.json').open('ab') as retained:
            retained.write(b' ')
    if failed: sys.exit(7)
else:
    sys.exit(90)
"#;

/// Child entry point: inject a real loopback HTTP server into the same CLI logic.
#[test]
fn pipeline_cli_fixture() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::{Arc, atomic::AtomicBool},
    };
    let Ok(encoded) = std::env::var("AGENT_YOUTUBE_TEST_ARGS") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_TEST_ROOT").unwrap());
    let args: Vec<String> = serde_json::from_str(&encoded).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let base = origin.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let path = line
                .split_whitespace()
                .nth(1)
                .unwrap()
                .split('?')
                .next()
                .unwrap()
                .to_owned();
            let mut headers = std::collections::BTreeMap::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.insert(k.to_ascii_lowercase(), v.trim().to_owned());
            }
            let len = headers
                .get("content-length")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let range = headers.get("range").cloned().unwrap_or_default();
            let log = serde_json::json!({"tool":"http","args":[path],"range":range,"at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64});
            let mut log_file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(root.join("calls.jsonl"))
                .unwrap();
            writeln!(log_file, "{log}").unwrap();
            let mut status = 200;
            let mut extra = String::new();
            let bytes = if std::env::var_os("FAKE_DENY_NETWORK").is_some() {
                status = 503;
                Vec::new()
            } else if path == "/youtubei/v1/player" {
                let source = std::env::var_os("FAKE_MEDIA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("input.m4a"));
                let len = fs::metadata(source).unwrap().len();
                let media_url = if std::env::var_os("FAKE_UNTRUSTED_URL").is_some() {
                    "http://outside.invalid/audio".to_owned()
                } else if std::env::var_os("FAKE_SOURCE_LEASE").is_some() {
                    format!("{base}/audio?expire=4102444800")
                } else {
                    format!("{base}/audio")
                };
                let mut player = serde_json::json!({"playabilityStatus":{"status":"OK"},"videoDetails":{"videoId":if std::env::var_os("FAKE_WRONG_ID").is_some(){"wrongwrongxx".to_owned()}else{std::env::var("FAKE_VIDEO_ID").unwrap_or_else(|_|"a2hbJlPxj4M".into())},"title":"Русская запись"},"streamingData":{"adaptiveFormats":[{"itag":140,"lastModified":"12345","mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","bitrate":128000,"contentLength":len.to_string(),"url":media_url}]}});
                player["videoDetails"]["lengthSeconds"] = std::env::var("FAKE_DURATION")
                    .unwrap_or_else(|_| "12".into())
                    .into();
                if std::env::var("FAKE_DURATION")
                    .is_ok_and(|value| value == "absent" || value == "legacy")
                {
                    player["videoDetails"]
                        .as_object_mut()
                        .unwrap()
                        .remove("lengthSeconds");
                    if std::env::var("FAKE_DURATION").is_ok_and(|value| value == "legacy") {
                        player["duration"] = 12.into();
                    }
                }
                if let Ok(modified) = std::env::var("FAKE_LAST_MODIFIED") {
                    player["streamingData"]["adaptiveFormats"][0]["lastModified"] = modified.into();
                }
                if std::env::var_os("FAKE_SABR").is_some() {
                    player["streamingData"]["adaptiveFormats"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("url");
                    player["streamingData"]["adaptiveFormats"].as_array_mut().unwrap().push(serde_json::json!({"itag":160,"lastModified":"12345","mimeType":"video/mp4; codecs=\"avc1\"","bitrate":100000}));
                    player["streamingData"]["serverAbrStreamingUrl"] =
                        serde_json::json!(format!("{base}/sabr"));
                    player["playerConfig"] = serde_json::json!({"mediaCommonConfig":{"mediaUstreamerRequestConfig":{"videoPlaybackUstreamerConfig":"AQ"}}});
                }
                serde_json::to_vec(&player).unwrap()
            } else if (path == "/watch" || path.starts_with("/embed/"))
                && std::env::var_os("FAKE_LAST_MODIFIED").is_some()
            {
                b"var ytInitialPlayerResponse={\"playabilityStatus\":{\"status\":\"LOGIN_REQUIRED\",\"reason\":\"Sign in to confirm you are not a bot\",\"errorScreen\":{\"playerCaptchaViewModel\":{}}}};ytcfg.set({\"STS\":1,\"INNERTUBE_CONTEXT\":{\"client\":{\"clientName\":\"WEB\",\"visitorData\":\"fixture\"}},\"WEB_PLAYER_CONTEXT_CONFIGS\":{\"WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER\":{\"encryptedHostFlags\":\"fixture\"}}});".to_vec()
            } else if path == "/sabr" {
                let stalled = std::env::var_os("FAKE_SABR_STALL_UNTIL_RETRY").is_some()
                    && serde_json::from_slice::<serde_json::Value>(
                        &fs::read(root.join("run/job-retry.json")).unwrap(),
                    )
                    .unwrap()["attempts"]
                        == 0;
                let first = !root.join("sabr-stall-served").exists();
                if stalled {
                    fs::write(root.join("sabr-stall-served"), b"served").unwrap();
                }
                let indexed = std::env::var_os("FAKE_INDEXED_SABR").is_some();
                let segments = if indexed {
                    let counter = root.join("sabr-indexed-count");
                    let count = if counter.exists() {
                        fs::read_to_string(&counter)
                            .unwrap()
                            .parse::<usize>()
                            .unwrap()
                    } else {
                        0
                    };
                    fs::write(&counter, (count + 1).to_string()).unwrap();
                    count + 2
                } else if !stalled {
                    2
                } else if first {
                    1
                } else {
                    0
                };
                sabr_fixture(&fs::read(root.join("input.m4a")).unwrap(), segments)
            } else if path == "/audio"
                && std::env::var_os("FAKE_RETRY_COOLDOWN").is_some()
                && !root.join("cooldown-served").exists()
            {
                fs::write(root.join("cooldown-served"), b"served").unwrap();
                status = 429;
                extra = format!(
                    "Retry-After: {}\r\n",
                    std::env::var("FAKE_RETRY_COOLDOWN").unwrap()
                );
                Vec::new()
            } else if path == "/audio"
                && std::env::var_os("FAKE_RETRY_AFTER_PREFIX").is_some()
                && !range.starts_with("bytes=0-")
                && !root.join("prefix-cooldown-served").exists()
            {
                fs::write(root.join("prefix-cooldown-served"), b"served").unwrap();
                let state: serde_json::Value =
                    serde_json::from_slice(&fs::read(root.join("run/state.json")).unwrap())
                        .unwrap();
                let prefix = state["download_prefix"].as_str().unwrap();
                let receipt: serde_json::Value = serde_json::from_slice(
                    &fs::read(root.join("run").join(format!("{prefix}.json"))).unwrap(),
                )
                .unwrap();
                writeln!(log_file, "{}", serde_json::json!({"tool":"fault","args":["retained-prefix"],"bytes":receipt["progress"]["bytes"],"sha256":receipt["progress"]["digest"]})).unwrap();
                status = 429;
                extra = "Retry-After: 1\r\n".into();
                Vec::new()
            } else if path == "/audio" && std::env::var_os("FAKE_FAIL_DOWNLOAD").is_some() {
                status = 503;
                Vec::new()
            } else if path == "/audio" {
                let source = std::env::var_os("FAKE_MEDIA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("input.m4a"));
                let data = fs::read(source).unwrap();
                let (a, b) = range
                    .strip_prefix("bytes=")
                    .unwrap()
                    .split_once('-')
                    .unwrap();
                let a = a.parse::<usize>().unwrap();
                let b = b.parse::<usize>().unwrap();
                status = 206;
                extra = format!(
                    "Content-Range: bytes {a}-{b}/{}\r\nETag: \"original\"\r\n",
                    data.len()
                );
                data[a..=b].to_vec()
            } else {
                status = 404;
                Vec::new()
            };
            let header = format!(
                "HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",
                bytes.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&bytes);
        }
    });
    let downloader = agent_youtube::download::Downloader::loopback(&origin).unwrap();
    if let Err(error) = agent_youtube::cli::run_with_downloader(
        args.into_iter().map(Into::into).collect(),
        cancelled.clone(),
        downloader,
    ) {
        agent_youtube::protocol::error(
            error.as_ref(),
            cancelled.load(std::sync::atomic::Ordering::SeqCst),
        );
        std::process::exit(
            if cancelled.load(Ordering::SeqCst)
                && !error
                    .downcast_ref::<agent_youtube::protocol::Failure>()
                    .is_some_and(|e| e.code == "job_deadline_exceeded")
            {
                130
            } else {
                1
            },
        );
    }
}

fn sabr_fixture(audio: &[u8], segments: usize) -> Vec<u8> {
    fn var(out: &mut Vec<u8>, mut n: u64) {
        while n > 127 {
            out.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        out.push(n as u8);
    }
    fn num(out: &mut Vec<u8>, k: u64, n: u64) {
        var(out, k * 8);
        var(out, n);
    }
    fn blob(out: &mut Vec<u8>, k: u64, b: &[u8]) {
        var(out, k * 8 + 2);
        var(out, b.len() as u64);
        out.extend_from_slice(b);
    }
    fn part(out: &mut Vec<u8>, kind: u8, bytes: &[u8]) {
        out.push(kind);
        out.push(240);
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    let mut out = Vec::new();
    let mut format = Vec::new();
    num(&mut format, 1, 140);
    num(&mut format, 2, 12345);
    let mut meta = Vec::new();
    let video_id = std::env::var("FAKE_VIDEO_ID").unwrap_or_else(|_| "a2hbJlPxj4M".into());
    let duration_ms = std::env::var("FAKE_DURATION")
        .unwrap_or_else(|_| "12".into())
        .parse::<u64>()
        .unwrap()
        * 1000;
    blob(&mut meta, 1, video_id.as_bytes());
    blob(&mut meta, 2, &format);
    num(&mut meta, 3, duration_ms);
    let indexed = std::env::var_os("FAKE_INDEXED_SABR").is_some();
    num(
        &mut meta,
        4,
        if indexed {
            fmp4::ranges(audio).len() as u64 - 2
        } else {
            0
        },
    );
    blob(&mut meta, 5, b"audio/mp4; codecs=\"mp4a.40.2\"");
    part(&mut out, 42, &meta);
    let ranges = if indexed {
        fmp4::ranges(audio)
    } else {
        vec![0..32, 32..audio.len()]
    };
    for (id, range) in ranges.into_iter().enumerate().take(segments) {
        let offset = range.start as u64;
        let bytes = &audio[range];
        let id = id as u8;
        let mut h = Vec::new();
        num(&mut h, 1, u64::from(id));
        blob(&mut h, 2, video_id.as_bytes());
        num(&mut h, 3, 140);
        num(&mut h, 4, 12345);
        num(&mut h, 6, offset);
        num(&mut h, 8, u64::from(id == 0));
        num(
            &mut h,
            9,
            if indexed {
                u64::from(id.saturating_sub(1))
            } else {
                0
            },
        );
        num(
            &mut h,
            11,
            if indexed {
                u64::from(id.saturating_sub(1)) * 8000
            } else {
                0
            },
        );
        num(&mut h, 12, if indexed { 8000 } else { duration_ms });
        num(&mut h, 14, bytes.len() as u64);
        part(&mut out, 20, &h);
        let mut payload = vec![id];
        payload.extend_from_slice(bytes);
        part(&mut out, 21, &payload);
        part(&mut out, 22, &[id]);
    }
    out
}
#[test]
fn indexed_sabr_prefix_matches_full_pcm_and_resumes_without_http() {
    use sha2::{Digest, Sha256};
    let media = fmp4::fragmented(16000);
    let reference = Fixture::new();
    fs::write(reference.root.join("input.m4a"), &media).unwrap();
    failure(
        reference
            .run_command(URL)
            .env("FAKE_SABR", "1")
            .env("FAKE_INDEXED_SABR", "1")
            .env("FAKE_DURATION", "24")
            .env("FAKE_FAIL_CHUNK", "000000")
            .output()
            .unwrap(),
        "process_failed",
    );
    let expected = fs::read(reference.path("chunks/000000.wav")).unwrap();
    let data = expected
        .windows(4)
        .position(|part| part == b"data")
        .unwrap()
        + 8;
    let expected_hash = format!(
        "{:x}",
        Sha256::digest(&expected[data + 16016 * 2..data + 40016 * 2])
    );
    let fixture = Fixture::new();
    fs::write(fixture.root.join("input.m4a"), &media).unwrap();
    let failed = fixture
        .run_command(URL)
        .args(["--start", "1.001", "--end", "2.501"])
        .env("FAKE_SABR", "1")
        .env("FAKE_INDEXED_SABR", "1")
        .env("FAKE_DURATION", "24")
        .env("FAKE_FAIL_CHUNK", "000000")
        .output()
        .unwrap();
    assert_eq!(
        batch_json(&failed)["error"]["code"],
        "process_failed",
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(
        fs::read(fixture.path("chunks/000000.wav")).unwrap(),
        expected
    );
    assert!(!fixture.path("media/source.audio").exists());
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("download.json")).unwrap()).unwrap();
    assert_eq!(evidence["status"], "prefix_verified");
    let bytes = evidence["bytes"].as_u64().unwrap() as usize;
    assert!(bytes < media.len() * 3 / 4);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    let prefix = state["download_prefix"].as_str().unwrap();
    let path = fixture.path(&format!("{prefix}.m4a"));
    assert_eq!(fs::read(&path).unwrap(), media[..bytes]);
    let requests = fixture.calls("http", "/sabr").len();
    assert!(requests < reference.calls("http", "/sabr").len());
    let original = fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    fs::write(&path, &corrupt).unwrap();
    let saved_state = fs::read(fixture.path("state.json")).unwrap();
    let refused = Command::new("/usr/bin/python3")
        .arg(fixture.root.join("agent-youtube-test"))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .env("FAKE_DENY_NETWORK", "1")
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .output()
        .unwrap();
    assert_eq!(
        batch_json(&refused)["error"]["code"],
        "download_prefix_invalid"
    );
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert_eq!(fs::read(fixture.path("state.json")).unwrap(), saved_state);
    fs::write(&path, original).unwrap();
    let resumed = Command::new("/usr/bin/python3")
        .arg(fixture.root.join("agent-youtube-test"))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .env("FAKE_DENY_NETWORK", "1")
        .env("FAKE_CAPTURE_PCM", "1")
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(fixture.calls("http", "/sabr").len(), requests);
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("raw/000000.json")).unwrap()).unwrap();
    assert_eq!(raw["fixture_pcm_sha256"], expected_hash);
    let response = batch_json(&resumed);
    let range = &response["result"]["transcript"]["range"];
    assert_eq!(range["download"], "prefix");
    assert_eq!(range["downloaded_bytes"], bytes);
    assert_eq!(range["source_bytes"], media.len());
    assert!(
        !path.exists(),
        "owned source prefix must be cleaned after validated publication"
    );
    let mut documents = Vec::new();
    contract::collect(&failed, &mut documents);
    contract::collect(&resumed, &mut documents);
    for name in ["transcript.json", "coverage.json", "manifest.json"] {
        documents.push(serde_json::from_slice(&fs::read(fixture.path(name)).unwrap()).unwrap());
    }
    contract::validate(&documents);
}

#[test]
fn indexed_prefix_with_insufficient_pcm_falls_back_without_adopting_candidate_chunks() {
    let fixture = Fixture::new();
    let mut media = fmp4::fragmented(16000);
    let index = media.windows(4).position(|w| w == b"sidx").unwrap();
    let count = u16::from_be_bytes(media[index + 26..index + 28].try_into().unwrap()) as usize;
    for entry in 0..count {
        let at = index + 28 + entry * 12 + 4;
        let duration = u32::from_be_bytes(media[at..at + 4].try_into().unwrap());
        media[at..at + 4].copy_from_slice(&(duration * 2).to_be_bytes());
    }
    fs::write(fixture.root.join("input.m4a"), &media).unwrap();
    let output = fixture
        .run_command(URL)
        .args(["--start", "1", "--end", "12"])
        .env("FAKE_SABR", "1")
        .env("FAKE_INDEXED_SABR", "1")
        .env("FAKE_DURATION", "24")
        .env("FAKE_FAIL_CHUNK", "000000")
        .output()
        .unwrap();
    assert_eq!(
        batch_json(&output)["error"]["code"],
        "process_failed",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("range_full_source_required"));
    assert_eq!(fs::read(fixture.path("media/source.audio")).unwrap(), media);
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("download.json")).unwrap()).unwrap();
    assert_eq!(evidence["status"], "verified");
    assert!(
        fs::read_dir(fixture.path("chunks"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".agent-youtube-")),
        "failed candidate media must remain owned and unadopted"
    );
}

#[test]
fn sabr_pipeline_preserves_exact_aac_and_resumes_recognition() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_SABR", "1")
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    assert_eq!(
        fs::read(fixture.path("media/source.audio")).unwrap(),
        fs::read(fixture.root.join("input.m4a")).unwrap()
    );
    assert_eq!(fixture.calls("http", "/sabr").len(), 1);
    success(fixture.run());
    assert!(fixture.path("transcript.json").is_file());
    assert!(!fixture.path("media/source.audio").exists());
}

#[test]
fn source_leases_survive_asr_failure_and_cleanup_only_after_publication() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_SOURCE_LEASE", "1")
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    assert!(!fixture.path("transcript.json").exists());
    let leases: Vec<PathBuf> = fs::read_dir(fixture.path("media"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".lease-")
        })
        .collect();
    assert_eq!(leases.len(), 1);
    assert_eq!(
        fs::metadata(&leases[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let requests = fixture.calls("http", "/youtubei/v1/player").len();
    fs::write(fixture.path("media/user-source.lease"), b"unowned").unwrap();
    success(fixture.run());
    assert!(fixture.path("transcript.json").is_file());
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(fixture.calls("http", "/youtubei/v1/player").len(), requests);
    assert!(leases.iter().all(|path| !path.exists()));
    assert_eq!(
        fs::read(fixture.path("media/user-source.lease")).unwrap(),
        b"unowned"
    );
}

#[test]
fn incomplete_presentation_retains_source_without_transcript_or_recognition() {
    for ranged in [false, true] {
        let fixture = Fixture::new();
        let mut input = fs::read(fixture.root.join("input.m4a")).unwrap();
        let pos = input.windows(4).position(|w| w == b"elst").unwrap();
        assert_eq!(input[pos + 4], 0);
        let duration = u32::from_be_bytes(input[pos + 12..pos + 16].try_into().unwrap());
        input[pos + 12..pos + 16].copy_from_slice(&(duration * 2).to_be_bytes());
        fs::write(fixture.root.join("input.m4a"), &input).unwrap();
        let mut command = fixture.run_command(URL);
        if ranged {
            command.args(["--start", "0", "--end", "1"]);
        }
        let output = command.output().unwrap();
        if ranged {
            assert!(!output.status.success());
            let error = batch_json(&output);
            assert_eq!(error["error"]["code"], "audio_duration_mismatch");
            assert_eq!(
                error["error"]["details"]["source_frames_basis"],
                "container_index"
            );
            assert_eq!(error["error"]["details"]["indexed_seconds"], 24.0);
            assert!(error["error"]["details"].get("decoded_seconds").is_none());
        } else {
            failure(output, "incomplete M4A presentation");
        }
        assert_eq!(fs::read(fixture.path("media/source.audio")).unwrap(), input);
        assert!(!fixture.path("media/audio.wav").exists());
        assert!(!fixture.path("transcript.json").exists());
        assert!(fixture.calls("gigastt", "--output").is_empty());
    }
}

#[test]
fn batch_reuses_one_engine_and_only_retries_pending_files() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000000")
            .output()
            .unwrap(),
        "process_failed",
    );
    assert!(
        fixture.path("raw/000001.json").exists(),
        "later success must survive earlier failure"
    );
    assert!(!fixture.path("raw/000000.json").exists());
    assert_eq!(
        fixture
            .calls("gigastt", "transcribe-batch")
            .iter()
            .filter(|c| !c["args"].as_array().unwrap().iter().any(|v| v == "--help"))
            .count(),
        1
    );
    let raw = fs::read(fixture.path("raw/000001.json")).unwrap();
    success(fixture.run());
    assert_eq!(
        fixture
            .calls("gigastt", "transcribe-batch")
            .iter()
            .filter(|c| !c["args"].as_array().unwrap().iter().any(|v| v == "--help"))
            .count(),
        2
    );
    assert_eq!(fixture.calls("gigastt", "--output").len(), 3);
    assert_eq!(raw, fs::read(fixture.path("raw/000001.json")).unwrap());
    assert!(!fs::read_dir(fixture.path("")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("batch.")
    }));
}

#[test]
fn json_job_cancel_preserves_media_and_saved_options_resume_across_processes() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let child = fixture
        .run_command(URL)
        .env("FAKE_SLEEP", "10")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !fs::read_to_string(fixture.root.join("calls.jsonl"))
        .unwrap_or_default()
        .contains("fixture-file")
    {
        assert!(Instant::now() < deadline, "ASR did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let control = |name: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .arg(name)
            .arg("--work-dir")
            .arg(fixture.path(""))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    assert_eq!(control("status")["result"]["state"], "running");
    assert_eq!(control("cancel")["result"]["cancel_requested"], true);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(fixture.path("media/source.audio").is_file());
    assert_eq!(control("status")["result"]["state"], "resumable");
    let output = Command::new("/usr/bin/python3")
        .arg(fixture.root.join("agent-youtube-test"))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .arg("resume")
        .arg("--work-dir")
        .arg(fixture.path(""))
        .output()
        .unwrap();
    success(output);
    assert_eq!(control("status")["result"]["state"], "completed");
    assert_eq!(control("status")["result"]["transcript_validated"], true);
    assert_eq!(control("cancel")["result"]["cancel_requested"], false);
    fs::write(fixture.path("transcript.json"), b"tampered").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["status", "--work-dir"])
        .arg(fixture.path(""))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "checkpoint_integrity_mismatch");
}

#[test]
fn asr_deadline_preserves_audio_for_resume() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--asr-timeout-seconds", "1"])
        .env("FAKE_SLEEP", "10")
        .output()
        .unwrap();
    failure(output, "exceeded 1 seconds");
    assert!(fixture.path("media/source.audio").exists());
    success(fixture.run());
}

#[test]
fn manifest_keeps_source_and_raw_evidence_after_media_cleanup() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    success(fixture.run());
    let manifest_path = fixture.path("manifest.json");
    let bytes = fs::read(&manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(fixture.root.join("input.m4a")).unwrap())
    );
    assert_eq!(manifest["evidence"]["download"]["sha256"], digest);
    assert_eq!(
        manifest["evidence"]["download"]["representation"]["itag"],
        140
    );
    assert_eq!(manifest["chunks"].as_array().unwrap().len(), 2);
    for chunk in manifest["chunks"].as_array().unwrap() {
        let raw = fs::read(fixture.path(chunk["path"].as_str().unwrap())).unwrap();
        assert_eq!(chunk["sha256"], format!("{:x}", Sha256::digest(raw)));
    }
    assert!(!fixture.path("media/source.audio").exists());
    success(
        fixture
            .run_command(URL)
            .env("PATH", "/nonexistent")
            .output()
            .unwrap_or_else(|e| panic!("{e}")),
    );
    assert_eq!(bytes, fs::read(&manifest_path).unwrap());
    fs::write(manifest_path, b"{}").unwrap();
    failure(fixture.run(), "checkpoint integrity mismatch");
}

#[test]
fn completed_reuse_rejects_damaged_retained_evidence_without_running_tools() {
    for name in ["raw/000000.json", "source.json", "download.json"] {
        for missing in [false, true] {
            let fixture = Fixture::new();
            success(fixture.run());
            let transcript = fs::read(fixture.path("transcript.json")).unwrap();
            let manifest = fs::read(fixture.path("manifest.json")).unwrap();
            let calls = fs::read(fixture.root.join("calls.jsonl")).unwrap();
            // Model a completed publication whose media cleanup was interrupted.
            let media = fixture.path("media/source.audio");
            fs::write(&media, b"owned media").unwrap();
            let mut state: serde_json::Value =
                serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
            state["artifacts"]["media/source.audio"] =
                serde_json::json!(agent_youtube::storage::digest(&media).unwrap());
            fs::write(
                fixture.path("state.json"),
                serde_json::to_vec(&state).unwrap(),
            )
            .unwrap();
            if missing {
                fs::remove_file(fixture.path(name)).unwrap();
            } else {
                fs::write(fixture.path(name), b"damaged evidence").unwrap();
            }
            failure(fixture.run(), "checkpoint integrity mismatch");
            assert_eq!(
                fs::read(fixture.path("transcript.json")).unwrap(),
                transcript
            );
            assert_eq!(fs::read(fixture.path("manifest.json")).unwrap(), manifest);
            assert_eq!(fs::read(fixture.root.join("calls.jsonl")).unwrap(), calls);
            assert_eq!(fs::read(media).unwrap(), b"owned media");
            if !missing {
                assert_eq!(fs::read(fixture.path(name)).unwrap(), b"damaged evidence");
            }
        }
    }
}

#[test]
fn completed_chunk_resets_asr_inactivity_deadline() {
    let fixture = Fixture::new();
    success(
        fixture
            .run_command(URL)
            .args(["--asr-timeout-seconds", "1"])
            .env("FAKE_SLEEP", "0.7")
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
}

#[test]
fn failed_batch_media_stays_owned_until_durable_publication() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    let namespace = state["batch"]["namespace"].as_str().unwrap();
    let input = fixture.path(namespace).join("input/000000.wav");
    assert!(input.exists());
    // Refuse unknown entries rather than recursively cleaning an owned directory.
    let unknown = fixture.path(namespace).join("user-data");
    fs::write(&unknown, b"keep").unwrap();
    let output = fixture.run();
    failure(output.clone(), "unowned batch entry");
    let result: serde_json::Value = serde_json::from_slice(
        &output.stdout[output.stdout.iter().position(|b| *b == b'{').unwrap()..],
    )
    .unwrap();
    assert_eq!(result["error"]["code"], "cleanup_pending");
    assert_eq!(result["error"]["details"]["transcript_published"], true);
    assert!(fixture.path("transcript.json").exists());
    assert!(input.exists());
    assert_eq!(fs::read(&unknown).unwrap(), b"keep");
    fs::remove_file(unknown).unwrap();
    success(fixture.run());
    assert!(!input.exists());
}

#[test]
fn partial_batch_json_never_authorizes_cleanup_or_discards_valid_raw() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_INVALID_CHUNK", "000001")
            .output()
            .unwrap(),
        "batch did not produce every valid ASR result",
    );
    let saved = fs::read(fixture.path("raw/000000.json")).unwrap();
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    let partial = fixture
        .path(state["batch"]["namespace"].as_str().unwrap())
        .join("output/000001.json");
    assert_eq!(fs::read(&partial).unwrap(), b"{incomplete JSON");
    assert!(!fixture.path("transcript.json").exists());
    assert!(fixture.path("media/source.audio").exists());
    success(fixture.run());
    assert_eq!(saved, fs::read(fixture.path("raw/000000.json")).unwrap());
    assert!(!partial.exists());
    assert_eq!(fixture.calls("gigastt", "--output").len(), 3);
}

#[test]
fn final_asr_reconciliation_rejects_unexpected_and_oversized_outputs() {
    for (flag, message) in [
        ("FAKE_EXTRA_OUTPUT", "unexpected ASR batch output"),
        ("FAKE_OVERSIZED_OUTPUT", "ASR result exceeded 16 MiB"),
    ] {
        let fixture = Fixture::new();
        failure(
            fixture.run_command(URL).env(flag, "1").output().unwrap(),
            message,
        );
        assert!(!fixture.path("transcript.json").exists());
        assert!(fixture.path("media/source.audio").exists());
        assert!(fixture.path("chunks/000000.wav").exists());
    }
}

#[test]
fn unfinished_full_wav_run_reuses_raw_and_prepares_only_missing_chunks() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let mut pcm = ryf::read_s16(fixture.path("chunks/000000.wav")).unwrap().1;
    pcm.extend(ryf::read_s16(fixture.path("chunks/000001.wav")).unwrap().1);
    ryf::write_s16(&fixture.path("media/audio.wav"), &pcm, 16000).unwrap();
    let path = fixture.path("state.json");
    let mut state: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["artifacts"]["media/audio.wav"] = serde_json::json!(
        agent_youtube::storage::digest(&fixture.path("media/audio.wav")).unwrap()
    );
    state["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove("media/audio.json");
    state["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove("chunks/000001.wav");
    fs::remove_file(fixture.path("media/audio.json")).unwrap();
    fs::remove_file(fixture.path("chunks/000001.wav")).unwrap();
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    success(fixture.run());
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 3);
    assert!(!fixture.path("media/audio.wav").exists());
}

#[test]
fn agent_checkpoint_codes_distinguish_corruption_settings_and_missing_chunks() {
    for damage in ["snapshot", "journal", "settings", "missing", "unregistered"] {
        let fixture = Fixture::new();
        failure(
            fixture
                .run_command(URL)
                .env("FAKE_FAIL_CHUNK", "000001")
                .output()
                .unwrap(),
            "process_failed",
        );
        let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
        let source = fs::read(fixture.path("media/source.audio")).unwrap();
        let state_path = fixture.path("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        let (code, path) = match damage {
            "snapshot" => {
                fs::write(&state_path, b"{broken").unwrap();
                ("checkpoint_invalid", state_path.clone())
            }
            "journal" => {
                let path = fixture
                    .path(state["journal"]["directory"].as_str().unwrap())
                    .join("head.json");
                fs::write(&path, b"{broken").unwrap();
                (
                    "checkpoint_journal_invalid",
                    fixture.path(state["journal"]["directory"].as_str().unwrap()),
                )
            }
            "settings" => {
                state["config"]["chunk_seconds"] = serde_json::json!(20);
                fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
                ("checkpoint_settings_mismatch", state_path.clone())
            }
            "missing" => {
                fs::remove_file(fixture.path("chunks/000001.wav")).unwrap();
                (
                    "checkpoint_artifact_missing",
                    fixture.path("chunks/000001.wav"),
                )
            }
            _ => {
                state["artifacts"]
                    .as_object_mut()
                    .unwrap()
                    .remove("chunks/000001.wav");
                fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
                ("decoded_chunk_missing", fixture.path("chunks/000001.wav"))
            }
        };
        let state_before = fs::read(&state_path).unwrap();
        let output = fixture.run();
        failure_code(&output, code, &path);
        assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
        assert_eq!(
            fs::read(fixture.path("media/source.audio")).unwrap(),
            source
        );
        assert!(!fixture.path("transcript.json").exists());
        if matches!(damage, "snapshot" | "journal" | "settings") {
            assert_eq!(fs::read(&state_path).unwrap(), state_before);
        }
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn malformed_decoded_receipt_has_a_typed_error_and_preserves_evidence() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let source = fs::read(fixture.path("media/source.audio")).unwrap();
    let receipt = fixture.path("media/audio.json");
    for bytes in [
        b"{broken".as_slice(),
        b"{\"schema_version\":1,\"frames\":0,\"chunk_seconds\":10}",
    ] {
        fs::write(&receipt, bytes).unwrap();
        let path = fixture.path("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state["artifacts"]["media/audio.json"] =
            serde_json::json!(agent_youtube::storage::digest(&receipt).unwrap());
        fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
        failure_code(&fixture.run(), "decoded_receipt_invalid", &receipt);
        assert_eq!(fs::read(&receipt).unwrap(), bytes);
        assert_eq!(
            fs::read(fixture.path("media/source.audio")).unwrap(),
            source
        );
        assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
        assert!(!fixture.path("transcript.json").exists());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn constrained_address_space_reports_json_resource_failure_and_preserves_source() {
    use std::os::unix::process::CommandExt;
    let fixture = Fixture::new();
    let mut command = fixture.run_command(URL);
    // Limit only this fresh fixture process; never alter the developer session.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 128 << 20,
                rlim_max: 128 << 20,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let start = output
        .stdout
        .iter()
        .position(|byte| *byte == b'{')
        .unwrap_or_else(|| panic!("no JSON: {}", String::from_utf8_lossy(&output.stderr)));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout[start..]).unwrap();
    assert_eq!(result["error"]["code"], "resource_limit");
    assert_eq!(result["error"]["retryable"], true);
    assert!(fixture.path("media/source.audio").exists());
    assert!(!fixture.path("transcript.json").exists());
    assert!(fixture.calls("gigastt", "--output").is_empty());
}

#[test]
fn exact_decoder_state_corruption_fails_without_changing_retained_evidence() {
    for damage in ["digest", "json", "version", "codec", "prefix"] {
        let fixture = Fixture::new();
        failure(
            fixture
                .run_command(URL)
                .env("FAKE_FAIL_CHUNK", "000001")
                .output()
                .unwrap(),
            "process_failed",
        );
        let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
        let source = fs::read(fixture.path("media/source.audio")).unwrap();
        let path = fixture.path("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state["artifacts"]
            .as_object_mut()
            .unwrap()
            .remove("media/audio.json");
        fs::remove_file(fixture.path("media/audio.json")).unwrap();
        let resume = fixture.path("chunks/000000.resume.json");
        if damage == "prefix" {
            state["artifacts"]
                .as_object_mut()
                .unwrap()
                .remove("chunks/000000.wav");
        } else if matches!(damage, "digest" | "json") {
            fs::write(&resume, b"{broken").unwrap();
        } else {
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&resume).unwrap()).unwrap();
            if damage == "version" {
                value["version"] = serde_json::json!(99);
            } else {
                value["codec"][0] = serde_json::json!(0);
            }
            fs::write(&resume, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        if damage != "digest" {
            state["artifacts"]["chunks/000000.resume.json"] =
                serde_json::json!(agent_youtube::storage::digest(&resume).unwrap());
        }
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let bytes = fs::read(&resume).unwrap();
        failure_code(
            &fixture.run(),
            if damage == "digest" {
                "checkpoint_integrity_mismatch"
            } else if damage == "prefix" {
                "decoded_chunk_missing"
            } else {
                "decode_resume_invalid"
            },
            &if damage == "prefix" {
                fixture.path("chunks/000000.wav")
            } else {
                resume.clone()
            },
        );
        assert_eq!(fs::read(resume).unwrap(), bytes);
        assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
        assert_eq!(
            fs::read(fixture.path("media/source.audio")).unwrap(),
            source
        );
        assert!(!fixture.path("transcript.json").exists());
        assert_eq!(fixture.calls("gigastt", "--output").len(), 2);
    }
}

#[test]
fn interrupted_decoder_state_publication_recovers_before_resuming_exact_tail() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
    let path = fixture.path("state.json");
    let mut state: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove("media/audio.json");
    fs::remove_file(fixture.path("media/audio.json")).unwrap();
    let name = "chunks/000000.resume.json";
    let digest = state["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove(name)
        .unwrap();
    let pending = "chunks/.agent-youtube-111-222-333-000000.resume.partial.json";
    fs::rename(fixture.path(name), fixture.path(pending)).unwrap();
    state["publications"][name] = serde_json::json!({"pending":pending,"digest":digest});
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    let output = fixture.run();
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("restoring exact decoder state after 1 completed chunks")
    );
    success(output);
    assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
    assert_eq!(fixture.calls("gigastt", "--output").len(), 3);
    assert!(!fixture.path(name).exists());
    assert!(!fixture.path(pending).exists());
}

#[test]
#[ignore = "cross-build migration refusal; use tools/qualify_codec_migration.py"]
fn candidate_backend_refuses_old_job_without_mutation() {
    let candidate = std::env::var("AGENT_YOUTUBE_CANDIDATE_PIPELINE").unwrap();
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let before = snapshot_job(&fixture.root.join("run"));
    let args = vec![
        "transcribe".to_owned(),
        URL.to_owned(),
        "--work-dir".to_owned(),
        fixture.root.join("run").display().to_string(),
        "--model-dir".to_owned(),
        fixture.root.join("models").display().to_string(),
        "--chunk-seconds".to_owned(),
        "10".to_owned(),
        "--gigastt".to_owned(),
        fixture.root.join("gigastt").display().to_string(),
    ];
    let output = Command::new(candidate)
        .args(["--exact", "pipeline_cli_fixture", "--nocapture"])
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .env(
            "AGENT_YOUTUBE_TEST_ARGS",
            serde_json::to_string(&args).unwrap(),
        )
        .output()
        .unwrap();
    failure_code(
        &output,
        "checkpoint_settings_mismatch",
        &fixture.path("state.json"),
    );
    let after = snapshot_job(&fixture.root.join("run"));
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    // Verify recoverability even if invocation bookkeeping violated immutability.
    success(fixture.run());
    assert!(
        changed.is_empty(),
        "candidate modified saved job paths: {changed:?}"
    );
}

fn snapshot_job(root: &Path) -> std::collections::BTreeMap<PathBuf, String> {
    use sha2::{Digest, Sha256};
    fn visit(base: &Path, at: &Path, files: &mut std::collections::BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn incompatible_backend_refuses_before_preflight_or_saved_job_mutation() {
    let fixture = Fixture::new();
    failure(
        fixture
            .run_command(URL)
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap(),
        "process_failed",
    );
    let path = fixture.path("state.json");
    let original = fs::read(&path).unwrap();
    let mut state: serde_json::Value = serde_json::from_slice(&original).unwrap();
    state["config"]["media_backend"] = "incompatible-backend".into();
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let before = snapshot_job(&fixture.root.join("run"));
    let calls = fs::read(fixture.root.join("calls.jsonl")).unwrap();
    let output = fixture.run();
    failure_code(&output, "checkpoint_settings_mismatch", &path);
    assert_eq!(snapshot_job(&fixture.root.join("run")), before);
    assert_eq!(fs::read(fixture.root.join("calls.jsonl")).unwrap(), calls);
    fs::write(path, original).unwrap();
    success(fixture.run());
}

#[test]
fn multilingual_models_are_selected_recorded_and_bound_to_resume() {
    for variant in ["ml_ctc", "ml_ctc_large"] {
        let fixture = Fixture::new();
        for name in [
            "multilingual_ctc.int8.onnx",
            "multilingual_large_ctc.int8.onnx",
            "multilingual_vocab.txt",
        ] {
            fs::write(
                fixture.root.join("models").join(name),
                b"fixture multilingual model",
            )
            .unwrap();
        }
        let failed = fixture
            .run_command(URL)
            .args(["--model-variant", variant])
            .env("FAKE_FAIL_CHUNK", "000001")
            .output()
            .unwrap();
        assert!(!failed.status.success());
        let raw = fs::read(fixture.path("raw/000000.json")).unwrap();
        let state = fs::read(fixture.path("state.json")).unwrap();
        let changed = fixture
            .run_command(URL)
            .args(["--model-variant", "e2e_rnnt"])
            .output()
            .unwrap();
        assert!(!changed.status.success());
        assert_eq!(fs::read(fixture.path("state.json")).unwrap(), state);
        let output = fixture
            .run_command(URL)
            .args(["--model-variant", variant])
            .output()
            .unwrap();
        assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), raw);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: serde_json::Value = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .unwrap();
        assert_eq!(
            value["result"]["transcript"]["engine"]["model_variant"],
            variant
        );
        assert_eq!(value["result"]["transcript"]["recognizer_language"], "mul");
        assert!(value["result"]["transcript"]["source_language"].is_null());
        let calls = fixture.calls("gigastt", "transcribe-batch");
        assert!(calls.iter().any(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == variant)
        }));
        let before = fs::read(fixture.path("transcript.json")).unwrap();
        let output = fixture
            .run_command(URL)
            .args(["--model-variant", "e2e_rnnt"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), before);
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .args(["resume", "--work-dir"])
            .arg(fixture.path(""))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["result"]["transcript"]["engine"]["model_variant"],
            variant
        );
        contract::validate(&[value]);
    }
}

#[test]
#[ignore = "opt-in complete cached real recordings, real multilingual models and cancellation/resume"]
fn research_multilingual_recordings_resume_with_real_gigastt() {
    use std::time::{Duration, Instant};
    let base = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_MULTILINGUAL_PROBE").unwrap());
    assert!(!base.exists());
    assert!(
        base.parent().unwrap().canonicalize().unwrap().starts_with(
            std::env::current_dir()
                .unwrap()
                .join("runs")
                .canonicalize()
                .unwrap()
        )
    );
    fs::create_dir(&base).unwrap();
    let models = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_MULTILINGUAL_MODELS").unwrap())
        .canonicalize()
        .unwrap();
    let engine = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_MULTILINGUAL_ENGINE").unwrap())
        .canonicalize()
        .unwrap();
    let mut reports = Vec::new();
    for (label, variant, id, duration, env) in [
        (
            "english",
            "ml_ctc",
            "jNQXAC9IVRw",
            "19",
            "AGENT_YOUTUBE_MULTILINGUAL_ENGLISH",
        ),
        (
            "russian",
            "ml_ctc_large",
            "of4B3aw7MXo",
            "53",
            "AGENT_YOUTUBE_MULTILINGUAL_RUSSIAN",
        ),
    ] {
        let source = PathBuf::from(std::env::var_os(env).unwrap());
        let fixture = Fixture::at(base.join(label), true);
        fs::copy(&source, fixture.root.join("input.m4a")).unwrap();
        let arguments = vec![
            "transcribe".to_owned(),
            format!("https://www.youtube.com/watch?v={id}"),
            "--work-dir".into(),
            fixture.path("").to_str().unwrap().into(),
            "--gigastt".into(),
            engine.to_str().unwrap().into(),
            "--model-dir".into(),
            models.to_str().unwrap().into(),
            "--model-variant".into(),
            variant.into(),
            "--chunk-seconds".into(),
            "10".into(),
            "--encoder-threads".into(),
            "2".into(),
            "--retry-window-seconds".into(),
            "0".into(),
            "--response".into(),
            "metadata".into(),
        ];
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pipeline_cli_fixture", "--nocapture"])
            .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
            .env(
                "AGENT_YOUTUBE_TEST_ARGS",
                serde_json::to_string(&arguments).unwrap(),
            )
            .env("FAKE_VIDEO_ID", id)
            .env("FAKE_DURATION", duration)
            .stdout(fs::File::create(fixture.root.join("initial.stdout")).unwrap())
            .stderr(fs::File::create(fixture.root.join("initial.stderr")).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        let first = loop {
            let candidate = fs::read_dir(fixture.path(""))
                .unwrap_or_else(|_| fs::read_dir(&fixture.root).unwrap())
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("batch.")
                })
                .map(|p| p.join("output/000000.json"))
                .find(|p| p.exists());
            if let Some(path) = candidate
                && let Ok(bytes) = fs::read(&path)
                && serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()
            {
                break bytes;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "initial run ended before interruption; inspect retained probe"
            );
            assert!(
                Instant::now() < deadline,
                "multilingual recognition timed out; inspect retained probe"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        // The owned fixture installs the same cancellation handler as the CLI.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        let status = child.wait().unwrap();
        assert!(!status.success());
        let output = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .args(["resume", "--work-dir"])
            .arg(fixture.path(""))
            .args(["--response", "metadata"])
            .output()
            .unwrap();
        fs::write(fixture.root.join("resume.stdout"), &output.stdout).unwrap();
        fs::write(fixture.root.join("resume.stderr"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(fs::read(fixture.path("raw/000000.json")).unwrap(), first);
        let transcript: agent_youtube::transcript::Transcript =
            serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
        transcript.validate().unwrap();
        assert_eq!(transcript.engine.model_variant, variant);
        assert_eq!(transcript.language, "mul");
        assert!(
            transcript
                .segments
                .iter()
                .any(|s| !s.text.trim().is_empty())
        );
        assert!(!fixture.path("media/source.audio").exists());
        reports.push(serde_json::json!({"label":label,"model_variant":variant,"video_id":id,"duration_seconds":transcript.duration,
            "input_sha256":agent_youtube::storage::digest(&source).unwrap(),"complete_recording":true,
            "controlled_loopback_acquisition":true,"fresh_youtube_download":false,"real_gigastt":true,
            "interrupted":true,"resumed":true,"first_raw_preserved":true,"transcript_validated":true,"cleanup_complete":true}));
        fs::write(
            base.join("report.json"),
            serde_json::to_vec_pretty(&reports).unwrap(),
        )
        .unwrap();
    }
}

fn batch_command(fixture: &Fixture, input: &Path) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command
        .arg(fixture.root.join("agent-youtube-test"))
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .arg("batch")
        .arg(input)
        .arg("--work-dir")
        .arg(fixture.root.join("batch"))
        .arg("--model-dir")
        .arg(fixture.root.join("models"))
        .arg("--gigastt")
        .arg(fixture.root.join("gigastt"))
        .args([
            "--chunk-seconds",
            "10",
            "--retry-window-seconds",
            "0",
            "--jobs",
            "2",
        ]);
    command
}

#[test]
fn batch_isolates_failures_and_reuses_verified_jobs_across_processes() {
    let fixture = Fixture::new();
    let input = fixture.root.join("items.jsonl");
    fs::write(&input, format!("{{\"id\":\"../lecture\",\"url\":{URL:?}}}\n{{\"id\":\"bad\",\"url\":\"https://example.com/private?token=secret\"}}\n{{\"id\":\"second\",\"url\":{URL:?}}}\n")).unwrap();
    let first = batch_command(&fixture, &input).output().unwrap();
    assert!(!first.status.success());
    let value: serde_json::Value = batch_json(&first);
    assert_eq!(value["error"]["code"], "batch_incomplete", "{value}");
    assert_eq!(value["error"]["retryable"], false);
    let batch = &value["error"]["details"]["batch"];
    assert_eq!(batch["succeeded"], 2);
    assert_eq!(batch["failed"], 1);
    assert_eq!(batch["items"][0]["id"], "../lecture");
    assert_eq!(batch["items"][1]["error"]["code"], "batch_url_invalid");
    assert!(!String::from_utf8_lossy(&first.stdout).contains("token=secret"));
    assert!(!fixture.root.join("lecture").exists());
    for item in [0, 2] {
        let dir = PathBuf::from(batch["items"][item]["work_dir"].as_str().unwrap());
        assert!(dir.join("transcript.json").is_file());
        assert_eq!(batch["items"][item]["reused"], false);
    }
    let before = batch_asr_calls(&fixture);
    let again = batch_command(&fixture, &input).output().unwrap();
    let again: serde_json::Value = batch_json(&again);
    let saved = &again["error"]["details"]["batch"];
    assert_eq!(saved["succeeded"], 2, "{again}");
    assert_eq!(saved["items"][0]["reused"], true);
    assert_eq!(batch_asr_calls(&fixture), before);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("batch/batch-result.json")).unwrap())
            .unwrap();
    assert_eq!(&report, saved);
    let mut documents = Vec::new();
    contract::collect(&first, &mut documents);
    contract::validate(&documents);
    fs::write(&input, format!("{{\"id\":\"changed\",\"url\":{URL:?}}}\n")).unwrap();
    let changed = batch_command(&fixture, &input).output().unwrap();
    let changed: serde_json::Value = batch_json(&changed);
    assert_eq!(changed["error"]["code"], "batch_identity_mismatch");
    assert_eq!(batch_asr_calls(&fixture), before);
}

fn batch_json(output: &Output) -> serde_json::Value {
    let start = output.stdout.iter().position(|b| *b == b'{').unwrap();
    serde_json::Deserializer::from_slice(&output.stdout[start..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap()
}

#[test]
fn batch_serializes_real_recognizer_processes_and_reports_success_contract() {
    let fixture = Fixture::new();
    let input = fixture.root.join("items.jsonl");
    fs::write(
        &input,
        format!("{{\"id\":\"one\",\"url\":{URL:?}}}\n{{\"id\":\"two\",\"url\":{URL:?}}}\n"),
    )
    .unwrap();
    let output = batch_command(&fixture, &input)
        .env("FAKE_ASR_EXCLUSIVE", "1")
        .env("FAKE_SLEEP", "0.1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = batch_json(&output);
    assert_eq!(value["result"]["succeeded"], 2);
    assert_eq!(value["result"]["recognition_workers"], 1);
    assert_eq!(batch_asr_calls(&fixture), 2);
    let mut documents = Vec::new();
    contract::collect(&output, &mut documents);
    contract::validate(&documents);
}

#[test]
fn cancelling_one_batch_job_does_not_cancel_its_sibling() {
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    let input = fixture.root.join("items.jsonl");
    fs::write(
        &input,
        format!("{{\"id\":\"one\",\"url\":{URL:?}}}\n{{\"id\":\"two\",\"url\":{URL:?}}}\n"),
    )
    .unwrap();
    let mut command = batch_command(&fixture, &input);
    command
        .env("FAKE_SLEEP", "0.5")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let job = loop {
        let call = fixture.calls("gigastt", "fixture-file").into_iter().next();
        if let Some(call) = call {
            let audio = PathBuf::from(call["args"][1].as_str().unwrap());
            break audio
                .ancestors()
                .find(|p| p.join("job.json").is_file())
                .unwrap()
                .to_path_buf();
        }
        assert!(Instant::now() < deadline, "recognizer did not start");
        std::thread::sleep(Duration::from_millis(20));
    };
    let cancelled = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .arg("cancel")
        .arg("--work-dir")
        .arg(&job)
        .output()
        .unwrap();
    assert!(cancelled.status.success());
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    let value = batch_json(&result);
    let batch = &value["error"]["details"]["batch"];
    assert_eq!(batch["succeeded"], 1, "{value}");
    assert_eq!(batch["failed"], 1);
    assert_eq!(batch["state"], "partial");
    let calls = batch_asr_calls(&fixture);
    let again = batch_command(&fixture, &input).output().unwrap();
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(batch_asr_calls(&fixture), calls + 1);
}

fn batch_asr_calls(fixture: &Fixture) -> usize {
    fixture
        .calls("gigastt", "transcribe-batch")
        .into_iter()
        .filter(|call| {
            !call["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a == "--help")
        })
        .count()
}

#[test]
fn interrupted_batch_preserves_jobs_and_resumes_after_process_restart() {
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    let input = fixture.root.join("items.jsonl");
    fs::write(
        &input,
        (0..3)
            .map(|id| format!("{{\"id\":\"{id}\",\"url\":{URL:?}}}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let child = batch_command(&fixture, &input)
        .env("FAKE_SLEEP", "10")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while fixture.calls("gigastt", "fixture-file").is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    let value = batch_json(&output);
    assert_eq!(value["error"]["code"], "interrupted");
    let report = &value["error"]["details"]["batch"];
    assert_eq!(report["state"], "interrupted");
    assert!(report["pending"].as_u64().unwrap() >= 1);
    let started: Vec<_> = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["state"] == "failed")
        .collect();
    assert!(!started.is_empty());
    assert!(started.iter().any(|item| {
        PathBuf::from(item["work_dir"].as_str().unwrap())
            .join("media/source.audio")
            .is_file()
    }));
    let resumed = batch_command(&fixture, &input).output().unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(batch_json(&resumed)["result"]["succeeded"], 3);
}

#[test]
#[ignore = "requires an explicitly supplied older CLI to verify unsafe downgrade refusal"]
fn older_cli_rejects_prefix_checkpoint_without_changing_owned_media() {
    let older = std::env::var_os("AGENT_YOUTUBE_PREVIOUS_CLI").unwrap();
    let fixture = Fixture::new();
    let failed = fixture
        .run_command(URL)
        .args(["--start", "1", "--end", "2"])
        .env("FAKE_FAIL_CHUNK", "000000")
        .output()
        .unwrap();
    assert_eq!(batch_json(&failed)["error"]["code"], "process_failed");
    let mut before = Vec::new();
    for name in [
        "state.json",
        "job.json",
        "media/source.audio",
        "media/audio.json",
        "chunks/000000.wav",
    ] {
        before.push((name, fs::read(fixture.path(name)).unwrap()));
    }
    let refused = Command::new(older)
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let response: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(response["error"]["code"], "checkpoint_invalid");
    for (name, bytes) in before {
        assert_eq!(
            fs::read(fixture.path(name)).unwrap(),
            bytes,
            "older CLI changed {name}"
        );
    }
    assert!(!fixture.path("transcript.json").exists());
}

#[test]
fn selected_prefix_reports_indexed_duration_and_resumes_in_another_process() {
    let fixture = Fixture::new();
    let failed = fixture
        .run_command(URL)
        .args(["--start", "1.001", "--end", "2.501"])
        .env("FAKE_FAIL_CHUNK", "000000")
        .output()
        .unwrap();
    assert_eq!(batch_json(&failed)["error"]["code"], "process_failed");
    let decoded: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("media/audio.json")).unwrap()).unwrap();
    assert_eq!(decoded["schema_version"], 2);
    assert_eq!(decoded["prefix_frames"], 160000);
    assert_eq!(decoded["frames"], 192000);
    assert!(!fixture.path("chunks/000001.wav").exists());
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert_eq!(state["schema_version"], 6);
    let resumed = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let summary = &response["result"]["transcript"]["range"];
    assert_eq!(summary["decode"], "prefix");
    assert_eq!(summary["download"], "full_audio");
    assert_eq!(summary["source_frames_basis"], "container_index");
    assert_eq!(summary["decoded_through_seconds"], 10.0);
    let mut documents = Vec::new();
    contract::collect(&failed, &mut documents);
    contract::collect(&resumed, &mut documents);
    for name in ["transcript.json", "coverage.json"] {
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path(name)).unwrap()).unwrap();
        assert_eq!(document["schema_version"], 3);
        assert_eq!(document["range"]["decoded_prefix"]["through_frame"], 160000);
        assert_eq!(
            document["range"]["decoded_prefix"]["source_frames_basis"],
            "container_index"
        );
        documents.push(document);
    }
    contract::validate(&documents);
}

#[test]
fn ranged_jobs_keep_only_intersecting_fixed_grid_pcm() {
    let mut documents = Vec::new();
    for (start, end, indices, code) in [
        ("0", Some("10"), vec![0], "process_failed"),
        ("10", Some("11"), vec![1], "process_failed"),
        ("9.5", Some("10.5"), vec![0, 1], "process_failed"),
        ("10", None, vec![1], "process_failed"),
        ("13", None, vec![], "range_out_of_bounds"),
    ] {
        let fixture = Fixture::new();
        let mut command = fixture.run_command(URL);
        command.args(["--start", start]);
        if let Some(end) = end {
            command.args(["--end", end]);
        }
        let failed = command.env("FAKE_FAIL_CHUNK", "000000").output().unwrap();
        assert_eq!(batch_json(&failed)["error"]["code"], code);
        for index in 0..2 {
            assert_eq!(
                fixture.path(&format!("chunks/{index:06}.wav")).exists(),
                indices.contains(&index),
                "range {start}..{end:?}, PCM chunk {index}"
            );
        }
        let decoded: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path("media/audio.json")).unwrap()).unwrap();
        assert_eq!(decoded["frames"], 192000);
        assert_eq!(
            fs::read(fixture.path("media/source.audio")).unwrap(),
            fs::read(fixture.root.join("input.m4a")).unwrap()
        );
        if code == "process_failed" {
            let resumed = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
                .args(["resume", "--work-dir"])
                .arg(fixture.path(""))
                .output()
                .unwrap();
            assert!(
                resumed.status.success(),
                "{}",
                String::from_utf8_lossy(&resumed.stderr)
            );
            let response: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
            assert_eq!(
                response["result"]["transcript"]["range"]["download"],
                "full_audio"
            );
            contract::collect(&resumed, &mut documents);
        }
    }
    contract::validate(&documents);
}

#[test]
fn selected_range_uses_exact_cross_chunk_pcm_and_resumes_with_absolute_times() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let failed = fixture
        .run_command(URL)
        .args(["--start", "00:00:09", "--end", "11"])
        .env("FAKE_FAIL_CHUNK", "000000")
        .output()
        .unwrap();
    let failure = batch_json(&failed);
    assert_eq!(failure["error"]["code"], "process_failed", "{failure}");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("state.json")).unwrap()).unwrap();
    assert_eq!(state["schema_version"], 6);
    // This acquisition was decoded in full, as in legacy ranged schema5.
    // Reuse that format to exercise migration without changing audio/ASR data.
    state["schema_version"] = serde_json::json!(5);
    fs::write(
        fixture.path("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();

    let a = ryf::read(fixture.path("chunks/000000.wav")).unwrap();
    let b = ryf::read(fixture.path("chunks/000001.wav")).unwrap();
    let mut samples = a.channels[0][9 * 16000..].to_vec();
    samples.extend_from_slice(&b.channels[0][..16000]);
    let pcm = ryf::f32_to_s16le(&samples);
    let expected = format!("{:x}", Sha256::digest(pcm));
    let resumed = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["resume", "--work-dir"])
        .arg(fixture.path(""))
        .env("FAKE_CAPTURE_PCM", "1")
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("raw/000000.json")).unwrap()).unwrap();
    assert_eq!(raw["duration"], 2.0);
    assert_eq!(raw["fixture_pcm_sha256"], expected);
    assert!(!fixture.path("raw/000001.json").exists());
    let t: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("transcript.json")).unwrap()).unwrap();
    assert_eq!(t["schema_version"], 2);
    assert_eq!(t["segments"][0]["start"], 9.0);
    assert_eq!(t["range"]["start_frame"], 144000);
    assert_eq!(t["range"]["end_frame"], 176000);
    let response: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(
        response["result"]["transcript"]["range"]["start_seconds"],
        9.0
    );
    assert_eq!(
        response["result"]["transcript"]["coverage"]["frames"],
        32000
    );
    let original = fs::read(fixture.path("transcript.json")).unwrap();
    let invocation = fs::read(fixture.path("job.json")).unwrap();
    let changed = fixture
        .run_command(URL)
        .args(["--start", "8", "--end", "11"])
        .output()
        .unwrap();
    assert_eq!(
        batch_json(&changed)["error"]["code"],
        "checkpoint_settings_mismatch"
    );
    assert_eq!(fs::read(fixture.path("transcript.json")).unwrap(), original);
    assert_eq!(fs::read(fixture.path("job.json")).unwrap(), invocation);
    let no_range = fixture.run_command(URL).output().unwrap();
    assert_eq!(
        batch_json(&no_range)["error"]["code"],
        "checkpoint_settings_mismatch"
    );
    let mut documents = Vec::new();
    contract::collect(&resumed, &mut documents);
    contract::validate(&documents);
}

#[test]
fn range_end_clamps_to_decoded_audio_and_empty_selection_retains_media() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args(["--start", "10", "--end", "90", "--response", "full"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = batch_json(&output);
    let t = &response["result"]["transcript"];
    assert_eq!(t["range"]["requested"]["end_millis"], 90000);
    assert_eq!(t["range"]["end_frame"], 192000);
    assert_eq!(t["range"]["end_frame"], t["range"]["source_frames"]);
    assert_eq!(t["schema_version"], 2);
    assert_eq!(t["segments"][0]["start"], 10.0);
    let mut docs = Vec::new();
    contract::collect(&output, &mut docs);
    for name in ["transcript.json", "manifest.json", "coverage.json"] {
        docs.push(serde_json::from_slice(&fs::read(fixture.path(name)).unwrap()).unwrap());
    }
    contract::validate(&docs);
    let outside = Fixture::new();
    let failed = outside
        .run_command(URL)
        .args(["--start", "13"])
        .output()
        .unwrap();
    assert_eq!(batch_json(&failed)["error"]["code"], "range_out_of_bounds");
    assert!(outside.path("media/source.audio").is_file());
    assert!(outside.calls("gigastt", "fixture-file").is_empty());
}
#[test]
fn ranged_pause_boundaries_and_subtitle_exports_keep_original_video_time() {
    let fixture = Fixture::new();
    let output = fixture
        .run_command(URL)
        .args([
            "--start",
            "1.001",
            "--end",
            "11.501",
            "--boundary-window-ms",
            "1000",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let coverage: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("coverage.json")).unwrap()).unwrap();
    assert_eq!(coverage["chunks"], 2);
    assert_eq!(coverage["range"]["start_frame"], 16016);
    for format in ["json", "md", "srt", "vtt"] {
        let path = fixture.root.join(format!("ranged.{format}"));
        let exported = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
            .arg("export")
            .arg(fixture.path("transcript.json"))
            .arg("--format")
            .arg(format)
            .arg("--output")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            exported.status.success(),
            "{}",
            String::from_utf8_lossy(&exported.stderr)
        );
        let text = fs::read_to_string(path).unwrap();
        if format == "json" {
            let x: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(x["range"]["start_seconds"], 1.001);
        } else if format == "md" {
            assert!(text.contains("Recognition range: 1.001"));
        } else {
            assert!(text.contains(if format == "srt" {
                "00:00:01,001"
            } else {
                "00:00:01.001"
            }));
        }
    }
}

#[test]
fn batch_preserves_common_range_and_rejects_changed_bounds() {
    let fixture = Fixture::new();
    let input = fixture.root.join("ranged.jsonl");
    fs::write(
        &input,
        format!("{{\"id\":\"one\",\"url\":{URL:?}}}\n{{\"id\":\"two\",\"url\":{URL:?}}}\n"),
    )
    .unwrap();
    let output = batch_command(&fixture, &input)
        .args(["--start", "9", "--end", "11"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = batch_json(&output);
    assert_eq!(response["result"]["succeeded"], 2);
    for item in response["result"]["items"].as_array().unwrap() {
        assert_eq!(item["range"]["start_seconds"], 9.0);
        assert_eq!(item["range"]["end_seconds"], 11.0);
        let path = PathBuf::from(item["artifacts"]["transcript"].as_str().unwrap());
        let t: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(t["range"]["start_frame"], 144000);
        assert_eq!(t["range"]["end_frame"], 176000);
    }
    let before = batch_asr_calls(&fixture);
    let again = batch_command(&fixture, &input)
        .args(["--start", "9", "--end", "11"])
        .output()
        .unwrap();
    assert!(again.status.success());
    assert_eq!(batch_asr_calls(&fixture), before);
    let changed = batch_command(&fixture, &input)
        .args(["--start", "8", "--end", "11"])
        .output()
        .unwrap();
    assert_eq!(
        batch_json(&changed)["error"]["code"],
        "batch_identity_mismatch"
    );
    assert_eq!(batch_asr_calls(&fixture), before);
}

#[test]
fn discovered_defaults_match_saved_pipeline_options() {
    let fixture = Fixture::new();
    let original = fixture.run_command(URL);
    let mut args: Vec<_> = original.get_args().map(|s| s.to_os_string()).collect();
    let index = args.iter().position(|s| s == "--chunk-seconds").unwrap();
    args.drain(index..index + 2);
    let output = Command::new("/usr/bin/python3")
        .args(args)
        .env("AGENT_YOUTUBE_TEST_ROOT", &fixture.root)
        .output()
        .unwrap();
    let response = batch_json(&output);
    success(output);
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path("job.json")).unwrap()).unwrap();
    let described = Command::new(env!("CARGO_BIN_EXE_agent-youtube"))
        .args(["describe", "transcribe"])
        .output()
        .unwrap();
    let described: serde_json::Value = serde_json::from_slice(&described.stdout).unwrap();
    let props = &described["result"]["commands"][0]["input_schema"]["properties"];
    for name in ["chunk_seconds", "asr_timeout_seconds", "boundary_window_ms"] {
        assert_eq!(saved["options"][name], props[name]["default"]);
    }
    assert_eq!(
        saved["options"]["retry_window_seconds"],
        props["retry_window_seconds"]["x-default-policy"]["fallback"]
    );
    assert_eq!(
        saved["options"]["model_variant"],
        props["model_variant"]["x-default-policy"]["fallback"]
    );
    let variant = described["result"]["commands"][0]["success_variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| {
            v["when"]["response"]
                .as_array()
                .unwrap()
                .contains(&props["response"]["default"])
        })
        .unwrap();
    assert_eq!(response["schema_version"], variant["schema_version"]);
    assert!(saved["options"].get("range").is_none());
}
