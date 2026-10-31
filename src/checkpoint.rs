mod paths;
use paths::*;
mod journal;
use crate::{Result, storage};
use journal::{Anchor, Change, Journal};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
fn file_identity(path: &Path) -> std::io::Result<Option<[u64; 9]>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(None);
    }
    Ok(Some([
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.nlink(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
        u64::from(metadata.mode()),
    ]))
}

/// Failed evidence checks never authorize overwriting or removing the reported path.
pub(crate) fn evidence_failure(
    code: &'static str,
    path: &Path,
    message: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        code,
        Some(false),
        message,
        serde_json::json!({"path":path.to_string_lossy(),"preserved":true}),
    )
}
pub(crate) fn evidence_context(
    code: &'static str,
    path: &Path,
    error: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    let error = storage::audio_write_error(error, path, "checkpoint");
    if error.is::<crate::protocol::Failure>() {
        return error;
    }
    if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() != std::io::ErrorKind::NotFound)
    {
        return crate::protocol::failure_details(
            "checkpoint_unavailable",
            None,
            error,
            serde_json::json!({"path":path.to_string_lossy(),"preserved":true}),
        );
    }
    evidence_failure(code, path, error)
}

#[cfg(test)]
static SAVED_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub(crate) struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<crate::time_range::TimeRange>,
    pub url: String,
    pub model_dir: PathBuf,
    #[serde(default)]
    pub model_variant: crate::pipeline::ModelVariant,
    pub model_fingerprint: BTreeMap<String, String>,
    pub gigastt_version: String,
    pub gigastt_binary: String,
    pub chunk_seconds: u32,
    #[serde(default)]
    pub boundary_window_ms: u32,
    #[serde(default)]
    pub encoder_threads: Option<usize>,
    #[serde(default)]
    pub media_backend: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub engine_environment: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Publication {
    pending: String,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct State {
    application: String,
    schema_version: u32,
    config: Config,
    artifacts: BTreeMap<String, String>,
    #[serde(default)]
    publications: BTreeMap<String, Publication>,
    #[serde(default)]
    scratch: BTreeSet<String>,
    #[serde(default)]
    download_prefix: Option<String>,
    #[serde(default)]
    retired_downloads: Vec<String>,
    #[serde(default)]
    batch: Option<Batch>,
    #[serde(default)]
    retired_batches: Vec<Batch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal: Option<Anchor>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Batch {
    pub namespace: String,
    pub indices: Vec<usize>,
}
impl Batch {
    pub fn dirs(&self, root: &Path) -> Result<(PathBuf, PathBuf)> {
        if self.indices.is_empty() || self.indices.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("invalid ASR batch indices".into());
        }
        let fields: Vec<_> = self
            .namespace
            .strip_prefix("batch.")
            .ok_or("invalid batch namespace")?
            .split('.')
            .collect();
        if fields.len() != 3
            || fields
                .iter()
                .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("invalid batch namespace".into());
        }
        let dir = root.join(&self.namespace);
        for path in [&dir, &dir.join("input"), &dir.join("output")] {
            if fs::symlink_metadata(path).is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink()) {
                return Err("refusing nonregular batch directory".into());
            }
        }
        Ok((dir.join("input"), dir.join("output")))
    }
}

