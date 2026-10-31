//! Bounded orchestration of independent durable transcription jobs.
use crate::{
    Result,
    download::Downloader,
    pipeline::{Options, Pipeline, RecognitionGate},
    protocol, storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

const INPUT_LIMIT: u64 = 8 * 1024 * 1024;
const ITEM_LIMIT: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Item {
    id: String,
    url: String,
}

fn invalid() -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure(
        "batch_input_invalid",
        Some(false),
        "expected nonempty JSONL records with unique nonempty IDs (up to 128 bytes), URLs (up to 4096 bytes), at most 10000 items and 8 MiB total",
    )
}
fn read_items(input: &Path) -> Result<Vec<Item>> {
    let mut bytes = Vec::new();
    storage::open_regular_input(input)?
        .take(INPUT_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > INPUT_LIMIT {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for line in text.lines() {
        let item: Item = serde_json::from_str(line).map_err(|_| invalid())?;
        if item.id.trim().is_empty()
            || item.id.len() > 128
            || item.id.chars().any(char::is_control)
            || item.url.len() > 4096
            || !ids.insert(item.id.clone())
            || items.len() == ITEM_LIMIT
        {
            return Err(invalid());
        }
        items.push(item);
    }
    if items.is_empty() {
        return Err(invalid());
    }
    Ok(items)
}
fn write(path: &Path, value: &Value) -> Result<()> {
    storage::reject_nonregular(path)?;
    storage::atomic_write(path, &serde_json::to_vec(value)?, true)
}
fn key(item: &Item) -> String {
    format!("{:x}", Sha256::digest(item.id.as_bytes()))
}

pub(crate) fn run(
    input: &Path,
    mut options: Options,
    workers: usize,
    downloader: Downloader,
    cancelled: Arc<AtomicBool>,
) -> Result<Value> {
    if !(1..=8).contains(&workers) {
        return Err(protocol::failure(
            "batch_workers_invalid",
            Some(false),
            "--jobs must be between 1 and 8",
        ));
    }
    let items = read_items(input)?;
    storage::private_directory(&options.work_dir, true)?;
    options.work_dir = fs::canonicalize(&options.work_dir)?;
    let root = options.work_dir.clone();
    let _lock = storage::Lock::acquire(&root)?;
    let marker = root.join("batch.json");
    let identity = json!({"schema_version":1,"working_directory":std::env::current_dir()?,"options":options,"items":items});
    storage::reject_nonregular(&marker)?;
    match fs::symlink_metadata(&marker) {
        Ok(_) => {
            let prior: Value =
                serde_json::from_str(&storage::read_text(&marker, 32 * 1024 * 1024)?)?;
            if prior != identity {
                return Err(protocol::failure(
                    "batch_identity_mismatch",
                    Some(false),
                    "batch inputs or options differ; use the original command or a new work directory",
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            for entry in fs::read_dir(&root)? {
                if entry?.file_name() != ".agent-youtube.lock" {
                    return Err(protocol::failure(
                        "batch_directory_not_empty",
                        Some(false),
                        "new batch requires an empty work directory; existing files preserved",
                    ));
                }
            }
            storage::atomic_write(&marker, &serde_json::to_vec(&identity)?, false)?;
        }
        Err(e) => return Err(e.into()),
    }
    for name in ["jobs", "results"] {
        storage::private_directory(&root.join(name), true)?;
    }
    // A surviving child retains its job lock after the batch parent is killed.
    // Do not launch another recognizer until every retained job is idle.
    for item in &items {
        let job = root.join("jobs").join(key(item));
        if job.join(".agent-youtube.lock").try_exists()? && storage::Lock::observe(&job)?.is_none()
        {
            return Err(protocol::failure_details(
                "batch_jobs_busy",
                Some(true),
                "a retained job is still active; wait before repeating the batch",
                json!({"id":item.id,"work_dir":job,"next_action":"wait_and_repeat_batch"}),
            ));
        }
    }
    let recognition = Arc::new(RecognitionGate::default());
    let next = AtomicUsize::new(0);
    let signals: Vec<_> = (0..workers.min(items.len()))
        .map(|_| Arc::new(AtomicBool::new(false)))
        .collect();
    let stopped = AtomicBool::new(false);
    let aborted = AtomicBool::new(false);
    let results = std::thread::scope(|scope| -> Result<Vec<(usize, Value)>> {
        let watcher = std::thread::Builder::new()
            .name("batch-cancellation".into())
            .spawn_scoped(scope, || {
                while !stopped.load(Ordering::SeqCst) {
                    if cancelled.load(Ordering::SeqCst) || aborted.load(Ordering::SeqCst) {
                        for signal in &signals {
                            signal.store(true, Ordering::SeqCst);
                        }
                        break;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
            })?;
        let mut handles = Vec::new();
        let mut fatal = None;
        for signal in &signals {
            let work = || -> Result<Vec<(usize, Value)>> {
                let mut done = Vec::new();
                while !cancelled.load(Ordering::SeqCst) && !aborted.load(Ordering::SeqCst) {
                    signal.store(false, Ordering::SeqCst);
                    if cancelled.load(Ordering::SeqCst) || aborted.load(Ordering::SeqCst) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let job = root.join("jobs").join(key(item));
                    protocol::progress(
                        "batch",
                        "starting batch item",
                        json!({"id":item.id,"work_dir":job}),
                    );
                    let result = run_item(
                        item,
                        &job,
                        &options,
                        downloader.for_job(),
                        signal.clone(),
                        recognition.clone(),
                    );
                    let record = match result {
                        Ok(value) => value,
                        Err(error) => {
                            let error = protocol::context(error, "transcription_failed");
                            let typed = error.downcast_ref::<protocol::Failure>();
                            if typed.is_some_and(|e| {
                                matches!(
                                    e.code,
                                    "process_teardown_failed" | "web_runtime_teardown_failed"
                                )
                            }) {
                                aborted.store(true, Ordering::SeqCst);
                            }
                            let record = protocol::error_record(
                                error.as_ref(),
                                signal.load(Ordering::SeqCst),
                            );
                            json!({"id":item.id,"work_dir":job,"state":"failed","error":record["error"]})
                        }
                    };
                    if let Err(error) = write(
                        &root.join("results").join(format!("{}.json", key(item))),
                        &record,
                    ) {
                        aborted.store(true, Ordering::SeqCst);
                        return Err(error);
                    }
                    protocol::progress(
                        "batch",
                        "batch item finished",
                        json!({"id":item.id,"work_dir":job,"state":record["state"]}),
                    );
                    done.push((index, record));
                }
                Ok(done)
            };
            match std::thread::Builder::new()
                .name("batch-worker".into())
                .spawn_scoped(scope, work)
            {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    aborted.store(true, Ordering::SeqCst);
                    fatal = Some(error.into());
                    break;
                }
            }
        }
        let mut collected = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(done)) => collected.extend(done),
                Ok(Err(error)) => {
                    fatal = Some(error);
                    aborted.store(true, Ordering::SeqCst);
                }
                Err(_) => {
                    fatal = Some(protocol::failure(
                        "batch_worker_failed",
                        None,
                        "batch worker panicked; saved jobs preserved",
                    ));
                    aborted.store(true, Ordering::SeqCst);
                }
            }
        }
        stopped.store(true, Ordering::SeqCst);
        watcher.thread().unpark();
        if watcher.join().is_err() {
            fatal = Some(protocol::failure(
                "batch_worker_failed",
                None,
                "batch cancellation watcher panicked",
            ));
        }
        if let Some(error) = fatal {
            return Err(error);
        }
        Ok(collected)
    })?;
    let mut ordered: Vec<Value>=items.iter().map(|item| json!({"id":item.id,"work_dir":root.join("jobs").join(key(item)),"state":"pending","next_action":"repeat_batch"})).collect();
    for (index, result) in results {
        if let Some(slot) = ordered.get_mut(index) {
            *slot = result;
        }
    }
    let succeeded = ordered.iter().filter(|v| v["state"] == "succeeded").count();
    let failed = ordered.iter().filter(|v| v["state"] == "failed").count();
    let pending = ordered.len() - succeeded - failed;
    let report = json!({"state":if succeeded==items.len(){"completed"}else if cancelled.load(Ordering::SeqCst){"interrupted"}else{"partial"},
        "total":items.len(),"succeeded":succeeded,"failed":failed,"pending":pending,
        "workers":workers,"recognition_workers":1,"items":ordered});
    write(&root.join("batch-result.json"), &report)?;
    if succeeded != items.len() {
        let teardown = ordered
            .iter()
            .filter_map(|v| v["error"]["code"].as_str())
            .find(|code| {
                matches!(
                    *code,
                    "process_teardown_failed" | "web_runtime_teardown_failed"
                )
            });
        let retryable = if teardown.is_some() {
            None
        } else if pending > 0 || ordered.iter().any(|v| v["error"]["retryable"] == true) {
            Some(true)
        } else if ordered
            .iter()
            .filter(|v| v["state"] == "failed")
            .any(|v| v["error"]["retryable"].is_null())
        {
            None
        } else {
            Some(false)
        };
        return Err(protocol::failure_details(
            match teardown {
                Some("process_teardown_failed") => "process_teardown_failed",
                Some("web_runtime_teardown_failed") => "web_runtime_teardown_failed",
                _ => "batch_incomplete",
            },
            retryable,
            "batch has unsuccessful items; inspect individual errors and preserve the work directory",
            json!({"batch":report,"next_action":"inspect_items"}),
        ));
    }
    Ok(report)
}

fn run_item(
    item: &Item,
    root: &Path,
    template: &Options,
    downloader: Downloader,
    cancelled: Arc<AtomicBool>,
    recognition: Arc<RecognitionGate>,
) -> Result<Value> {
    let id = crate::pipeline::video_id(&item.url).map_err(|_| {
        protocol::failure(
            "batch_url_invalid",
            Some(false),
            "item must contain an HTTPS YouTube video URL",
        )
    })?;
    let mut options = template.clone();
    options.url = format!("https://www.youtube.com/watch?v={id}");
    options.work_dir = root.to_path_buf();
    let outcome = Pipeline::with_downloader(options, cancelled, downloader)?
        .with_recognition_gate(recognition)
        .run_report()?;
    let mut result = json!({"id":item.id,"work_dir":root,"state":"succeeded","reused":outcome.reused,
        "artifacts":{"transcript":outcome.transcript,"manifest":outcome.manifest},"cleanup_complete":true});
    if let Some(range) = outcome.range {
        result["range"] = range.summary();
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agent-youtube-batch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn options(&self) -> Options {
            Options {
                range: None,
                url: "https://www.youtube.com/watch?v=a2hbJlPxj4M".into(),
                work_dir: self.0.join("batch"),
                model_dir: self.0.join("models"),
                gigastt: self.0.join("gigastt"),
                model_variant: Default::default(),
                chunk_seconds: 300,
                boundary_window_ms: 0,
                asr_timeout_seconds: 1800,
                asr_memory_mib: None,
                encoder_threads: None,
                retry_window_seconds: 0,
            }
        }
        fn input(&self, text: &str) -> std::path::PathBuf {
            let path = self.0.join("input.jsonl");
            fs::write(&path, text).unwrap();
            path
        }
        fn run(&self, input: &Path, options: Options, cancelled: bool) -> Result<Value> {
            run(
                input,
                options,
                2,
                Downloader::loopback("http://127.0.0.1:1").unwrap(),
                Arc::new(AtomicBool::new(cancelled)),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn code(error: &(dyn std::error::Error + 'static)) -> &'static str {
        error.downcast_ref::<protocol::Failure>().unwrap().code
    }
    #[test]
    fn malformed_and_duplicate_inputs_do_not_create_jobs_or_echo_content() {
        let f = Fixture::new();
        for text in [
            "",
            "\n",
            "secret is not json",
            "{\"id\":\"a\",\"url\":\"x\",\"token\":\"secret\"}",
            "{\"id\":\"a\",\"url\":\"x\"}\n{\"id\":\"a\",\"url\":\"y\"}",
            "{\"id\":\"\",\"url\":\"x\"}",
        ] {
            let input = f.input(text);
            let error = f.run(&input, f.options(), false).unwrap_err();
            assert_eq!(code(error.as_ref()), "batch_input_invalid");
            assert!(!error.to_string().contains("secret"));
            assert!(!f.options().work_dir.exists());
        }
        let input = f.input(&"x".repeat(INPUT_LIMIT as usize + 1));
        assert_eq!(
            code(f.run(&input, f.options(), false).unwrap_err().as_ref()),
            "batch_input_invalid"
        );
    }
    #[test]
    fn cancellation_is_durable_and_manifest_is_immutable() {
        let f = Fixture::new();
        let input = f.input("{\"id\":\"../x\",\"url\":\"invalid\"}");
        let error = f.run(&input, f.options(), true).unwrap_err();
        assert_eq!(code(error.as_ref()), "batch_incomplete");
        let report: Value = serde_json::from_str(
            &fs::read_to_string(f.options().work_dir.join("batch-result.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["pending"], 1);
        assert_eq!(report["state"], "interrupted");
        assert!(!f.0.join("x").exists());
        let before = fs::read(f.options().work_dir.join("batch.json")).unwrap();
        let mut changed = f.options();
        changed.encoder_threads = Some(1);
        assert_eq!(
            code(f.run(&input, changed, true).unwrap_err().as_ref()),
            "batch_identity_mismatch"
        );
        assert_eq!(
            fs::read(f.options().work_dir.join("batch.json")).unwrap(),
            before
        );
        let _lock = storage::Lock::acquire(&f.options().work_dir).unwrap();
        let error = f.run(&input, f.options(), true).unwrap_err();
        assert_eq!(code(error.as_ref()), "work_directory_busy");
    }
    #[test]
    fn foreign_roots_are_preserved() {
        let f = Fixture::new();
        let input = f.input("{\"id\":\"a\",\"url\":\"invalid\"}");
        fs::create_dir(f.options().work_dir.clone()).unwrap();
        fs::write(f.options().work_dir.join("foreign"), b"keep").unwrap();
        assert_eq!(
            code(f.run(&input, f.options(), false).unwrap_err().as_ref()),
            "batch_directory_not_empty"
        );
        assert_eq!(
            fs::read(f.options().work_dir.join("foreign")).unwrap(),
            b"keep"
        );
    }
    #[test]
    fn retained_child_job_lock_blocks_all_new_work() {
        let f = Fixture::new();
        let input = f.input("{\"id\":\"a\",\"url\":\"invalid\"}");
        let _ = f.run(&input, f.options(), true);
        let item = Item {
            id: "a".into(),
            url: "invalid".into(),
        };
        let job = f.options().work_dir.join("jobs").join(key(&item));
        storage::private_directory(&job, true).unwrap();
        let _child = storage::Lock::acquire(&job).unwrap();
        let error = f.run(&input, f.options(), false).unwrap_err();
        assert_eq!(code(error.as_ref()), "batch_jobs_busy");
        assert!(
            fs::read_dir(f.options().work_dir.join("results"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    fn item_count_and_worker_bounds_fail_before_publication() {
        let f = Fixture::new();
        let input = f.input(
            &(0..=ITEM_LIMIT)
                .map(|id| format!("{{\"id\":\"{id}\",\"url\":\"x\"}}\n"))
                .collect::<String>(),
        );
        assert_eq!(
            code(f.run(&input, f.options(), false).unwrap_err().as_ref()),
            "batch_input_invalid"
        );
        for workers in [0, 9] {
            let error = run(
                &input,
                f.options(),
                workers,
                Downloader::loopback("http://127.0.0.1:1").unwrap(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap_err();
            assert_eq!(code(error.as_ref()), "batch_workers_invalid");
        }
        assert!(!f.options().work_dir.exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_result_never_overwrites_its_target() {
        let f = Fixture::new();
        let input = f.input("{\"id\":\"a\",\"url\":\"invalid\"}");
        let _ = f.run(&input, f.options(), true);
        let victim = f.0.join("private");
        fs::write(&victim, b"keep").unwrap();
        let receipt = f.options().work_dir.join("results").join(format!(
            "{}.json",
            key(&Item {
                id: "a".into(),
                url: "invalid".into()
            })
        ));
        std::os::unix::fs::symlink(&victim, receipt).unwrap();
        assert!(f.run(&input, f.options(), false).is_err());
        assert_eq!(fs::read(victim).unwrap(), b"keep");
    }
}
