use crate::{Result, process::Runner, protocol, storage};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(test)]
use std::ffi::OsStr;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

const RESPONSE_LIMIT: u64 = 16 * 1024 * 1024;
const PROGRESS_LIMIT: u64 = 1024 * 1024;
static NEXT_CAPTURE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub(crate) struct Sitta {
    executable: PathBuf,
    operation_timeout: Duration,
    outer_timeout: Duration,
    #[cfg(test)]
    mode: Option<OsString>,
    #[cfg(test)]
    contract: bool,
    #[cfg(test)]
    route: Option<OsString>,
}

#[derive(Deserialize)]
struct Envelope {
    schema_version: u32,
    command: String,
    ok: bool,
    result: Option<Value>,
    error: Option<PublicError>,
}

#[derive(Deserialize)]
struct PublicError {
    code: String,
    #[allow(dead_code)]
    message: String,
    retryable: bool,
    http_status: Option<u16>,
    retry_after_seconds: Option<u64>,
}

fn capture(prefix: &Path, kind: &str) -> (PathBuf, PathBuf) {
    let id = NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed);
    (
        PathBuf::from(format!("{}.sitta-{}-{}-stdout", prefix.display(), kind, id)),
        PathBuf::from(format!("{}.sitta-{}-{}-stderr", prefix.display(), kind, id)),
    )
}

