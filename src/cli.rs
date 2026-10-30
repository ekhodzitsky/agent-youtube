use crate::{
    Result,
    download::Downloader,
    export::{self, Format},
    pipeline::{Options, Pipeline},
    protocol,
    protocol::context,
    storage, transcript,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const HELP: &str = "agent-youtube — JSON-only local Russian YouTube transcription

Usage:
  agent-youtube transcribe URL --work-dir DIR [options]
  agent-youtube status --work-dir DIR
  agent-youtube resume --work-dir DIR [--response compact|metadata|full]
  agent-youtube cancel --work-dir DIR
  agent-youtube setup [--runtime-dir DIR] [--model-dir DIR] [--archive FILE]
  agent-youtube doctor [--work-dir DIR] [tool/model options]
  agent-youtube export TRANSCRIPT.json --format json|md|txt|srt|vtt --output FILE [--force]

Options:
  --retry-window-seconds N    automatic recovery wait budget (default 3600; 0 disables)
  --response compact|metadata|full  transcribe response (default compact, schema2; full preserves schema1)
  --model-dir DIR             e2e_rnnt INT8 models (default ~/.gigastt/models)
  --runtime-dir DIR           prepared runtime (default tools-local/runtime if present)
  --gigastt PATH              explicit recognizer; bypasses managed discovery; gigastt with offline JSON batch support (2.22.0 verified)
  --chunk-seconds N           checkpoint interval, 10..600 (default 300)
  --asr-timeout-seconds N     maximum time without a valid new chunk (default 1800)
  --boundary-window-ms N      move ASR cuts into nearby quiet spans, 0..2000 (default 0)
  --asr-memory-mib N          observed worker-group RSS budget in MiB (Linux, optional)
  --encoder-threads N         encoder CPU threads (default gigastt auto)

Stdout: one JSON result; transcribe compact/metadata uses schema_version=2.
Stderr: schema_version=1 JSON event lines. Errors have code, retryable and details.
Unset GIGASTT_* and ORT_* environment settings; use explicit options.
Resume reuses saved options; legacy runs use the same transcribe command. Only audio is downloaded.
Exact ASR JSON is retained. Temporary media are removed after validated publication.
Exports use existing JSON and never run recognition. Existing outputs are protected.
";

pub fn run(args: Vec<OsString>, cancelled: Arc<AtomicBool>) -> Result<()> {
    run_with_downloader(args, cancelled, Downloader::youtube()?)
}
/// Run against an explicitly supplied downloader for embedding and offline tests.
pub fn run_with_downloader(
    args: Vec<OsString>,
    cancelled: Arc<AtomicBool>,
    downloader: Downloader,
) -> Result<()> {
    execute(args, cancelled, downloader).map_err(|e| context(e, "invalid_arguments"))
}
fn execute(args: Vec<OsString>, cancelled: Arc<AtomicBool>, downloader: Downloader) -> Result<()> {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        protocol::success("help", serde_json::json!({"help":HELP}))?;
        return Ok(());
    }
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        protocol::success("version", crate::agent::producer())?;
        return Ok(());
    }
    let command = args[0].to_str().ok_or("command must be UTF-8")?;
    let mut positional = Vec::new();
    let mut flags = BTreeMap::new();
    let mut force = false;
    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            positional.extend(iter.cloned());
            break;
        } else if arg == "--help" || arg == "-h" {
            protocol::success("help", serde_json::json!({"help":HELP}))?;
            return Ok(());
        } else if arg == "--force" {
            if force {
                return Err("duplicate --force".into());
            }
            force = true;
        } else if arg.to_string_lossy().starts_with('-') {
            let name = arg.to_str().ok_or("option name must be UTF-8")?.to_owned();
            if ![
                "--work-dir",
                "--model-dir",
                "--runtime-dir",
                "--archive",
                "--gigastt",
                "--chunk-seconds",
                "--asr-timeout-seconds",
                "--retry-window-seconds",
                "--asr-memory-mib",
                "--boundary-window-ms",
                "--encoder-threads",
                "--format",
                "--output",
                "--response",
            ]
            .contains(&name.as_str())
            {
                return Err(format!("unknown option {name}; see --help").into());
            }
            let value = iter
                .next()
                .ok_or_else(|| format!("missing value for {name}"))?;
            if flags.insert(name.clone(), value.clone()).is_some() {
                return Err(format!("duplicate option {name}").into());
            }
        } else {
            positional.push(arg.clone());
        }
    }
    match command {
        "setup" => {
            if force || !positional.is_empty() {
                return Err("setup accepts options only".into());
            }
            let root = take_path(&mut flags, "--runtime-dir", crate::runtime::DEFAULT_ROOT);
            let models = flags.remove("--model-dir").map(PathBuf::from);
            let archive = flags.remove("--archive").map(PathBuf::from);
            reject_remaining(&flags)?;
            let result = crate::runtime::setup(&root, models, archive, &cancelled)
                .map_err(|e| context(e, "setup_failed"))?;
            protocol::success(command, result)?;
        }
        "status" | "cancel" | "resume" => {
            if force || !positional.is_empty() {
                return Err(
                    "job commands require --work-dir and no positional arguments or --force".into(),
                );
            }
            let root = PathBuf::from(take_required(&mut flags, "--work-dir")?);
            if command == "resume" {
                let response = flags
                    .remove("--response")
                    .unwrap_or_else(|| "compact".into());
                validate_response(&response)?;
                reject_remaining(&flags)?;
                let options =
                    crate::job::resume(&root).map_err(|e| context(e, "job_resume_failed"))?;
                let pipeline = Pipeline::with_downloader(options, cancelled.clone(), downloader)?;
                transcribe(&pipeline, &response, &cancelled, command)?;
            } else {
                reject_remaining(&flags)?;
                let result = if command == "status" {
                    crate::job::status(&root)
                } else {
                    crate::job::cancel(&root)
                }
                .map_err(|e| context(e, "job_control_failed"))?;
                protocol::success(command, result)?;
            }
        }
        "export" => {
            if positional.len() != 1 {
                return Err("export requires exactly one transcript JSON path".into());
            }
            let input = PathBuf::from(&positional[0]);
            let format: Format = take_required(&mut flags, "--format")?
                .to_str()
                .ok_or("format must be UTF-8")?
                .parse()?;
            let output = PathBuf::from(take_required(&mut flags, "--output")?);
            reject_remaining(&flags)?;
            if output == input
                || (output.exists()
                    && std::fs::canonicalize(&output)? == std::fs::canonicalize(&input)?)
            {
                return Err("export output must differ from input transcript".into());
            }
            export_file(&input, &output, format, force, &cancelled)
                .map_err(|e| context(e, "export_failed"))?;
        }
        "transcribe" | "doctor" => {
            if command == "doctor" && flags.contains_key("--retry-window-seconds") {
                return Err("--retry-window-seconds is only accepted by transcribe".into());
            }
            let response = if command == "transcribe" {
                flags
                    .remove("--response")
                    .unwrap_or_else(|| "compact".into())
            } else {
                "compact".into()
            };
            if !["compact", "metadata", "full"]
                .iter()
                .any(|v| response == *v)
            {
                return Err("--response must be compact, metadata or full".into());
            }
            if force {
                return Err("--force is only accepted by export".into());
            }
            let url = if command == "transcribe" {
                if positional.len() != 1 {
                    return Err("transcribe requires exactly one YouTube URL".into());
                }
                positional[0]
                    .to_str()
                    .ok_or("URL must be UTF-8")?
                    .to_owned()
            } else {
                if !positional.is_empty() {
                    return Err("doctor does not accept positional arguments".into());
                }
                "https://www.youtube.com/watch?v=a2hbJlPxj4M".into()
            };
            let work_dir = if command == "transcribe" {
                PathBuf::from(take_required(&mut flags, "--work-dir")?)
            } else {
                take_path(&mut flags, "--work-dir", "runs/doctor")
            };
            let explicit_gigastt = flags.remove("--gigastt").map(PathBuf::from);
            let runtime_dir = flags.remove("--runtime-dir").map(PathBuf::from);
            if explicit_gigastt.is_some() && runtime_dir.is_some() {
                return Err("--gigastt and --runtime-dir cannot be combined".into());
            }
            let managed = if explicit_gigastt.is_none() {
                crate::runtime::select(
                    runtime_dir
                        .as_deref()
                        .unwrap_or(Path::new(crate::runtime::DEFAULT_ROOT)),
                    runtime_dir.is_some(),
                )?
            } else {
                None
            };
            let gigastt = explicit_gigastt
                .or_else(|| managed.as_ref().map(|s| s.gigastt.clone()))
                .unwrap_or_else(|| PathBuf::from("gigastt"));
            let model_dir = if let Some(path) = flags.remove("--model-dir") {
                PathBuf::from(path)
            } else if let Some(selected) = managed {
                selected.model_dir
            } else {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or("HOME is unset; specify --model-dir")?
                    .join(".gigastt/models")
            };
            let chunk_seconds = flags
                .remove("--chunk-seconds")
                .map(|v| v.to_string_lossy().parse::<u32>())
                .transpose()?
                .unwrap_or(300);
            let retry_window_seconds = match flags.remove("--retry-window-seconds") {
                Some(value) => value.to_string_lossy().parse()?,
                None if command == "transcribe" => {
                    crate::job::retry_window(&work_dir, &url)?.unwrap_or(3600)
                }
                None => 0,
            };
            let options = Options {
                url,
                work_dir,
                model_dir,
                chunk_seconds,
                asr_timeout_seconds: flags
                    .remove("--asr-timeout-seconds")
                    .map(|v| v.to_string_lossy().parse())
                    .transpose()?
                    .unwrap_or(1800),
                boundary_window_ms: flags
                    .remove("--boundary-window-ms")
                    .map(|v| v.to_string_lossy().parse())
                    .transpose()?
                    .unwrap_or(0),
                asr_memory_mib: flags
                    .remove("--asr-memory-mib")
                    .map(|v| v.to_string_lossy().parse())
                    .transpose()?,
                encoder_threads: flags
                    .remove("--encoder-threads")
                    .map(|v| v.to_string_lossy().parse())
                    .transpose()?,
                retry_window_seconds,
                gigastt,
            };
            reject_remaining(&flags)?;
            let pipeline = Pipeline::with_downloader(options, cancelled.clone(), downloader)?;
            if command == "doctor" {
                let version = pipeline
                    .doctor()
                    .map_err(|e| context(e, "dependency_failed"))?;
                protocol::success(
                    "doctor",
                    serde_json::json!({"gigastt_version":version,"model_variant":"e2e_rnnt","verified":false,"preflight_passed":true,
                        "checks":{"executable_runs":true,"required_flags_present":true,"model_files_readable":true,
                            "model_load":"not_checked","inference":"not_checked","youtube_access":"not_checked"}}),
                )?;
            } else {
                transcribe(&pipeline, &response, &cancelled, command)?;
            }
        }
        _ => return Err(format!("unknown command {command}; see --help").into()),
    }
    Ok(())
}
fn export_file(
    input: &Path,
    output: &Path,
    format: Format,
    force: bool,
    cancelled: &AtomicBool,
) -> Result<()> {
    use std::io::{Seek, Write};
    let mut reader = std::io::BufReader::new(storage::open_regular_input(input)?);
    let check_cancelled = || -> Result<()> {
        if cancelled.load(Ordering::SeqCst) {
            Err("interrupted".into())
        } else {
            Ok(())
        }
    };
    let header = transcript::read_streaming(&mut reader, |_| check_cancelled())?;
    storage::atomic_write_with(output, force, |file| {
        let mut writer = std::io::BufWriter::new(file);
        if format == Format::Json {
            crate::agent::write_view(&mut writer, input, false, cancelled)?;
            writer.flush()?;
            return Ok(());
        }
        {
            let mut renderer = export::Renderer::new(&mut writer, format, &header)?;
            reader.rewind()?;
            transcript::read_streaming(&mut reader, |segment| {
                check_cancelled()?;
                renderer.segment(segment)
            })?;
        }
        writer.flush()?;
        Ok(())
    })?;
    protocol::success(
        "export",
        serde_json::json!({"output":output,"format":format!("{format:?}").to_lowercase()}),
    )
}

