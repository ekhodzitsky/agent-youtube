//! Local, generation-scoped job control. The OS lock is the authority for liveness.
use crate::{Result, checkpoint::Checkpoint, pipeline::Options, protocol, storage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
mod retry;
pub(crate) use retry::Retry;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    schema_version: u32,
    working_directory: PathBuf,
    options: Options,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    schema_version: u32,
    generation: u64,
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    storage::reject_nonregular(path)?;
    Ok(serde_json::from_str(&storage::read_text(path, 64 << 10)?)?)
}
fn root(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("job directory must be a real directory".into());
    }
    Ok(fs::canonicalize(path)?)
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    storage::reject_nonregular(path)?;
    storage::atomic_write(path, &serde_json::to_vec_pretty(value)?, true)
}

pub(crate) fn save(options: &Options) -> Result<()> {
    write(
        &options.work_dir.join("job.json"),
        &Invocation {
            schema_version: 1,
            working_directory: std::env::current_dir()?,
            options: options.clone(),
        },
    )
}
// Read policy without imposing the relative tool-path constraints of resume.
fn invocation_retry_window(root: &Path, id: &str) -> Result<Option<u64>> {
    let saved = match read::<Invocation>(&root.join("job.json")) {
        Ok(saved) => saved,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if saved.schema_version != 1
        || saved.options.work_dir != root
        || crate::pipeline::video_id(&saved.options.url)? != id
    {
        return Err(protocol::failure(
            "job_invalid",
            Some(false),
            "saved invocation identity differs; retry policy preserved",
        ));
    }
    Ok(Some(saved.options.retry_window_seconds))
}
pub(crate) fn retry_window(path: &Path, url: &str) -> Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let root = root(path)?;
    let id = crate::pipeline::video_id(url)?;
    let prior = invocation_retry_window(&root, &id)?;
    if let Some(schedule) = retry::saved(&root)? {
        if schedule.video_id != id
            || prior.is_some_and(|window| window != 0 && window != schedule.window_seconds)
        {
            return Err(protocol::failure(
                "job_retry_invalid",
                Some(false),
                "saved invocation disagrees with retry schedule",
            ));
        }
        return Ok(Some(schedule.window_seconds));
    }
    Ok(prior)
}
pub(crate) fn resume(path: &Path) -> Result<Options> {
    let root = root(path)?;
    let mut saved: Invocation = read(&root.join("job.json"))?;
    if saved.schema_version != 1 || saved.options.work_dir != root {
        return Err(protocol::failure(
            "job_invalid",
            Some(false),
            "unsupported or mismatched saved invocation",
        ));
    }
    if std::env::current_dir()? != saved.working_directory
        && (saved.options.model_dir.is_relative()
            || (saved.options.gigastt.is_relative()
                && saved.options.gigastt.components().count() > 1))
    {
        return Err(protocol::failure_details(
            "job_working_directory_changed",
            Some(false),
            "relative invocation paths require the original working directory",
            json!({"working_directory":saved.working_directory,"next_action":"resume_from_working_directory"}),
        ));
    }
    if let Some(schedule) = retry::saved(&root)? {
        if schedule.video_id != crate::pipeline::video_id(&saved.options.url)?
            || (saved.options.retry_window_seconds != 0
                && saved.options.retry_window_seconds != schedule.window_seconds)
        {
            return Err(protocol::failure(
                "job_retry_invalid",
                Some(false),
                "saved invocation disagrees with retry schedule",
            ));
        }
        // The retry policy is durable before preflight can update an old invocation.
        saved.options.retry_window_seconds = schedule.window_seconds;
    }
    Ok(saved.options)
}