fn read_optional(path: &Path, limit: u64) -> Result<Vec<u8>> {
    match storage::read_text(path, limit) {
        Ok(data) => Ok(data.into_bytes()),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

impl Sitta {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let Some(executable) = std::env::var_os("AGENT_YOUTUBE_SITTA") else {
            return Ok(None);
        };
        if executable.is_empty() {
            return Err("AGENT_YOUTUBE_SITTA must name the Sitta executable".into());
        }
        Ok(Some(Self::new(PathBuf::from(executable))?))
    }

    pub(crate) fn new(executable: PathBuf) -> Result<Self> {
        let sitta = Self {
            executable,
            operation_timeout: Duration::from_secs(30 * 60),
            outer_timeout: Duration::from_secs(30 * 60 + 15),
            #[cfg(test)]
            mode: None,
            #[cfg(test)]
            contract: false,
            #[cfg(test)]
            route: None,
        };
        sitta.negotiate()?;
        Ok(sitta)
    }

    #[cfg(test)]
    pub(crate) fn for_test(executable: PathBuf, mode: &str) -> Result<Self> {
        let mut sitta = Self {
            executable,
            operation_timeout: Duration::from_secs(30 * 60),
            outer_timeout: Duration::from_secs(30 * 60 + 15),
            mode: Some(OsString::from(mode)),
            contract: true,
            route: None,
        };
        if mode == "timeout" {
            sitta.operation_timeout = Duration::from_millis(50);
            sitta.outer_timeout = Duration::from_secs(2);
        }
        sitta.negotiate()?;
        Ok(sitta)
    }

    #[cfg(test)]
    pub(crate) fn for_live(executable: PathBuf, route: &str, timeout: Duration) -> Result<Self> {
        let sitta = Self {
            executable,
            operation_timeout: timeout,
            outer_timeout: timeout + Duration::from_secs(15),
            mode: None,
            contract: false,
            route: Some(OsString::from(route)),
        };
        sitta.negotiate()?;
        Ok(sitta)
    }

    fn command(&self, args: &[OsString]) -> Vec<OsString> {
        let mut command = Vec::new();
        #[cfg(test)]
        if self.contract {
            command.extend([
                OsString::from("-test.run=^TestCLIContractProcess$"),
                OsString::from("-test.count=1"),
                OsString::from("--"),
            ]);
        }
        command.extend_from_slice(args);
        command
    }

    fn run(
        &self,
        prefix: &Path,
        args: &[OsString],
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> (Result<()>, Vec<u8>, Vec<u8>) {
        let (stdout, stderr) = capture(prefix, "process");
        let (outcome, out, err) = std::thread::scope(|scope| {
            let stop = Arc::new(AtomicBool::new(cancelled.load(Ordering::SeqCst)));
            let done = Arc::new(AtomicBool::new(false));
            let monitor_stop = stop.clone();
            let monitor_done = done.clone();
            let monitor = scope.spawn(move || {
                while !monitor_done.load(Ordering::SeqCst) {
                    if cancelled.load(Ordering::SeqCst) {
                        monitor_stop.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            let runner = Runner::new(stop).with_timeout(timeout);
            #[cfg(test)]
            let runner = match &self.mode {
                Some(mode) => runner.with_env(OsStr::new("SITTA_CLI_CONTRACT_MODE"), mode),
                None => runner,
            };
            let outcome = runner.run(&self.executable, &self.command(args), &stdout, &stderr);
            done.store(true, Ordering::SeqCst);
            let _ = monitor.join();
            let out = read_optional(&stdout, RESPONSE_LIMIT);
            let err = read_optional(&stderr, PROGRESS_LIMIT);
            (outcome, out, err)
        });
        match (outcome, out, err) {
            (Ok(()), Ok(out), Ok(err)) => (Ok(()), out, err),
            (Err(error), Ok(out), Ok(err)) => (Err(error), out, err),
            (Ok(()), Err(error), _) | (Err(_), Err(error), _) => {
                (Err(error), Vec::new(), Vec::new())
            }
            (Err(error), Ok(out), Err(_)) => (Err(error), out, Vec::new()),
            (Ok(()), Ok(out), Err(error)) => (Err(error), out, Vec::new()),
        }
    }

    fn negotiate(&self) -> Result<()> {
        let cancelled = AtomicBool::new(false);
        let prefix = std::env::temp_dir().join(format!("sitta-negotiate-{}", std::process::id()));
        let (outcome, version, _) = self.run(
            &prefix,
            &[OsString::from("--version")],
            &cancelled,
            Duration::from_secs(10),
        );
        outcome?;
        let envelope = parse_envelope(&version, "--version")?;
        let result = success(envelope)?;
        if result["name"] != "sitta"
            || result["version"]
                .as_str()
                .is_none_or(|version| version.is_empty())
        {
            return Err("Sitta returned an invalid version identity".into());
        }
        let (outcome, help, _) = self.run(
            &prefix,
            &[OsString::from("help")],
            &cancelled,
            Duration::from_secs(10),
        );
        outcome?;
        let result = success(parse_envelope(&help, "help")?)?;
        let commands = result["commands"]
            .as_array()
            .ok_or("Sitta help omitted command discovery")?;
        if !commands.iter().any(|command| {
            command
                .as_str()
                .is_some_and(|command| command.starts_with("download "))
        }) {
            return Err("Sitta does not offer the required download command".into());
        }
        Ok(())
    }

    pub(crate) fn download(
        &self,
        id: &str,
        prefix: &Path,
        cancelled: &AtomicBool,
    ) -> Result<PathBuf> {
        let output = super::suffix(prefix, ".m4a");
        let timeout = format!("{}ms", self.operation_timeout.as_millis());
        let args = vec![
            OsString::from("download"),
            OsString::from("--output"),
            output.clone().into_os_string(),
            OsString::from("--timeout"),
            OsString::from(timeout),
            OsString::from(id),
        ];
        #[cfg(test)]
        let args = {
            let mut args = args;
            if let Some(route) = &self.route {
                args.splice(1..1, [OsString::from("--route"), route.clone()]);
            }
            args
        };
        let (outcome, stdout, stderr) = self.run(prefix, &args, cancelled, self.outer_timeout);
        if cancelled.load(Ordering::SeqCst) {
            return Err(protocol::failure(
                "interrupted",
                Some(true),
                "download interrupted; media preserved",
            ));
        }
        parse_progress(&stderr)?;
        let result = match parse_envelope(&stdout, "download") {
            Ok(envelope) => success(envelope)?,
            Err(error) => {
                if cancelled.load(Ordering::SeqCst) {
                    return Err(protocol::failure(
                        "interrupted",
                        Some(true),
                        "download interrupted; media preserved",
                    ));
                }
                if let Err(process) = outcome {
                    return Err(process);
                }
                return Err(error);
            }
        };
        outcome?;
        verify_media(&result, &output)
    }
}

fn parse_envelope(data: &[u8], command: &str) -> Result<Envelope> {
    let mut documents = serde_json::Deserializer::from_slice(data).into_iter::<Envelope>();
    let envelope = documents
        .next()
        .ok_or_else(|| {
            protocol::failure(
                "sitta_protocol_invalid",
                Some(false),
                "missing Sitta response",
            )
        })?
        .map_err(|error| protocol::failure("sitta_protocol_invalid", Some(false), error))?;
    if documents.next().is_some() || envelope.schema_version != 1 || envelope.command != command {
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta returned an incompatible or multiple-document response",
        ));
    }
    Ok(envelope)
}

fn success(envelope: Envelope) -> Result<Value> {
    if envelope.ok {
        if let Some(result) = envelope.result.filter(|result| !result.is_null())
            && envelope.error.is_none()
        {
            return Ok(result);
        }
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta returned an inconsistent success envelope",
        ));
    }
    let error = envelope.error.ok_or_else(|| {
        protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta returned an inconsistent error envelope",
        )
    })?;
    if envelope.result.is_some() {
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta returned a result alongside an error",
        ));
    }
    Err(public_failure(error))
}