pub(crate) struct Checkpoint {
    root: PathBuf,
    state: State,
    sequence: AtomicU64,
    journal: Option<Journal>,
    #[cfg(unix)]
    verified: std::cell::RefCell<BTreeMap<String, ([u64; 9], String)>>,
}
impl Checkpoint {
    fn load(root: &Path) -> Result<Option<Self>> {
        Self::read(root).map_err(|error| {
            evidence_context("checkpoint_invalid", &root.join("state.json"), error)
        })
    }
    fn read(root: &Path) -> Result<Option<Self>> {
        let path = root.join("state.json");
        storage::reject_nonregular(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::metadata(&path)?.len();
        crate::memory::require(bytes.saturating_mul(16), &path, "checkpoint_load")?;
        let mut state: State = serde_json::from_str(&storage::read_text(&path, bytes)?)?;
        if state.application != "agent-youtube" {
            return Err("checkpoint belongs to another application; use a new --work-dir".into());
        }
        if (state.schema_version == 5) != state.config.range.is_some() {
            return Err(
                "ranged jobs require checkpoint schema5; whole-video jobs use schemas1..4".into(),
            );
        }
        if let Some(range) = state.config.range {
            range.validate()?;
        }
        let journal = match (state.schema_version, state.journal.clone()) {
            (1, None) => None,
            (2..=5, Some(anchor)) => Some(
                Journal::load(root, &anchor, bytes, &mut state).map_err(|error| {
                    evidence_context(
                        "checkpoint_journal_invalid",
                        &root.join(&anchor.directory),
                        error,
                    )
                })?,
            ),
            _ => return Err("unsupported or invalid checkpoint schema".into()),
        };
        if state.retired_downloads.len() > 2 {
            return Err("invalid acquisition count".into());
        }
        let mut prefixes = BTreeSet::new();
        for prefix in state.download_prefix.iter().chain(&state.retired_downloads) {
            validate_download_prefix(prefix)?;
            if !prefixes.insert(prefix) {
                return Err("duplicate acquisition namespace".into());
            }
        }
        Ok(Some(Self {
            root: root.to_owned(),
            state,
            sequence: AtomicU64::new(0),
            journal,
            #[cfg(unix)]
            verified: std::cell::RefCell::new(BTreeMap::new()),
        }))
    }
    fn duration_evidence(&self) -> Result<(u64, Option<u64>)> {
        let unavailable = || {
            crate::protocol::failure_details(
                "source_duration_unavailable",
                Some(false),
                "cannot establish the selected recording duration; evidence preserved",
                serde_json::json!({"stage":"audio_validation","path":self.root.join("source.json"),"preserved":true}),
            )
        };
        if !self.completed("source.json")? || !self.completed("download.json")? {
            return Err(unavailable());
        }
        let source: serde_json::Value = serde_json::from_str(&storage::read_text(
            &self.root.join("source.json"),
            16 * 1024 * 1024,
        )?)?;
        let download: serde_json::Value = serde_json::from_str(&storage::read_text(
            &self.root.join("download.json"),
            1024 * 1024,
        )?)?;
        let initial = source["videoDetails"]["lengthSeconds"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| source["duration"].as_u64())
            .filter(|seconds| *seconds > 0);
        if let Some(duration) = download
            .get("duration_seconds")
            .filter(|value| !value.is_null())
        {
            let selected = duration
                .as_u64()
                .filter(|seconds| *seconds > 0)
                .ok_or_else(unavailable)?;
            if initial.is_some_and(|original| original.abs_diff(selected) > 2) {
                return Err(crate::protocol::failure_details(
                    "source_duration_conflict",
                    Some(false),
                    "initial and selected recording durations conflict; evidence preserved",
                    serde_json::json!({"stage":"audio_validation","initial_seconds":initial,"selected_seconds":selected,"tolerance_seconds":2,"preserved":true}),
                ));
            }
            return Ok((selected, initial));
        }
        // Legacy evidence has no selected duration. Its original metadata is
        // usable only when it describes the exact retained representation.
        let identity = &download["representation"];
        let same_video = source["videoDetails"]["videoId"]
            .as_str()
            .is_some_and(|id| Some(id) == identity["video_id"].as_str());
        let matching = source["streamingData"]["adaptiveFormats"]
            .as_array()
            .is_some_and(|formats| {
                formats.iter().any(|format| {
                    format["itag"] == identity["itag"]
                        && format["lastModified"] == identity["last_modified"]
                        && format["mimeType"] == identity["mime"]
                        && format["contentLength"]
                            .as_str()
                            .and_then(|value| value.parse::<u64>().ok())
                            == identity["length"].as_u64()
                        && format["xtags"].as_str().unwrap_or("")
                            == identity["xtags"].as_str().unwrap_or("")
                        && format
                            .pointer("/audioTrack/id")
                            .and_then(serde_json::Value::as_str)
                            == identity["audio_track"].as_str()
                })
            });
        if !same_video || !matching {
            return Err(unavailable());
        }
        let expected = initial.ok_or_else(unavailable)?;
        Ok((expected, Some(expected)))
    }
    pub(crate) fn source_duration(&self) -> Result<u64> {
        Ok(self.duration_evidence()?.0)
    }
    pub(crate) fn validate_duration(&self, actual_seconds: f64) -> Result<()> {
        let (expected, initial) = self.duration_evidence()?;
        if !actual_seconds.is_finite()
            || actual_seconds <= 0.0
            || (actual_seconds - expected as f64).abs() > 2.0
            || initial.is_some_and(|original| (actual_seconds - original as f64).abs() > 2.0)
        {
            return Err(crate::protocol::failure_details(
                "audio_duration_mismatch",
                Some(false),
                "decoded recording duration disagrees with selected source; evidence preserved",
                serde_json::json!({"stage":"audio_validation","expected_seconds":expected,"initial_seconds":initial,"decoded_seconds":actual_seconds,"tolerance_seconds":2,"preserved":true}),
            ));
        }
        Ok(())
    }
    pub(crate) fn job_status(root: &Path) -> Result<Option<serde_json::Value>> {
        let Some(checkpoint) = Self::load(root)? else {
            return Ok(None);
        };
        let published = checkpoint.state.artifacts.contains_key("transcript.json");
        if published {
            for name in checkpoint
                .state
                .artifacts
                .keys()
                .filter(|name| !media_artifact(name))
            {
                if !checkpoint.completed(name)? {
                    return Err(evidence_failure(
                        "checkpoint_artifact_missing",
                        &root.join(name),
                        "published evidence is missing",
                    ));
                }
            }
            if !checkpoint.completed("transcript.json")? {
                return Err(evidence_failure(
                    "checkpoint_artifact_missing",
                    &root.join("transcript.json"),
                    "published transcript is missing",
                ));
            }
            let header = crate::transcript::read_streaming(
                std::io::BufReader::new(storage::open_regular_input(
                    &root.join("transcript.json"),
                )?),
                |_| Ok(()),
            )?;
            if header.range.map(|r| r.requested) != checkpoint.state.config.range {
                return Err("published range differs from saved configuration".into());
            }
            checkpoint.validate_duration(header.duration)?;
        }
        let cleanup_complete = published
            && checkpoint.state.download_prefix.is_none()
            && checkpoint.state.retired_downloads.is_empty()
            && checkpoint.state.scratch.is_empty()
            && checkpoint.state.batch.is_none()
            && checkpoint.state.retired_batches.is_empty()
            && !checkpoint
                .state
                .artifacts
                .keys()
                .any(|name| media_artifact(name));
        Ok(Some(serde_json::json!({
            "state":if published { if cleanup_complete {"completed"} else {"cleanup_pending"} } else {"resumable"},
            "next_action":if cleanup_complete {"read_transcript"} else {"resume"},
            "transcript_validated":published,"cleanup_complete":cleanup_complete,
            "transcript":if published {Some(root.join("transcript.json"))}else{None},
            "retained_acquisitions":checkpoint.state.retired_downloads.len() + usize::from(checkpoint.state.download_prefix.is_some())
        })))
    }
    /// Reject a model change before journal recovery or publication cleanup.
    pub(crate) fn validate_model(root: &Path, model: crate::pipeline::ModelVariant) -> Result<()> {
        if let Some(checkpoint) = Self::load(root)?
            && checkpoint.state.config.model_variant != model
        {
            return Err(evidence_failure(
                "checkpoint_settings_mismatch",
                &root.join("state.json"),
                "model variant changed; use a new --work-dir to preserve previous results",
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_range(
        root: &Path,
        range: Option<crate::time_range::TimeRange>,
    ) -> Result<()> {
        if let Some(checkpoint) = Self::load(root)?
            && checkpoint.state.config.range != range
        {
            return Err(evidence_failure(
                "checkpoint_settings_mismatch",
                &root.join("state.json"),
                "recognition range changed; use a new work directory",
            ));
        }
        Ok(())
    }
    /// Caller holds the run lock; this check reads saved state without recovery.
    pub(crate) fn validate_backend(root: &Path, backend: &str) -> Result<()> {
        if let Some(checkpoint) = Self::load(root)?
            && checkpoint.state.config.media_backend != backend
        {
            return Err(evidence_failure(
                "checkpoint_settings_mismatch",
                &root.join("state.json"),
                "media backend changed; use a new --work-dir to preserve previous results",
            ));
        }
        Ok(())
    }
    pub fn open(root: &Path, config: Config) -> Result<Self> {
        if let Some(checkpoint) = Self::load(root)? {
            if checkpoint.state.config != config {
                return Err(evidence_failure(
                    "checkpoint_settings_mismatch",
                    &root.join("state.json"),
                    "run settings, media backend, model, engine or environment changed; use a new --work-dir to preserve previous results",
                ));
            }
            return Ok(checkpoint);
        }
        for dir in ["media", "chunks", "raw"] {
            if fs::read_dir(root.join(dir))?.next().is_some() {
                return Err("unowned run artifacts: use an empty --work-dir".into());
            }
        }
        if fs::symlink_metadata(root.join("transcript.json")).is_ok() {
            return Err("transcript.json already exists; use a new --work-dir".into());
        }
        let mut checkpoint = Self {
            root: root.to_owned(),
            state: State {
                application: "agent-youtube".into(),
                schema_version: 1,
                config,
                artifacts: BTreeMap::new(),
                publications: BTreeMap::new(),
                scratch: BTreeSet::new(),
                download_prefix: None,
                retired_downloads: Vec::new(),
                batch: None,
                retired_batches: Vec::new(),
                journal: None,
            },
            sequence: AtomicU64::new(0),
            journal: None,
            #[cfg(unix)]
            verified: std::cell::RefCell::new(BTreeMap::new()),
        };
        checkpoint.compact()?;
        Ok(checkpoint)
    }
    pub fn finished(root: &Path, url: &str) -> Result<Option<Self>> {
        let Some(checkpoint) = Self::load(root)? else {
            return Ok(None);
        };
        if checkpoint.state.config.url != url {
            return Err(evidence_failure(
                "checkpoint_settings_mismatch",
                &root.join("state.json"),
                "run settings refer to another source URL",
            ));
        }
        if !checkpoint.state.artifacts.contains_key("transcript.json")
            && !checkpoint
                .state
                .publications
                .contains_key("transcript.json")
        {
            return Ok(None);
        }
        Ok(Some(checkpoint))
    }
    pub fn batch(&self) -> Option<Batch> {
        self.state.batch.clone()
    }
    pub fn begin_batch(&mut self, indices: Vec<usize>) -> Result<Batch> {
        if self.state.batch.is_some() {
            return Err("unfinished ASR batch".into());
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let batch = Batch {
            namespace: format!(
                "batch.{}.{stamp}.{}",
                std::process::id(),
                self.sequence.fetch_add(1, Ordering::Relaxed)
            ),
            indices,
        };
        let (input, output) = batch.dirs(&self.root)?;
        if self.root.join(&batch.namespace).exists() {
            return Err("batch namespace already exists".into());
        }
        self.state.batch = Some(batch.clone());
        self.persist(Change::BeginBatch {
            batch: batch.clone(),
        })?;
        self.compact()?;
        storage::private_directory(&self.root.join(&batch.namespace), false)?;
        storage::private_directory(&input, false)?;
        storage::private_directory(&output, false)?;
        storage::sync_parent(&self.root.join(&batch.namespace))?;
        Ok(batch)
    }
    pub fn retire_batch(&mut self) -> Result<()> {
        if let Some(batch) = self.state.batch.take() {
            self.state.retired_batches.push(batch);
            self.persist(Change::RetireBatch)?;
        }
        Ok(())
    }
    fn finish_batches(&mut self) -> Result<()> {
        if self.state.batch.is_none() && self.state.retired_batches.is_empty() {
            return Ok(());
        }
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for batch in self
            .state
            .retired_batches
            .iter()
            .chain(self.state.batch.iter())
        {
            let (input, output) = batch.dirs(&self.root)?;
            let parent = self.root.join(&batch.namespace);
            if parent.exists() {
                for entry in fs::read_dir(&parent)? {
                    let path = entry?.path();
                    if path != input && path != output {
                        return Err("unowned batch entry; preserved".into());
                    }
                }
            }
            for (dir, ext) in [(&input, "wav"), (&output, "json")] {
                if !dir.exists() {
                    continue;
                }
                for entry in fs::read_dir(dir)? {
                    let path = entry?.path();
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .ok_or("invalid batch filename")?;
                    let index = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse::<usize>().ok())
                        .ok_or("invalid batch filename")?;
                    if batch.indices.binary_search(&index).is_err()
                        || name != format!("{index:06}.{ext}")
                    {
                        return Err("unowned batch file; preserved".into());
                    }
                    storage::reject_nonregular(&path)?;
                    files.push(path);
                }
            }
            dirs.extend([input, output, parent]);
        }
        // The complete set is preflighted before removing any owned staging file.
        for path in files {
            fs::remove_file(&path)?;
            storage::sync_parent(&path)?;
        }
        for dir in dirs {
            if dir.exists() {
                fs::remove_dir(&dir)?;
                storage::sync_parent(&dir)?;
            }
        }
        self.state.batch = None;
        self.state.retired_batches.clear();
        self.persist(Change::ClearBatches)
    }
    pub fn original_acquisition(&self) -> Option<PathBuf> {
        self.state
            .retired_downloads
            .first()
            .map(|prefix| self.root.join(prefix))
    }
    pub fn retire_download(&mut self) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.anchor()?;
        }
        if self.state.retired_downloads.len() >= 2 {
            return Err(crate::protocol::failure_details(
                "download_acquisition_exhausted",
                Some(false),
                "three incompatible audio acquisitions; all attempts preserved",
                serde_json::json!({"attempts":3,"preserved":true,"next_action":"new_work_directory"}),
            ));
        }
        if self.state.artifacts.contains_key("media/source.audio")
            || self.state.publications.contains_key("media/source.audio")
        {
            return Err("cannot retire a published acquisition".into());
        }
        let prefix = self
            .state
            .download_prefix
            .take()
            .ok_or("missing acquisition namespace")?;
        validate_download_prefix(&prefix)?;
        self.state.retired_downloads.push(prefix);
        self.persist(Change::RetireDownload)
    }
    pub fn begin_download(&mut self) -> Result<PathBuf> {
        if let Some(prefix) = &self.state.download_prefix {
            validate_download_prefix(prefix)?;
            return Ok(self.root.join(prefix));
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let prefix = format!(
            "media/download.{}.{stamp}.{}",
            std::process::id(),
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let basename = Path::new(&prefix)
            .file_name()
            .ok_or("invalid download prefix")?
            .to_string_lossy();
        for entry in fs::read_dir(self.root.join("media"))? {
            if entry?
                .file_name()
                .to_string_lossy()
                .starts_with(basename.as_ref())
            {
                return Err("download namespace already exists".into());
            }
        }
        self.state.download_prefix = Some(prefix.clone());
        self.persist(Change::Download {
            prefix: prefix.clone(),
        })?;
        self.compact()?;
        Ok(self.root.join(prefix))
    }
    fn validate_source_namespace(&self, name: &str, pending: &str) -> Result<()> {
        if name == "media/source.audio" {
            let prefix = self
                .state
                .download_prefix
                .as_deref()
                .ok_or("missing download namespace")?;
            validate_download_prefix(prefix)?;
            if !download_output(pending, prefix) {
                return Err("audio publication is outside its download namespace".into());
            }
        }
        Ok(())
    }
    pub fn ensure_manifest(
        &mut self,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<PathBuf>> {
        let path = self.root.join("manifest.json");
        if self.completed("manifest.json")? {
            // A manifest hash cannot prove that its retained evidence still exists.
            // Media may already be removed by an interrupted cleanup.
            for name in self
                .state
                .artifacts
                .keys()
                .filter(|name| !media_artifact(name))
            {
                self.completed(name)?;
            }
            return Ok(Some(path));
        }
        // Old completed runs cannot reconstruct a source digest or selection.
        if !self.completed("download.json")? {
            return Ok(None);
        }
        if !self.completed("transcript.json")? {
            return Err("manifest requires a validated transcript".into());
        }
        let download: serde_json::Value = serde_json::from_str(&storage::read_text(
            &self.root.join("download.json"),
            1024 * 1024,
        )?)?;
        let transcript_digest = self
            .state
            .artifacts
            .get("transcript.json")
            .ok_or("missing transcript digest")?;
        let transcript_path = self.root.join("transcript.json");
        let header = crate::transcript::read_streaming(
            std::io::BufReader::new(fs::File::open(&transcript_path)?),
            |_| Ok(()),
        )?;
        let coverage = if self.completed("coverage.json")? {
            crate::agent::Coverage::load(&transcript_path, header.duration)?
        } else {
            None
        };
        if header.range.map(|r| r.requested) != self.state.config.range {
            return Err("published range differs from saved invocation".into());
        }
        if let Some(coverage) = &coverage {
            coverage.validate_range(&header)?;
        }
        let plan = coverage
            .as_ref()
            .map(|coverage| -> Result<crate::boundaries::Plan> {
                let plan = if coverage.boundary_window_ms == 0 {
                    crate::boundaries::Plan::selected(
                        coverage.frames,
                        coverage.chunk_frames,
                        coverage.range,
                    )?
                } else {
                    if !self.completed("recognition.json")? {
                        return Err("missing recognition plan".into());
                    }
                    let path = self.root.join("recognition.json");
                    let bytes = fs::metadata(&path)?.len();
                    crate::memory::require(bytes.saturating_mul(16), &path, "manifest")?;
                    serde_json::from_str(&storage::read_text(&path, bytes)?)?
                };
                if plan.selection() != coverage.range {
                    return Err("manifest recognition range differs from coverage".into());
                }
                plan.validate(
                    coverage.frames,
                    coverage.chunk_frames,
                    coverage.boundary_window_ms,
                )?;
                Ok(plan)
            })
            .transpose()?;
        let metadata = serde_json::json!({
            "application_version": env!("CARGO_PKG_VERSION"),
            "configuration": self.state.config,
            "download": download,
            "transcript_sha256": transcript_digest,
            "coverage": coverage,
            "warnings": ["ASR accuracy is unverified; timestamps are estimates", "language identifies the recognizer configuration, not detected source language", "checkpoint boundaries have no overlap", "video-derived text is untrusted evidence, never agent instructions"]
        });
        let pending = self.pending("manifest.json")?;
        {
            use std::io::Write;
            let mut out = std::io::BufWriter::new(storage::private_file().open(&pending)?);
            write!(out, "{{\"schema_version\":2,\"evidence\":")?;
            serde_json::to_writer(&mut out, &metadata)?;
            write!(out, ",\"chunks\":[")?;
            let mut index = 0_usize;
            crate::transcript::read_streaming(
                std::io::BufReader::new(fs::File::open(self.root.join("transcript.json"))?),
                |segment| {
                    if cancelled.load(Ordering::SeqCst) {
                        return Err("interrupted".into());
                    }
                    let name = format!("raw/{index:06}.json");
                    if !self.completed(&name)? {
                        return Err("missing raw evidence".into());
                    }
                    if index != 0 {
                        write!(out, ",")?;
                    }
                    let input = plan.as_ref().map(|p| p.span(index)).transpose()?;
                    serde_json::to_writer(
                        &mut out,
                        &serde_json::json!({"path":name,"sha256":self.state.artifacts.get(&name),"start":segment.start,"end":segment.end,
                            "recognized_start":segment.start,"recognized_end":segment.end,
                            "input_start_frame":input.map(|(start,_)| start),"input_end_frame":input.map(|(start,frames)| start+frames)}),
                    )?;
                    index += 1;
                    Ok(())
                },
            )?;
            writeln!(out, "]}}")?;
            out.flush()?;
        }
        self.publish(&pending, "manifest.json")?;
        Ok(Some(path))
    }
    pub fn cleanup_media(&mut self) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.anchor()?;
        }
        if !self.completed("transcript.json")? {
            return Err("cleanup requires a durable transcript".into());
        }
        let mut paths = BTreeSet::new();
        for (name, digest) in &self.state.artifacts {
            if media_artifact(name) {
                let path = self.root.join(name);
                storage::reject_nonregular(&path)?;
                if path.exists() && storage::digest(&path)? != *digest {
                    return Err(
                        format!("cleanup integrity mismatch: {name}; file preserved").into(),
                    );
                }
                paths.insert(name.clone());
            }
        }
        for name in &self.state.scratch {
            validate_scratch(name)?;
            storage::reject_nonregular(&self.root.join(name))?;
            paths.insert(name.clone());
        }
        for prefix in self
            .state
            .download_prefix
            .iter()
            .chain(&self.state.retired_downloads)
        {
            validate_download_prefix(prefix)?;
            let basename = Path::new(prefix)
                .file_name()
                .ok_or("invalid download prefix")?
                .to_string_lossy();
            for entry in fs::read_dir(self.root.join("media"))? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{basename}."))
                {
                    storage::reject_nonregular(&entry.path())?;
                    paths.insert(format!(
                        "media/{}",
                        entry
                            .file_name()
                            .to_str()
                            .ok_or("non-UTF-8 download scratch")?
                    ));
                }
            }
        }
        self.finish_batches()?;
        // Preflight every target before deleting any. Missing paths mean an interrupted cleanup.
        for name in &paths {
            let path = self.root.join(name);
            if path.exists() {
                fs::remove_file(&path)?;
                storage::sync_parent(&path)?;
            }
        }
        if !paths.is_empty()
            || self.state.download_prefix.is_some()
            || !self.state.retired_downloads.is_empty()
        {
            self.state.artifacts.retain(|name, _| !media_artifact(name));
            self.state.scratch.clear();
            self.state.download_prefix = None;
            self.state.retired_downloads.clear();
            self.persist(Change::Cleanup)?;
        }
        self.compact()?;
        Ok(())
    }
    pub fn recover(&mut self) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.anchor()?;
        }
        for name in self.state.publications.keys().cloned().collect::<Vec<_>>() {
            crate::protocol::event(format!("recovering interrupted publication: {name}"));
            self.finish_publication(&name)?;
        }
        Ok(())
    }
    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
    fn persist(&mut self, change: Change) -> Result<()> {
        self.persist_change(change)
            .map_err(|e| storage::audio_write_error(e, &self.state_path(), "checkpoint"))
    }
    fn persist_change(&mut self, change: Change) -> Result<()> {
        // Mutations are already applied in memory. A legacy migration snapshots
        // that change directly rather than replaying it twice after the anchor.
        if self.state.schema_version
            < if self.state.config.range.is_some() {
                5
            } else {
                4
            }
        {
            return self.compact();
        }
        if let Some(journal) = &mut self.journal {
            journal.append(change)?;
            if !journal.needs_snapshot() {
                return Ok(());
            }
        }
        self.compact()
    }
    fn compact(&mut self) -> Result<()> {
        if self.journal.is_none() {
            self.journal = Some(Journal::create(
                &self.root,
                self.sequence.fetch_add(1, Ordering::Relaxed),
            )?);
        }
        let journal = self.journal.as_mut().ok_or("missing checkpoint journal")?;
        let anchor = journal.anchor()?;
        let schema = if self.state.config.range.is_some() {
            5
        } else {
            4
        };
        if self.state.schema_version == schema && self.state.journal.as_ref() == Some(&anchor) {
            return Ok(());
        }
        let planned = (self.state.artifacts.len() as u64)
            .saturating_mul(512)
            .saturating_add((self.state.publications.len() as u64).saturating_mul(512))
            .saturating_add((self.state.scratch.len() as u64).saturating_mul(256))
            .saturating_add(
                self.state
                    .batch
                    .iter()
                    .chain(self.state.retired_batches.iter())
                    .fold(0_u64, |bytes, batch| {
                        bytes.saturating_add((batch.indices.len() as u64).saturating_mul(64))
                    }),
            )
            .saturating_add(64 << 10);
        if let Err(error) =
            crate::memory::require(planned, &self.state_path(), "checkpoint_snapshot")
        {
            if let Some(journal) = self.journal.as_mut() {
                journal.invalidate();
            }
            return Err(error);
        }
        self.state.schema_version = schema;
        self.state.journal = Some(anchor);
        let bytes = match serde_json::to_vec_pretty(&self.state) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.journal
                    .as_mut()
                    .ok_or("missing checkpoint journal")?
                    .invalidate();
                return Err(error.into());
            }
        };
        #[cfg(test)]
        SAVED_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if let Err(error) = storage::atomic_write(&self.state_path(), &bytes, true) {
            self.journal
                .as_mut()
                .ok_or("missing checkpoint journal")?
                .invalidate();
            return Err(error);
        }
        self.journal
            .as_mut()
            .ok_or("missing checkpoint journal")?
            .snapshotted(bytes.len() as u64);
        Ok(())
    }
    pub fn latest_decode_resume(&self) -> Option<(usize, String)> {
        self.state
            .artifacts
            .keys()
            .filter_map(|name| {
                let index = name
                    .strip_prefix("chunks/")?
                    .strip_suffix(".resume.json")?
                    .parse::<usize>()
                    .ok()?;
                Some((index, name.clone()))
            })
            .max_by_key(|(index, _)| *index)
    }
    pub fn chunk_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.state.artifacts.keys().filter_map(|name| {
            name.strip_prefix("chunks/")?
                .strip_suffix(".wav")?
                .parse()
                .ok()
        })
    }
    /// Scheduling only: actual consumers and cleanup still force full hashes.
    pub fn verified(&self, name: &str) -> Result<bool> {
        #[cfg(unix)]
        {
            let Some(expected) = self.state.artifacts.get(name) else {
                return Ok(false);
            };
            validate_relative(name)?;
            if let Ok(Some(identity)) = file_identity(&self.root.join(name))
                && self
                    .verified
                    .borrow()
                    .get(name)
                    .is_some_and(|entry| entry.0 == identity && entry.1 == *expected)
            {
                return Ok(true);
            }
        }
        self.completed(name)
    }
    pub fn completed(&self, name: &str) -> Result<bool> {
        validate_relative(name)
            .map_err(|error| evidence_context("checkpoint_invalid", &self.state_path(), error))?;
        let Some(expected) = self.state.artifacts.get(name) else {
            return Ok(false);
        };
        let path = self.root.join(name);
        storage::reject_nonregular(&path)
            .map_err(|error| evidence_context("checkpoint_artifact_invalid", &path, error))?;
        if !path.is_file() {
            return Err(evidence_failure(
                "checkpoint_artifact_missing",
                &path,
                format!(
                    "checkpoint integrity mismatch: missing {}; preserve the damaged run and use a new --work-dir",
                    path.display()
                ),
            ));
        }
        #[cfg(unix)]
        let before = file_identity(&path).ok().flatten();
        if storage::digest(&path)
            .map_err(|error| evidence_context("checkpoint_artifact_invalid", &path, error))?
            != *expected
        {
            return Err(evidence_failure(
                "checkpoint_integrity_mismatch",
                &path,
                format!(
                    "checkpoint integrity mismatch: {}; preserve the damaged run and use a new --work-dir",
                    path.display()
                ),
            ));
        }
        #[cfg(unix)]
        if let Some(identity) = before
            && file_identity(&path).ok().flatten() == Some(identity)
        {
            self.verified
                .borrow_mut()
                .insert(name.into(), (identity, expected.clone()));
        }
        Ok(true)
    }
    pub fn publish(&mut self, pending: &Path, name: &str) -> Result<()> {
        self.publish_many(&[(pending.to_owned(), name.to_owned())])
    }
    pub fn publish_many(&mut self, files: &[(PathBuf, String)]) -> Result<()> {
        self.publish_group(files)
            .map_err(|e| storage::audio_write_error(e, &self.root, "publication"))
    }
    fn publish_group(&mut self, files: &[(PathBuf, String)]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let mut intents = BTreeMap::new();
        for (pending, name) in files {
            let publication = self.prepare_publication(pending, name)?;
            if intents.insert(name.clone(), publication).is_some()
                || self.state.publications.contains_key(name)
            {
                return Err("duplicate or unfinished publication".into());
            }
        }
        self.state.publications.extend(intents.clone());
        // All intents are durable before linking any destination. Recovery can
        // finish every file even if the process stops midway through this group.
        self.persist(Change::Intents { entries: intents })?;
        if files
            .iter()
            .any(|(_, name)| !name.starts_with("chunks/") && !name.starts_with("raw/"))
        {
            self.compact()?;
        }
        for (_, name) in files {
            self.link_publication(name)?;
        }
        self.persist(Change::Published {
            names: files.iter().map(|(_, name)| name.clone()).collect(),
        })
    }
    fn prepare_publication(&self, pending: &Path, name: &str) -> Result<Publication> {
        let relative = pending
            .strip_prefix(&self.root)?
            .to_str()
            .ok_or("pending path must be UTF-8")?
            .to_owned();
        validate_publication(name, &relative)?;
        self.validate_source_namespace(name, &relative)?;
        storage::reject_nonregular(pending)?;
        fs::File::open(pending)?.sync_all()?;
        storage::sync_parent(pending)?;
        Ok(Publication {
            pending: relative,
            digest: storage::digest(pending)?,
        })
    }
    fn finish_publication(&mut self, name: &str) -> Result<()> {
        self.link_publication(name)?;
        self.persist(Change::Published {
            names: vec![name.to_owned()],
        })
    }
    fn link_publication(&mut self, name: &str) -> Result<()> {
        self.link_publication_inner(name).map_err(|error| {
            evidence_context("checkpoint_recovery_failed", &self.root.join(name), error)
        })
    }
    fn link_publication_inner(&mut self, name: &str) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.anchor()?;
        }
        let publication = self
            .state
            .publications
            .get(name)
            .ok_or("missing publication intent")?
            .clone();
        validate_publication(name, &publication.pending)?;
        self.validate_source_namespace(name, &publication.pending)?;
        let pending = self.root.join(&publication.pending);
        let destination = self.root.join(name);
        storage::reject_nonregular(&pending)?;
        storage::reject_nonregular(&destination)?;
        let source = if destination.exists() {
            if storage::digest(&destination)? != publication.digest {
                return Err(evidence_failure(
                    "checkpoint_publication_conflict",
                    &destination,
                    format!(
                        "publication conflicts with existing {}; file preserved",
                        destination.display()
                    ),
                ));
            }
            &destination
        } else {
            if storage::digest(&pending)? != publication.digest {
                return Err(evidence_failure(
                    "checkpoint_pending_invalid",
                    &pending,
                    "pending publication failed integrity verification",
                ));
            }
            &pending
        };
        if name == "transcript.json" {
            let header = crate::transcript::read_streaming(
                std::io::BufReader::new(storage::open_regular_input(source)?),
                |_| Ok(()),
            )?;
            self.validate_duration(header.duration)?;
        }
        if source == &pending {
            fs::hard_link(&pending, &destination)?;
        }
        fs::File::open(&destination)?.sync_all()?;
        storage::sync_parent(&destination)?;
        if pending.exists() {
            if storage::digest(&pending)? != publication.digest {
                return Err(evidence_failure(
                    "checkpoint_pending_invalid",
                    &pending,
                    "pending publication failed integrity verification",
                ));
            }
            fs::remove_file(&pending)?;
            storage::sync_parent(&pending)?;
        }
        self.state
            .artifacts
            .insert(name.into(), publication.digest.clone());
        self.state.publications.remove(name);
        self.state.scratch.remove(&publication.pending);
        // Publication just verified these owned bytes; only scheduling uses this memo.
        // Consumers and cleanup still hash independently.
        #[cfg(unix)]
        if let Ok(Some(identity)) = file_identity(&destination) {
            self.verified
                .borrow_mut()
                .insert(name.into(), (identity, publication.digest.clone()));
        }
        Ok(())
    }
    pub fn publish_bytes(&mut self, bytes: &[u8], name: &str) -> Result<()> {
        let pending = self.pending(name)?;
        storage::atomic_write(&pending, bytes, false)?;
        self.publish(&pending, name)
    }
    pub fn pending(&mut self, name: &str) -> Result<PathBuf> {
        self.pending_many(&[name])?
            .pop()
            .ok_or_else(|| "missing pending path".into())
    }
    pub fn pending_many(&mut self, names: &[&str]) -> Result<Vec<PathBuf>> {
        let paths = names
            .iter()
            .map(|name| self.reserve_pending(name))
            .collect::<Result<Vec<_>>>()?;
        if !paths.is_empty() {
            self.persist(Change::Scratch {
                names: paths
                    .iter()
                    .map(|path| {
                        path.strip_prefix(&self.root)?
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "non-UTF-8 scratch path".into())
                    })
                    .collect::<Result<Vec<_>>>()?,
            })?;
        }
        Ok(paths)
    }
    fn reserve_pending(&mut self, name: &str) -> Result<PathBuf> {
        validate_relative(name)?;
        let requested = Path::new(name);
        let filename = requested
            .file_name()
            .ok_or("missing pending file name")?
            .to_str()
            .ok_or("non-UTF-8 pending file name")?;
        let parent = requested.parent().unwrap_or(Path::new(""));
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let relative = parent.join(format!(
            ".agent-youtube-{}-{stamp}-{sequence}-{filename}",
            std::process::id()
        ));
        let relative = relative
            .to_str()
            .ok_or("scratch path must be UTF-8")?
            .to_owned();
        validate_scratch(&relative)?;
        self.state.scratch.insert(relative.clone());
        Ok(self.root.join(relative))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn missing_or_foreign_application_identity_is_rejected_without_mutation() {
        for application in [None, Some("foreign-application")] {
            let (root, checkpoint) = fixture();
            drop(checkpoint);
            let path = root.join("state.json");
            let mut state: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            state.as_object_mut().unwrap().remove("application");
            if let Some(application) = application {
                state["application"] = serde_json::json!(application);
            }
            let bytes = serde_json::to_vec(&state).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert!(Checkpoint::load(&root).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn schema3_partial_acquisition_migrates_without_rewriting_retained_evidence() {
        let (root, mut checkpoint) = fixture();
        checkpoint
            .publish_bytes(b"exact legacy ASR evidence", "raw/000000.json")
            .unwrap();
        let original = checkpoint.begin_download().unwrap();
        let media = PathBuf::from(format!("{}.m4a", original.display()));
        fs::write(&media, b"legacy partial media").unwrap();
        let snapshot = root.join("state.json");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
        legacy["schema_version"] = serde_json::json!(3);
        legacy.as_object_mut().unwrap().remove("retired_downloads");
        fs::write(&snapshot, serde_json::to_vec(&legacy).unwrap()).unwrap();
        drop(checkpoint);
        let mut current = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(current.begin_download().unwrap(), original);
        current.retire_download().unwrap();
        drop(current);
        let mut current = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(current.state.schema_version, 4);
        assert_ne!(current.begin_download().unwrap(), original);
        assert_eq!(fs::read(&media).unwrap(), b"legacy partial media");
        assert_eq!(
            fs::read(root.join("raw/000000.json")).unwrap(),
            b"exact legacy ASR evidence"
        );
        assert!(current.completed("raw/000000.json").unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn acquisition_retirement_survives_restart_and_cleans_only_after_publication() {
        let (root, mut checkpoint) = fixture();
        let first = checkpoint.begin_download().unwrap();
        let media = first.with_extension(format!(
            "{}.m4a",
            first.extension().unwrap().to_str().unwrap()
        ));
        fs::write(&media, b"partial original representation").unwrap();
        checkpoint.retire_download().unwrap();
        drop(checkpoint);
        let mut checkpoint = Checkpoint::load(&root).unwrap().unwrap();
        let second = checkpoint.begin_download().unwrap();
        assert_ne!(first, second);
        assert!(checkpoint.cleanup_media().is_err());
        assert_eq!(
            fs::read(&media).unwrap(),
            b"partial original representation"
        );
        checkpoint.retire_download().unwrap();
        drop(checkpoint);
        let mut checkpoint = Checkpoint::load(&root).unwrap().unwrap();
        checkpoint.begin_download().unwrap();
        let error = checkpoint.retire_download().unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "download_acquisition_exhausted"
        );
        assert!(media.exists());
        checkpoint
            .publish_bytes(br#"{"videoDetails":{"lengthSeconds":"1"}}"#, "source.json")
            .unwrap();
        checkpoint
            .publish_bytes(br#"{"duration_seconds":1}"#, "download.json")
            .unwrap();
        let transcript = serde_json::json!({"schema_version":1,"source":{"url":"fixture","video_id":"fixture","title":null},"language":"ru","duration":1.0,"engine":{"name":"fixture","version":"1","model_variant":"fixture"},"segments":[]});
        checkpoint
            .publish_bytes(&serde_json::to_vec(&transcript).unwrap(), "transcript.json")
            .unwrap();
        checkpoint.cleanup_media().unwrap();
        assert!(!media.exists());
        drop(checkpoint);
        let checkpoint = Checkpoint::load(&root).unwrap().unwrap();
        assert!(checkpoint.state.retired_downloads.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_replays_changes_without_rewriting_each_snapshot() {
        use std::io::Write;
        let (root, mut checkpoint) = fixture();
        let snapshot = fs::read(root.join("state.json")).unwrap();
        let pending = checkpoint.pending("raw/000000.json").unwrap();
        assert_eq!(fs::read(root.join("state.json")).unwrap(), snapshot);
        let anchor = checkpoint.state.journal.as_ref().unwrap();
        let log = root.join(&anchor.directory).join("events");
        fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(b"uncommitted torn record")
            .unwrap();
        let tail_length = fs::metadata(&log).unwrap().len();
        drop(checkpoint);
        let mut reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert!(
            reopened
                .state
                .scratch
                .contains(pending.strip_prefix(&root).unwrap().to_str().unwrap())
        );
        assert_eq!(
            fs::metadata(&log).unwrap().len(),
            tail_length,
            "loading must preserve uncommitted tail"
        );
        reopened.pending("raw/000001.json").unwrap();
        reopened.compact().unwrap();
        let reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(reopened.state.scratch.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_journal_damage_is_rejected_without_modifying_evidence() {
        use std::io::{Seek, SeekFrom, Write};
        for damage in [
            "truncate",
            "hash",
            "missing head",
            "head mismatch",
            "symlink",
            "hardlink",
        ] {
            let (root, mut checkpoint) = fixture();
            checkpoint.pending("raw/000000.json").unwrap();
            let directory = root.join(&checkpoint.state.journal.as_ref().unwrap().directory);
            let log = directory.join("events");
            drop(checkpoint);
            match damage {
                "truncate" => {
                    let file = fs::OpenOptions::new().write(true).open(&log).unwrap();
                    file.set_len(file.metadata().unwrap().len() - 1).unwrap();
                }
                "hash" => {
                    let mut file = fs::OpenOptions::new().write(true).open(&log).unwrap();
                    file.seek(SeekFrom::Start(8)).unwrap();
                    file.write_all(&[99; 32]).unwrap();
                }
                "missing head" => fs::remove_file(directory.join("head.json")).unwrap(),
                "head mismatch" => fs::write(directory.join("head.json"), b"{}").unwrap(),
                #[cfg(unix)]
                "symlink" => {
                    let target = root.join("retained-log");
                    fs::rename(&log, &target).unwrap();
                    std::os::unix::fs::symlink(target, &log).unwrap();
                }
                "hardlink" => fs::hard_link(&log, root.join("retained-log")).unwrap(),
                _ => continue,
            }
            let snapshot = fs::read(root.join("state.json")).unwrap();
            let bytes = fs::read(&log).unwrap();
            let error = Checkpoint::load(&root).err().unwrap();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "checkpoint_journal_invalid",
                "accepted {damage}"
            );
            assert_eq!(fs::read(root.join("state.json")).unwrap(), snapshot);
            assert_eq!(fs::read(&log).unwrap(), bytes);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn schema1_migrates_on_mutation_without_rewriting_artifacts() {
        let (root, mut checkpoint) = fixture();
        checkpoint
            .publish_bytes(b"retained exact evidence", "raw/000000.json")
            .unwrap();
        checkpoint.compact().unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        legacy["schema_version"] = serde_json::json!(1);
        legacy.as_object_mut().unwrap().remove("journal");
        fs::write(
            root.join("state.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        drop(checkpoint);
        let mut reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert!(reopened.journal.is_none());
        assert!(reopened.completed("raw/000000.json").unwrap());
        reopened.pending("raw/000001.json").unwrap();
        drop(reopened);
        let reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(reopened.state.schema_version, 4);
        assert_eq!(reopened.state.scratch.len(), 1);
        assert_eq!(
            fs::read(root.join("raw/000000.json")).unwrap(),
            b"retained exact evidence"
        );
        assert!(reopened.completed("raw/000000.json").unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chunk_updates_do_not_repeat_a_large_batch_index_list() {
        let (root, mut checkpoint) = fixture();
        checkpoint.begin_batch((0..100_000).collect()).unwrap();
        let snapshot = fs::read(root.join("state.json")).unwrap();
        let log = root
            .join(&checkpoint.state.journal.as_ref().unwrap().directory)
            .join("events");
        let before = fs::metadata(&log).unwrap().len();
        checkpoint
            .publish_bytes(b"exact raw evidence", "raw/000000.json")
            .unwrap();
        assert!(
            fs::metadata(&log).unwrap().len() - before < 4096,
            "chunk update repeated batch metadata"
        );
        assert_eq!(fs::read(root.join("state.json")).unwrap(), snapshot);
        drop(checkpoint);
        let reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(reopened.batch().unwrap().indices.len(), 100_000);
        assert!(reopened.completed("raw/000000.json").unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_head_publication_requires_reopen_and_preserves_foreign_targets() {
        let (root, mut checkpoint) = fixture();
        let directory = root.join(&checkpoint.state.journal.as_ref().unwrap().directory);
        let original = root.join("original-head");
        fs::rename(directory.join("head.json"), &original).unwrap();
        let foreign = root.join("foreign");
        fs::write(&foreign, b"preserve me").unwrap();
        std::os::unix::fs::symlink(&foreign, directory.join("head.json")).unwrap();
        assert!(checkpoint.pending("raw/000000.json").is_err());
        let length = fs::metadata(directory.join("events")).unwrap().len();
        assert!(length > 0);
        assert!(checkpoint.pending("raw/000001.json").is_err());
        assert!(checkpoint.recover().is_err());
        assert!(checkpoint.compact().is_err());
        assert_eq!(
            fs::metadata(directory.join("events")).unwrap().len(),
            length
        );
        assert_eq!(fs::read(&foreign).unwrap(), b"preserve me");
        fs::remove_file(directory.join("head.json")).unwrap();
        fs::rename(original, directory.join("head.json")).unwrap();
        drop(checkpoint);
        let mut reopened = Checkpoint::load(&root).unwrap().unwrap();
        assert!(reopened.state.scratch.is_empty());
        reopened.pending("raw/000002.json").unwrap();
        drop(reopened);
        assert_eq!(
            Checkpoint::load(&root)
                .unwrap()
                .unwrap()
                .state
                .scratch
                .len(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_process_fixture() {
        if let Some(root) = std::env::var_os("AGENT_YOUTUBE_JOURNAL_CRASH_FIXTURE") {
            let root = PathBuf::from(root);
            let mut checkpoint = Checkpoint::load(&root).unwrap().unwrap();
            checkpoint.pending("raw/000000.json").unwrap();
            fs::write(root.join("ready"), b"durable").unwrap();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }

    #[test]
    fn journal_survives_process_death_without_a_new_snapshot() {
        let (root, checkpoint) = fixture();
        let snapshot = fs::read(root.join("state.json")).unwrap();
        drop(checkpoint);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "checkpoint::tests::journal_process_fixture"])
            .env("AGENT_YOUTUBE_JOURNAL_CRASH_FIXTURE", &root)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !root.join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = root.join("ready").exists();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready, "child never committed its record");
        assert_eq!(fs::read(root.join("state.json")).unwrap(), snapshot);
        assert_eq!(
            Checkpoint::load(&root)
                .unwrap()
                .unwrap()
                .state
                .scratch
                .len(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inaccessible_checkpoint_is_not_reported_as_corrupt_or_automatically_retryable() {
        let error = evidence_context(
            "checkpoint_invalid",
            Path::new("state.json"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
        );
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "checkpoint_unavailable");
        assert_eq!(failure.retryable, None);
    }

    #[test]
    #[ignore = "local long-artifact hash measurement; run alone with --nocapture"]
    fn long_artifact_hash_benchmark() {
        let (root, mut checkpoint) = fixture();
        let pending = checkpoint.pending("media/audio.wav").unwrap();
        crate::audio::sparse_test_wav(&pending, 3600 * 16000);
        checkpoint.publish(&pending, "media/audio.wav").unwrap();
        for memo in [false, true] {
            #[cfg(unix)]
            checkpoint.verified.borrow_mut().clear();
            storage::HASH_BYTES.set(0);
            let started = std::time::Instant::now();
            for _ in 0..5 {
                assert!(
                    if memo {
                        checkpoint.verified("media/audio.wav")
                    } else {
                        checkpoint.completed("media/audio.wav")
                    }
                    .unwrap()
                );
            }
            for _ in 0..2 {
                assert!(checkpoint.completed("media/audio.wav").unwrap());
            }
            eprintln!(
                "hash_benchmark memo={memo} bytes={} elapsed_ms={} pcm_bytes={}",
                storage::HASH_BYTES.get(),
                started.elapsed().as_millis(),
                fs::metadata(root.join("media/audio.wav")).unwrap().len()
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scheduling_memo_rejects_replacement_links_and_writes_even_with_restored_mtime() {
        let (root, mut checkpoint) = fixture();
        checkpoint
            .publish_bytes(b"original", "raw/000000.json")
            .unwrap();
        let path = root.join("raw/000000.json");
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        let time = file.metadata().unwrap().modified().unwrap();
        assert!(checkpoint.verified("raw/000000.json").unwrap());
        storage::HASH_BYTES.set(0);
        assert!(checkpoint.verified("raw/000000.json").unwrap());
        assert_eq!(storage::HASH_BYTES.get(), 0);
        fs::write(&path, b"tampered").unwrap();
        file.set_modified(time).unwrap();
        assert!(checkpoint.verified("raw/000000.json").is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"original").unwrap();
        assert!(checkpoint.verified("raw/000000.json").unwrap());
        let target = root.join("owner");
        fs::rename(&path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(checkpoint.verified("raw/000000.json").is_err());
        assert_eq!(fs::read(target).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema2_migration_snapshots_new_intents_without_replaying_them_twice() {
        let (root, mut checkpoint) = fixture();
        checkpoint
            .publish_bytes(b"raw evidence", "raw/000000.json")
            .unwrap();
        let pending = checkpoint.pending("raw/000001.json").unwrap();
        fs::write(&pending, b"new evidence").unwrap();
        checkpoint.compact().unwrap();
        let path = root.join("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state["schema_version"] = serde_json::json!(2);
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        drop(checkpoint);
        let mut old = Checkpoint::load(&root).unwrap().unwrap();
        old.publish(&pending, "raw/000001.json").unwrap();
        old.publish_bytes(b"decoder state fixture", "chunks/000000.resume.json")
            .unwrap();
        drop(old);
        let current = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(current.state.schema_version, 4);
        assert!(current.completed("raw/000000.json").unwrap());
        assert!(current.completed("chunks/000000.resume.json").unwrap());
        assert!(current.state.publications.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memory_refused_snapshot_poisoning_preserves_committed_journal() {
        let (root, mut checkpoint) = fixture();
        checkpoint.pending("raw/000000.json").unwrap();
        let error = crate::memory::with_capacity_for_test(0, || checkpoint.compact()).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "resource_limit"
        );
        assert!(checkpoint.recover().is_err());
        assert!(checkpoint.pending("raw/000001.json").is_err());
        drop(checkpoint);
        let resumed = Checkpoint::load(&root).unwrap().unwrap();
        assert_eq!(resumed.state.scratch.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    pub(crate) fn fixture() -> (PathBuf, Checkpoint) {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-checkpoint-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in ["media", "chunks", "raw"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let config = Config {
            range: None,
            url: "https://www.youtube.com/watch?v=a2hbJlPxj4M".into(),
            model_dir: root.join("models"),
            model_variant: Default::default(),
            model_fingerprint: BTreeMap::new(),
            gigastt_version: "fixture".into(),
            gigastt_binary: "fixture".into(),
            chunk_seconds: 10,
            boundary_window_ms: 0,
            encoder_threads: None,
            media_backend: "fixture".into(),
            engine_environment: BTreeMap::new(),
        };
        let checkpoint = Checkpoint::open(&root, config).unwrap();
        (root, checkpoint)
    }

    #[test]
    fn grouped_publication_recovers_all_intents_after_partial_linking() {
        let (root, mut checkpoint) = fixture();
        let names = ["raw/000000.json", "raw/000001.json", "raw/000002.json"];
        let pending = checkpoint.pending_many(&names).unwrap();
        for path in &pending {
            fs::write(path, b"exact evidence").unwrap();
        }
        let files: Vec<_> = pending
            .into_iter()
            .zip(names.iter().map(|s| s.to_string()))
            .collect();
        fs::write(root.join(names[1]), b"owner conflict").unwrap();
        assert!(checkpoint.publish_many(&files).is_err());
        assert_eq!(fs::read(root.join(names[1])).unwrap(), b"owner conflict");
        assert!(root.join(names[0]).exists());
        let state: State =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        assert_eq!(
            Checkpoint::load(&root)
                .unwrap()
                .unwrap()
                .state
                .publications
                .len(),
            3
        );
        drop(checkpoint);
        let mut resumed = Checkpoint::open(&root, state.config).unwrap();
        assert!(resumed.recover().is_err());
        assert_eq!(fs::read(root.join(names[1])).unwrap(), b"owner conflict");
        fs::remove_file(root.join(names[1])).unwrap();
        resumed.recover().unwrap();
        for name in names {
            assert!(resumed.completed(name).unwrap());
        }
        assert!(resumed.state.publications.is_empty());
        assert!(resumed.state.scratch.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "opt-in checkpoint write amplification benchmark; run alone with --nocapture"]
    fn checkpoint_publication_benchmark() {
        for count in [128, 512, 2048, 8192] {
            let root = std::env::temp_dir().join(format!(
                "agent-youtube-checkpoint-bench-{}-{count}",
                std::process::id()
            ));
            for dir in ["media", "chunks", "raw"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            let config = Config {
                range: None,
                url: "https://www.youtube.com/watch?v=a2hbJlPxj4M".into(),
                model_dir: root.join("models"),
                model_variant: Default::default(),
                model_fingerprint: BTreeMap::new(),
                gigastt_version: "fixture".into(),
                gigastt_binary: "fixture".into(),
                chunk_seconds: 10,
                boundary_window_ms: 0,
                encoder_threads: None,
                media_backend: "fixture".into(),
                engine_environment: BTreeMap::new(),
            };
            let mut checkpoint = Checkpoint::open(&root, config).unwrap();
            SAVED_BYTES.store(0, Ordering::Relaxed);
            let started = std::time::Instant::now();
            let group_size = std::env::var("AGENT_YOUTUBE_CHECKPOINT_GROUP")
                .unwrap_or_else(|_| "32".into())
                .parse::<usize>()
                .unwrap();
            let indices: Vec<_> = (0..count).collect();
            for group in indices.chunks(group_size) {
                let names: Vec<_> = group
                    .iter()
                    .map(|index| format!("raw/{index:06}.json"))
                    .collect();
                let refs: Vec<_> = names.iter().map(String::as_str).collect();
                let pending = checkpoint.pending_many(&refs).unwrap();
                for path in &pending {
                    fs::write(path, b"exact ASR fixture").unwrap();
                }
                let files: Vec<_> = pending.into_iter().zip(names).collect();
                checkpoint.publish_many(&files).unwrap();
            }
            // Finish with an up-to-date snapshot, as the baseline writer did.
            checkpoint.compact().unwrap();
            println!(
                "{}",
                serde_json::json!({"artifacts":count,"group_size":group_size,"state_bytes":SAVED_BYTES.load(Ordering::Relaxed),"elapsed_ms":started.elapsed().as_millis()})
            );
            assert_eq!(checkpoint.state.artifacts.len(), count);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn checkpoint_json_can_grow_past_the_former_16_mib_ceiling() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for dir in ["media", "chunks", "raw"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let config = Config {
            range: None,
            url: "https://youtu.be/a2hbJlPxj4M".into(),
            model_dir: root.join("models"),
            model_variant: Default::default(),
            model_fingerprint: BTreeMap::new(),
            gigastt_version: "fixture".into(),
            gigastt_binary: "fixture".into(),
            chunk_seconds: 10,
            boundary_window_ms: 0,
            encoder_threads: None,
            media_backend: "fixture".into(),
            engine_environment: BTreeMap::new(),
        };
        Checkpoint::open(&root, config).unwrap();
        let path = root.join("state.json");
        let state: State = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        for _ in 0..257 {
            file.write_all(&[b' '; 65536]).unwrap();
        }
        drop(file);
        assert!(Checkpoint::open(&root, state.config).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chunk_paths_extend_past_six_digits_without_weakening_namespace_validation() {
        assert!(
            validate_publication(
                "raw/1000000.json",
                "raw/.agent-youtube-1-2-3-1000000.partial.json"
            )
            .is_ok()
        );
        assert!(media_artifact("chunks/1000000.wav"));
        for name in [
            "raw/../outside.json",
            "raw/1000000/evil.json",
            "raw/notanindex.json",
            "raw/99999999999999999999999999999999.json",
        ] {
            assert!(
                validate_publication(name, "raw/.agent-youtube-1-2-3-1000000.partial.json")
                    .is_err()
            );
        }
    }
}