pub(crate) struct Active {
    stop: mpsc::Sender<()>,
    watcher: Option<JoinHandle<()>>,
    expired: Arc<AtomicBool>,
}
impl Active {
    // Caller holds the control lock during initialization and the run lock
    // for this guard's entire lifetime.
    pub(crate) fn start(
        options: &Options,
        cancelled: Arc<AtomicBool>,
        enforce_deadline: bool,
    ) -> Result<Self> {
        let root = &options.work_dir;
        let deadline = retry::saved(root)?
            .filter(|_| enforce_deadline)
            .and_then(|state| state.active_deadline());
        let expired = Arc::new(AtomicBool::new(false));
        let timed_out = expired.clone();
        if !root.join("job.json").exists() {
            save(options)?;
        }
        let control_path = root.join("job-control.json");
        let generation = match read::<Control>(&control_path) {
            Ok(previous) if previous.schema_version == 1 => previous
                .generation
                .checked_add(1)
                .ok_or("job generation exhausted")?,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                1
            }
            _ => {
                return Err(protocol::failure(
                    "job_invalid",
                    Some(false),
                    "invalid saved job control",
                ));
            }
        };
        write(
            &root.join("job-control.json"),
            &Control {
                schema_version: 1,
                generation,
            },
        )?;
        let cancel_path = root.join("job-cancel.json");
        let (stop, receiver) = mpsc::channel();
        let watcher = thread::Builder::new()
            .name("agent-youtube-cancel".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if deadline.is_some_and(|at| retry::now().is_ok_and(|time| time >= at)) {
                        timed_out.store(true, Ordering::SeqCst);
                        cancelled.store(true, Ordering::SeqCst);
                        break;
                    }
                    if let Ok(request) = read::<Control>(&cancel_path)
                        && request.schema_version == 1
                        && request.generation == generation
                    {
                        cancelled.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            })?;
        Ok(Self {
            stop,
            watcher: Some(watcher),
            expired,
        })
    }
    pub(crate) fn expired(&self) -> bool {
        self.expired.load(Ordering::SeqCst)
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

pub(crate) fn status(path: &Path) -> Result<Value> {
    let root = root(path)?;
    let idle = match storage::Lock::observe(&root) {
        Ok(idle) => idle,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(
                json!({"state":"not_started","next_action":"transcribe","work_dir":root,
                    "transcript_validated":false,"cleanup_complete":false}),
            );
        }
        Err(error) => return Err(error),
    };
    let Some(_idle) = idle else {
        let schedule = retry::saved(&root)?;
        let waiting = schedule
            .as_ref()
            .and_then(|s| s.next_retry_at)
            .is_some_and(|at| retry::now().is_ok_and(|time| at > time));
        let mut status = json!({"state":if waiting {"waiting"}else{"running"},"next_action":"poll_status","work_dir":root});
        if let Some(schedule) = schedule {
            schedule.status(&mut status, retry::now()?);
        }
        return Ok(status);
    };
    let mut status = Checkpoint::job_status(&root)?.unwrap_or_else(|| json!({
        "state":"not_started","next_action":if root.join("job.json").exists(){"resume"}else{"transcribe"},
        "transcript_validated":false,"cleanup_complete":false
    }));
    if status["next_action"] == "resume" && !root.join("job.json").exists() {
        status["next_action"] = json!("repeat_transcribe");
    }
    status["work_dir"] = json!(root);
    if let Some(schedule) = retry::saved(&root)? {
        let time = retry::now()?;
        schedule.status(&mut status, time);
        if schedule.exhausted(time) && status["state"] != "completed" {
            status["state"] = json!("deadline_exceeded");
            status["next_action"] = json!("inspect_preserved_artifacts");
        }
    }
    Ok(status)
}
pub(crate) fn cancel(path: &Path) -> Result<Value> {
    let root = root(path)?;
    let _control = storage::Lock::control(&root)?;
    let idle = match storage::Lock::observe(&root) {
        Ok(idle) => idle,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(
                json!({"state":"idle","cancel_requested":false,"next_action":"status","work_dir":root}),
            );
        }
        Err(error) => return Err(error),
    };
    if let Some(_idle) = idle {
        return Ok(
            json!({"state":"idle","cancel_requested":false,"next_action":"status","work_dir":root}),
        );
    }
    let control: Control = read(&root.join("job-control.json"))?;
    if control.schema_version != 1 || control.generation == 0 {
        return Err(protocol::failure(
            "job_invalid",
            Some(false),
            "invalid active job control",
        ));
    }
    write(&root.join("job-cancel.json"), &control)?;
    // The request is acknowledged, not process termination. A subsequent run has
    // a different generation and ignores this request even if publication races.
    Ok(
        json!({"state":"cancel_requested","cancel_requested":true,"next_action":"poll_status","work_dir":root}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_of_an_empty_directory_does_not_create_a_job_or_lock() {
        let (root, checkpoint) = crate::checkpoint::tests::fixture();
        drop(checkpoint);
        let before = fs::read(root.join("state.json")).unwrap();
        let entries = fs::read_dir(&root).unwrap().count();
        assert_eq!(status(&root).unwrap()["state"], "not_started");
        assert!(!root.join(".agent-youtube.lock").exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), entries);
        assert_eq!(fs::read(root.join("state.json")).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_is_scoped_to_the_current_locked_generation() {
        let (root, _checkpoint) = crate::checkpoint::tests::fixture();
        let options = Options {
            url: "https://youtu.be/a2hbJlPxj4M".into(),
            work_dir: root.clone(),
            model_dir: root.join("models"),
            gigastt: "gigastt".into(),
            chunk_seconds: 300,
            boundary_window_ms: 0,
            asr_timeout_seconds: 1800,
            asr_memory_mib: None,
            encoder_threads: None,
            retry_window_seconds: 0,
        };
        let initializing = storage::Lock::control(&root).unwrap();
        let lock = storage::Lock::acquire(&root).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = Active::start(&options, cancelled.clone(), true).unwrap();
        drop(initializing);
        assert_eq!(status(&root).unwrap()["state"], "running");
        assert_eq!(cancel(&root).unwrap()["cancel_requested"], true);
        for _ in 0..100 {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(cancelled.load(Ordering::SeqCst));
        drop(guard);
        drop(lock);
        assert_eq!(cancel(&root).unwrap()["cancel_requested"], false);
        let initializing = storage::Lock::control(&root).unwrap();
        let lock = storage::Lock::acquire(&root).unwrap();
        let error = cancel(&root).unwrap_err();
        assert_eq!(
            error.downcast_ref::<protocol::Failure>().unwrap().code,
            "work_directory_busy"
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = Active::start(&options, cancelled.clone(), true).unwrap();
        drop(initializing);
        thread::sleep(Duration::from_millis(250));
        assert!(
            !cancelled.load(Ordering::SeqCst),
            "stale request cancelled a new generation"
        );
        drop(guard);
        drop(lock);
        fs::remove_dir_all(root).unwrap();
    }
}