fn public_failure(error: PublicError) -> Box<dyn std::error::Error + Send + Sync> {
    let retryable = Some(error.retryable);
    let details = json!({"cause_code":error.code,"http_status":error.http_status,"retry_after_seconds":error.retry_after_seconds,"preserved":true});
    match error.code.as_str() {
        "cancelled" => protocol::failure_details(
            "interrupted",
            retryable,
            "Sitta download interrupted; media preserved",
            details,
        ),
        "deadline_exceeded" => protocol::failure_details(
            "process_timeout",
            retryable,
            "Sitta operation deadline exceeded; media preserved",
            details,
        ),
        "youtube_rate_limited" => protocol::failure_details(
            "youtube_rate_limited",
            retryable,
            "Sitta requested a cooldown; media preserved",
            details,
        ),
        _ => protocol::failure_details(
            "sitta_acquisition_failed",
            retryable,
            "Sitta acquisition failed; media preserved",
            details,
        ),
    }
}

fn parse_progress(data: &[u8]) -> Result<()> {
    for line in data
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event: Value = serde_json::from_slice(line)
            .map_err(|error| protocol::failure("sitta_protocol_invalid", Some(false), error))?;
        if event["schema_version"] != 1
            || event["event"] != "progress"
            || !event["stage"].is_string()
        {
            return Err(protocol::failure(
                "sitta_protocol_invalid",
                Some(false),
                "Sitta returned an invalid progress frame",
            ));
        }
        protocol::progress(
            "download",
            format!("Sitta {}", event["stage"].as_str().unwrap_or_default()),
            json!({}),
        );
    }
    Ok(())
}