fn take_required(flags: &mut BTreeMap<String, OsString>, key: &str) -> Result<OsString> {
    flags
        .remove(key)
        .ok_or_else(|| format!("missing required {key}").into())
}
fn take_path(flags: &mut BTreeMap<String, OsString>, key: &str, default: &str) -> PathBuf {
    flags
        .remove(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| default.into())
}
fn reject_remaining(flags: &BTreeMap<String, OsString>) -> Result<()> {
    if let Some(key) = flags.keys().next() {
        return Err(format!("option {key} is not valid for this command").into());
    }
    Ok(())
}

fn validate_response(response: &std::ffi::OsStr) -> Result<()> {
    if !["compact", "metadata", "full"]
        .iter()
        .any(|v| response == *v)
    {
        return Err("--response must be compact, metadata or full".into());
    }
    Ok(())
}
fn transcribe(
    pipeline: &Pipeline,
    response: &std::ffi::OsStr,
    cancelled: &AtomicBool,
    command: &str,
) -> Result<()> {
    let outcome = pipeline
        .run_report()
        .map_err(|e| context(e, "transcription_failed"))?;
    let view = if response == "full" {
        None
    } else {
        Some(
            crate::agent::View::open(&outcome.transcript, cancelled)
                .map_err(|e| context(e, "response_failed"))?,
        )
    };
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    write!(
        out,
        "{{\"schema_version\":{},\"ok\":true,\"command\":\"{command}\",\"result\":{{\"artifacts\":",
        if response == "full" { 1 } else { 2 }
    )?;
    serde_json::to_writer(
        &mut out,
        &serde_json::json!({"transcript":outcome.transcript,"manifest":outcome.manifest}),
    )?;
    write!(
        out,
        ",\"reused\":{},\"cleanup_complete\":true,\"transcript\":",
        outcome.reused
    )?;
    if response == "full" {
        std::io::copy(&mut std::fs::File::open(&outcome.transcript)?, &mut out)?;
    } else if let Some(view) = view {
        // Publication already succeeded. Complete the committed JSON
        // response rather than append an error to a partial document.
        view.write(&mut out, response == "metadata", &AtomicBool::new(false))?;
    }
    writeln!(out, "}}}}")?;
    Ok(())
}