fn verify_media(result: &Value, output: &Path) -> Result<PathBuf> {
    let media = &result["media"];
    if media["verified"] != true
        || media["path"].as_str() != Some(output.to_string_lossy().as_ref())
    {
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta returned an unverified output identity",
        ));
    }
    let metadata = fs::symlink_metadata(output)
        .map_err(|error| protocol::failure("sitta_protocol_invalid", Some(false), error))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || media["bytes"].as_u64() != Some(metadata.len())
    {
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta output is not a complete regular file",
        ));
    }
    let expected = media["sha256"]
        .as_str()
        .ok_or("Sitta omitted the output digest")?;
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || storage::digest(output)? != expected
    {
        return Err(protocol::failure(
            "sitta_protocol_invalid",
            Some(false),
            "Sitta output digest differs from its receipt",
        ));
    }
    Ok(output.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::atomic::AtomicBool, thread};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-sitta-{}-{}-{}",
            name,
            std::process::id(),
            NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
        ));
        storage::private_directory(&root, true).unwrap();
        root
    }

    #[test]
    fn envelope_evolution_and_error_redaction() {
        let payload = br#"{"schema_version":1,"command":"download","ok":true,"result":{"media":{}},"future":true}"#;
        assert!(success(parse_envelope(payload, "download").unwrap()).is_ok());
        for invalid in [
            br#"{"schema_version":2,"command":"download","ok":true,"result":{}}"#.as_slice(),
            br#"{"schema_version":1,"command":"download","ok":true,"result":{}}
{"schema_version":1,"command":"download","ok":true,"result":{}}"#.as_slice(),
            br#"{"schema_version":1,"command":"download","ok":false,"error":{"code":"x","message":"private","retryable":false},"result":{"private":true}}"#.as_slice(),
        ] {
            assert!(parse_envelope(invalid, "download").is_err() || success(parse_envelope(invalid, "download").unwrap()).is_err());
        }
        let error = PublicError {
            code: "youtube_http_error".into(),
            message: "signed-url-private-marker".into(),
            retryable: false,
            http_status: Some(403),
            retry_after_seconds: None,
        };
        let failure = public_failure(error).to_string();
        assert!(!failure.contains("private-marker") && !failure.contains("signed-url"));
    }

    #[test]
    fn real_cli_success_failure_cancellation_and_restart() {
        let Some(executable) = std::env::var_os("AGENT_YOUTUBE_SITTA_CONTRACT_BINARY") else {
            eprintln!("skipping explicit Sitta contract process qualification");
            return;
        };
        let executable = PathBuf::from(executable);
        let root = test_root("sitta-adapter");
        let downloader = super::super::Downloader::sitta_for_test(&executable, "success").unwrap();
        let output = downloader
            .download(
                "jNQXAC9IVRw",
                &root.join("success"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(fs::read(output).unwrap(), b"abcdef");

        let failure = Sitta::for_test(executable.clone(), "failure").unwrap();
        assert!(
            failure
                .download(
                    "jNQXAC9IVRw",
                    &root.join("failure"),
                    &AtomicBool::new(false)
                )
                .is_err()
        );
        assert!(!super::super::suffix(&root.join("failure"), ".m4a").exists());

        let restart = Sitta::for_test(executable.clone(), "restart").unwrap();
        assert!(
            restart
                .download(
                    "jNQXAC9IVRw",
                    &root.join("restart"),
                    &AtomicBool::new(false)
                )
                .is_err()
        );
        assert!(
            restart
                .download(
                    "jNQXAC9IVRw",
                    &root.join("restart"),
                    &AtomicBool::new(false)
                )
                .is_ok()
        );
        assert_eq!(
            fs::read(super::super::suffix(&root.join("restart"), ".m4a")).unwrap(),
            b"abcdef"
        );

        let interrupted = Sitta::for_test(executable.clone(), "interrupt").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal_cancelled = cancelled.clone();
        let signal = std::thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            signal_cancelled.store(true, Ordering::SeqCst);
        });
        assert!(
            interrupted
                .download("jNQXAC9IVRw", &root.join("interrupt"), &cancelled)
                .is_err()
        );
        signal.join().unwrap();

        let timeout = Sitta::for_test(executable, "timeout").unwrap();
        assert!(
            timeout
                .download(
                    "jNQXAC9IVRw",
                    &root.join("timeout"),
                    &AtomicBool::new(false)
                )
                .is_err()
        );
    }

    fn captured_result(prefix: &Path) -> Result<Value> {
        let parent = prefix.parent().ok_or("capture parent is missing")?;
        let name = prefix
            .file_name()
            .ok_or("capture name is missing")?
            .to_string_lossy();
        let mut paths = Vec::new();
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            if filename.starts_with(name.as_ref()) && filename.ends_with("stdout") {
                paths.push(entry.path());
            }
        }
        paths.sort();
        let path = paths.last().ok_or("Sitta result capture is missing")?;
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn live_prefix(root: &Path, name: &str) -> Result<PathBuf> {
        let directory = root.join(name);
        storage::private_directory(&directory, true)?;
        Ok(directory.join("input"))
    }

    #[test]
    #[ignore = "explicit owner-authorized live Sitta adapter handoff"]
    fn live_adapter_handoff_and_restart() {
        let executable =
            PathBuf::from(std::env::var_os("AGENT_YOUTUBE_SITTA_LIVE_EXECUTABLE").unwrap());
        let root = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_SITTA_LIVE_DIRECTORY").unwrap());
        storage::private_directory(&root, true).unwrap();
        let cancelled = AtomicBool::new(false);

        let direct = super::super::Downloader::sitta_live_for_test(
            &executable,
            "auto",
            Duration::from_secs(1800),
        )
        .unwrap();
        let direct_prefix = live_prefix(&root, "direct").unwrap();
        let direct_path = direct
            .download("of4B3aw7MXo", &direct_prefix, &cancelled)
            .unwrap();
        let direct_result = captured_result(&direct_prefix).unwrap();
        assert_eq!(fs::metadata(&direct_path).unwrap().len(), 858608);
        assert_eq!(
            storage::digest(&direct_path).unwrap(),
            "283550d73f033f6ef17becc4c9301fad5e100237333e4f3d66d5d10318fdccb5"
        );
        assert_eq!(direct_result["result"]["botguard_used"], false);
        eprintln!(
            "sitta_live_evidence {}",
            json!({"case":"direct","client":direct_result["result"]["recording"]["client"],"sabr":direct_result["result"]["recording"]["selected_audio"]["sabr"],"bytes":858608,"sha256":storage::digest(&direct_path).unwrap()})
        );

        let interrupted = super::super::Downloader::sitta_live_for_test(
            &executable,
            "auto",
            Duration::from_secs(2),
        )
        .unwrap();
        let restart_prefix = live_prefix(&root, "restart").unwrap();
        assert!(
            interrupted
                .download("dQw4w9WgXcQ", &restart_prefix, &cancelled)
                .is_err()
        );
        let checkpoint: Value = serde_json::from_slice(
            &fs::read(super::super::suffix(&restart_prefix, ".m4a.resume.json")).unwrap(),
        )
        .unwrap();
        let committed = checkpoint["bytes"].as_u64().unwrap();
        assert!(committed > 0 && super::super::suffix(&restart_prefix, ".m4a.partial").exists());
        let resumed = super::super::Downloader::sitta_live_for_test(
            &executable,
            "auto",
            Duration::from_secs(1800),
        )
        .unwrap();
        let resumed_path = resumed
            .download("dQw4w9WgXcQ", &restart_prefix, &cancelled)
            .unwrap();
        let resumed_result = captured_result(&restart_prefix).unwrap();
        assert_eq!(
            resumed_result["result"]["media"]["resumed_bytes"].as_u64(),
            Some(committed)
        );
        assert_eq!(fs::metadata(&resumed_path).unwrap().len(), 3449447);
        assert_eq!(
            storage::digest(&resumed_path).unwrap(),
            "51aa762a3254bcbad8813d2badff2f301ed0127a61643a0ab16f387c883e6394"
        );
        eprintln!(
            "sitta_live_evidence {}",
            json!({"case":"restart","client":resumed_result["result"]["recording"]["client"],"sabr":resumed_result["result"]["recording"]["selected_audio"]["sabr"],"resumed_bytes":committed,"bytes":3449447,"sha256":storage::digest(&resumed_path).unwrap()})
        );

        let protected = super::super::Downloader::sitta_live_for_test(
            &executable,
            "mweb-po",
            Duration::from_secs(1800),
        )
        .unwrap();
        let protected_prefix = live_prefix(&root, "protected").unwrap();
        let protected_path = protected
            .download("dQw4w9WgXcQ", &protected_prefix, &cancelled)
            .unwrap();
        let protected_result = captured_result(&protected_prefix).unwrap();
        assert_eq!(fs::metadata(&protected_path).unwrap().len(), 3449447);
        assert_eq!(
            storage::digest(&protected_path).unwrap(),
            "51aa762a3254bcbad8813d2badff2f301ed0127a61643a0ab16f387c883e6394"
        );
        assert_eq!(protected_result["result"]["botguard_used"], true);
        assert_eq!(protected_result["result"]["media"]["po"]["attached"], true);
        eprintln!(
            "sitta_live_evidence {}",
            json!({"case":"protected","client":protected_result["result"]["recording"]["client"],"sabr":protected_result["result"]["recording"]["selected_audio"]["sabr"],"bytes":3449447,"sha256":storage::digest(&protected_path).unwrap(),"po_attached":true})
        );
    }

    #[test]
    #[ignore = "explicit owner-authorized checkpoint-triggered live handoff"]
    fn live_checkpoint_restart_and_token_handoff() {
        let executable =
            PathBuf::from(std::env::var_os("AGENT_YOUTUBE_SITTA_LIVE_EXECUTABLE").unwrap());
        let root = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_SITTA_LIVE_DIRECTORY").unwrap());
        storage::private_directory(&root, true).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let restart_prefix = live_prefix(&root, "restart").unwrap();
        let checkpoint_path = super::super::suffix(&restart_prefix, ".m4a.resume.json");
        let interrupted = super::super::Downloader::sitta_live_for_test(
            &executable,
            "auto",
            Duration::from_secs(1800),
        )
        .unwrap();
        std::thread::scope(|scope| {
            let download =
                scope.spawn(|| interrupted.download("dQw4w9WgXcQ", &restart_prefix, &cancelled));
            while !download.is_finished() {
                if let Ok(data) = fs::read(&checkpoint_path)
                    && let Ok(checkpoint) = serde_json::from_slice::<Value>(&data)
                    && checkpoint["bytes"]
                        .as_u64()
                        .is_some_and(|bytes| bytes > 0 && bytes < 3449447)
                {
                    cancelled.store(true, Ordering::SeqCst);
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let outcome = download.join().unwrap();
            assert!(outcome.is_err());
        });
        let checkpoint: Value =
            serde_json::from_slice(&fs::read(&checkpoint_path).unwrap()).unwrap();
        let committed = checkpoint["bytes"].as_u64().unwrap();
        assert!(committed > 0 && super::super::suffix(&restart_prefix, ".m4a.partial").exists());
        cancelled.store(false, Ordering::SeqCst);
        let resumed = super::super::Downloader::sitta_live_for_test(
            &executable,
            "auto",
            Duration::from_secs(1800),
        )
        .unwrap();
        let resumed_path = resumed
            .download("dQw4w9WgXcQ", &restart_prefix, &cancelled)
            .unwrap();
        let resumed_result = captured_result(&restart_prefix).unwrap();
        assert_eq!(
            resumed_result["result"]["media"]["resumed_bytes"].as_u64(),
            Some(committed)
        );
        assert_eq!(fs::metadata(&resumed_path).unwrap().len(), 3449447);
        assert_eq!(
            storage::digest(&resumed_path).unwrap(),
            "51aa762a3254bcbad8813d2badff2f301ed0127a61643a0ab16f387c883e6394"
        );
        eprintln!(
            "sitta_live_evidence {}",
            json!({"case":"checkpoint_restart","client":resumed_result["result"]["recording"]["client"],"sabr":resumed_result["result"]["recording"]["selected_audio"]["sabr"],"resumed_bytes":committed,"bytes":3449447,"sha256":storage::digest(&resumed_path).unwrap()})
        );

        let protected = super::super::Downloader::sitta_live_for_test(
            &executable,
            "mweb-po",
            Duration::from_secs(1800),
        )
        .unwrap();
        let protected_prefix = live_prefix(&root, "protected").unwrap();
        let protected_path = protected
            .download("dQw4w9WgXcQ", &protected_prefix, &cancelled)
            .unwrap();
        let protected_result = captured_result(&protected_prefix).unwrap();
        assert_eq!(fs::metadata(&protected_path).unwrap().len(), 3449447);
        assert_eq!(
            storage::digest(&protected_path).unwrap(),
            "51aa762a3254bcbad8813d2badff2f301ed0127a61643a0ab16f387c883e6394"
        );
        assert_eq!(protected_result["result"]["botguard_used"], true);
        assert_eq!(protected_result["result"]["media"]["po"]["attached"], true);
        eprintln!(
            "sitta_live_evidence {}",
            json!({"case":"protected","client":protected_result["result"]["recording"]["client"],"sabr":protected_result["result"]["recording"]["selected_audio"]["sabr"],"bytes":3449447,"sha256":storage::digest(&protected_path).unwrap(),"po_attached":true})
        );
    }

    #[test]
    fn fake_cli_version_and_result_validation() {
        let root = test_root("sitta-fake");
        let script = root.join("fake-sitta.sh");
        let mut file = fs::File::create(&script).unwrap();
        writeln!(file, "#!/bin/sh\ncase \"$1\" in\n --version) printf '%s\\n' '{{\"schema_version\":1,\"command\":\"--version\",\"ok\":true,\"result\":{{\"name\":\"sitta\",\"version\":\"test\"}}}}' ;;\n help) printf '%s\\n' '{{\"schema_version\":1,\"command\":\"help\",\"ok\":true,\"result\":{{\"commands\":[\"download --output FILE URL\"]}}}}' ;;\n download) printf '%s\\n' '{{\"schema_version\":1,\"command\":\"download\",\"ok\":true,\"result\":{{\"media\":{{\"verified\":false}}}}}}' ;;\nesac").unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let sitta = Sitta::new(script).unwrap();
        assert!(
            sitta
                .download("jNQXAC9IVRw", &root.join("fake"), &AtomicBool::new(false))
                .is_err()
        );
    }
}
