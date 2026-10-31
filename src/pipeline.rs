mod model;
pub use model::ModelVariant;
mod publication;
mod recognition;
use crate::{
    Result, audio,
    checkpoint::{Checkpoint, Config},
    download::Downloader,
    process::Runner,
    storage,
    transcript::{self, Engine, GigasttResult, Source, Transcript},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub use crate::storage::read_text;
const JSON_LIMIT: u64 = 16 * 1024 * 1024;
const PUBLICATION_BATCH: usize = 32;
pub(crate) const MODEL_FILES: [&str; 4] = [
    "v3_e2e_rnnt_encoder_int8.onnx",
    "v3_e2e_rnnt_decoder.onnx",
    "v3_e2e_rnnt_joint.onnx",
    "v3_e2e_rnnt_vocab.txt",
];
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub url: String,
    pub work_dir: PathBuf,
    pub model_dir: PathBuf,
    #[serde(default)]
    pub model_variant: ModelVariant,
    pub gigastt: PathBuf,
    pub chunk_seconds: u32,
    pub boundary_window_ms: u32,
    pub asr_timeout_seconds: u64,
    pub asr_memory_mib: Option<u64>,
    pub encoder_threads: Option<usize>,
    #[serde(default)]
    pub retry_window_seconds: u64,
}

pub struct Outcome {
    pub transcript: PathBuf,
    pub manifest: Option<PathBuf>,
    pub reused: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DecodedChunks {
    schema_version: u32,
    frames: u64,
    chunk_seconds: u32,
}
struct DecodedAudio {
    frames: u64,
    legacy: Option<audio::Wav>,
}

#[derive(Default)]
pub(crate) struct RecognitionGate {
    lock: std::sync::Mutex<()>,
    aborted: AtomicBool,
}

pub struct Pipeline {
    options: Options,
    runner: Runner,
    sequence: AtomicU64,
    downloader: Downloader,
    recognition_gate: Option<Arc<RecognitionGate>>,
}

pub fn video_id(url: &str) -> Result<String> {
    // Accept a deliberately small URL grammar; normalize to one HTTPS origin.
    // Reject search expressions, local files and arbitrary origins at the CLI boundary.
    let input = url
        .strip_prefix("https://")
        .ok_or("expected an HTTPS YouTube URL")?;
    let (host, tail) = input
        .split_once('/')
        .ok_or("YouTube URL is missing a video id")?;
    let id = match host {
        "youtu.be" => tail.split(['?', '#']).next().unwrap_or_default(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" => {
            if let Some(query) = tail.strip_prefix("watch?") {
                query
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("v="))
                    .ok_or("missing v= video id")?
            } else if let Some(path) = tail
                .strip_prefix("shorts/")
                .or_else(|| tail.strip_prefix("live/"))
            {
                path.split(['?', '#']).next().unwrap_or_default()
            } else {
                return Err("expected /watch?v=, /shorts/ or /live/ YouTube URL".into());
            }
        }
        _ => return Err("only youtube.com and youtu.be video URLs are accepted".into()),
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("YouTube video id must contain exactly 11 letters, digits, _ or -".into());
    }
    Ok(id.into())
}

fn strings(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

impl Pipeline {
    pub fn new(options: Options, cancelled: Arc<AtomicBool>) -> Result<Self> {
        Self::with_downloader(options, cancelled, Downloader::youtube()?)
    }
    pub fn with_downloader(
        mut options: Options,
        cancelled: Arc<AtomicBool>,
        downloader: Downloader,
    ) -> Result<Self> {
        video_id(&options.url)?;
        if options.asr_timeout_seconds == 0 || options.encoder_threads == Some(0) {
            return Err("ASR timeout and encoder threads must be positive".into());
        }
        Runner::new(cancelled.clone()).with_memory_budget(
            options
                .asr_memory_mib
                .map(|mib| {
                    mib.checked_mul(1024 * 1024)
                        .ok_or("ASR memory budget overflow")
                })
                .transpose()?,
        )?;
        if options.boundary_window_ms > 2000 {
            return Err("--boundary-window-ms must be between 0 and 2000".into());
        }
        if !(10..=600).contains(&options.chunk_seconds) {
            return Err("--chunk-seconds must be between 10 and 600".into());
        }
        if fs::symlink_metadata(&options.work_dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("work directory must not be a symlink".into());
        }
        storage::private_directory(&options.work_dir, true)?;
        options.work_dir = fs::canonicalize(&options.work_dir)?;
        for subdir in ["logs", "media", "chunks", "raw"] {
            let path = options.work_dir.join(subdir);
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(format!("refusing symlink directory {}", path.display()).into());
            }
            storage::private_directory(&path, true)?;
        }
        let timeout = options.asr_timeout_seconds;
        Ok(Self {
            options,
            runner: Runner::new(cancelled).with_timeout(std::time::Duration::from_secs(timeout)),
            sequence: AtomicU64::new(0),
            downloader,
            recognition_gate: None,
        })
    }
    pub(crate) fn with_recognition_gate(mut self, gate: Arc<RecognitionGate>) -> Self {
        self.recognition_gate = Some(gate);
        self
    }
    fn recognition_slot(&self) -> Result<Option<std::sync::MutexGuard<'_, ()>>> {
        let Some(gate) = &self.recognition_gate else {
            return Ok(None);
        };
        loop {
            if self.runner.cancelled.load(Ordering::SeqCst) {
                return Err("interrupted while waiting for recognition".into());
            }
            if gate.aborted.load(Ordering::SeqCst) {
                return Err(crate::protocol::failure(
                    "process_teardown_failed",
                    None,
                    "recognition stopped after unverified worker teardown",
                ));
            }
            match gate.lock.try_lock() {
                Ok(guard) => {
                    if gate.aborted.load(Ordering::SeqCst) {
                        return Err(crate::protocol::failure(
                            "process_teardown_failed",
                            None,
                            "recognition stopped after unverified worker teardown",
                        ));
                    }
                    return Ok(Some(guard));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err("recognition gate is poisoned".into());
                }
            }
        }
    }
    fn command(
        &self,
        runner: &Runner,
        program: &Path,
        args: &[OsString],
        label: &str,
    ) -> Result<PathBuf> {
        self.command_with_progress(runner, program, args, label, || Ok(false))
    }
    fn command_with_progress(
        &self,
        runner: &Runner,
        program: &Path,
        args: &[OsString],
        label: &str,
        progress: impl FnMut() -> Result<bool>,
    ) -> Result<PathBuf> {
        if runner.cancelled.load(Ordering::SeqCst) {
            return Err("interrupted; run the same command to resume".into());
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let stem = format!("{}-{stamp}-{sequence}-{label}", std::process::id());
        let stdout = self
            .options
            .work_dir
            .join("logs")
            .join(format!("{stem}.out"));
        let stderr = self
            .options
            .work_dir
            .join("logs")
            .join(format!("{stem}.err"));
        let runner = if label == "recognize" {
            runner.with_memory_budget(
                self.options
                    .asr_memory_mib
                    .map(|mib| {
                        mib.checked_mul(1024 * 1024)
                            .ok_or("ASR memory budget overflow")
                    })
                    .transpose()?,
            )?
        } else {
            runner.with_timeout(std::time::Duration::from_secs(30))
        };
        runner.run_with_progress(program, args, &stdout, &stderr, progress)?;
        Ok(stdout)
    }

    fn capture(
        &self,
        runner: &Runner,
        program: &Path,
        args: &[OsString],
        label: &str,
    ) -> Result<String> {
        let path = self.command(runner, program, args, label)?;
        read_text(&path, JSON_LIMIT)
    }

    pub fn doctor(&self) -> Result<String> {
        self.doctor_with(&self.runner)
    }
    fn doctor_with(&self, runner: &Runner) -> Result<String> {
        for (name, _) in std::env::vars_os() {
            if name
                .to_str()
                .is_some_and(|n| n.starts_with("GIGASTT_") || n.starts_with("ORT_"))
            {
                return Err(crate::protocol::failure(
                    "ambient_asr_setting",
                    Some(false),
                    format!(
                        "unsupported ambient ASR setting {}; unset it and use explicit agent-youtube options",
                        name.to_string_lossy()
                    ),
                ));
            }
        }
        let output = self.capture(
            runner,
            &self.options.gigastt,
            &strings(&["--version"]),
            "version",
        )?;
        let gigastt_version = output
            .lines()
            .next()
            .ok_or("empty tool version")?
            .to_owned();
        crate::protocol::progress(
            "preflight",
            format!("dependency: {gigastt_version}"),
            serde_json::json!({"gigastt_version":gigastt_version}),
        );
        let help = self.capture(
            runner,
            &self.options.gigastt,
            &strings(&["transcribe-batch", "--help"]),
            "gigastt-help",
        )?;
        for flag in [
            "--format",
            "--pool-size",
            "--retries",
            "--offline",
            "--model-variant",
            "--punctuation",
            "--itn",
            "--log-level",
            "--model-dir",
        ]
        .into_iter()
        .chain(
            self.options
                .encoder_threads
                .map(|_| "--encoder-intra-threads"),
        ) {
            if !help
                .split(|c: char| {
                    c.is_whitespace() || matches!(c, ',' | '=' | '[' | ']' | '(' | ')')
                })
                .any(|token| token == flag)
            {
                return Err(crate::protocol::failure_details(
                    "asr_capability_missing",
                    Some(false),
                    format!(
                        "incompatible gigastt ({gigastt_version}): missing {flag}; install gigastt 2.22.0"
                    ),
                    serde_json::json!({"flag":flag,"gigastt_version":gigastt_version}),
                ));
            }
        }
        self.model_paths()?;
        crate::protocol::progress(
            "preflight",
            format!(
                "models: {} required files available",
                self.options.model_variant.as_str()
            ),
            serde_json::json!({}),
        );
        Ok(gigastt_version)
    }
    fn model_paths(&self) -> Result<Vec<(&'static str, PathBuf)>> {
        self.options
            .model_variant
            .files()
            .iter()
            .copied()
            .map(|name| {
                let path = self.options.model_dir.join(name);
                if !path.is_file() || fs::metadata(&path)?.len() == 0 {
                    return Err(format!(
                        "missing selected model file {}; see README model installation",
                        path.display()
                    )
                    .into());
                }
                fs::File::open(&path)?;
                Ok((name, path))
            })
            .collect()
    }
    fn media_backend(&self) -> String {
        if self.options.boundary_window_ms == 0 {
            audio::BACKEND.into()
        } else {
            format!("{}-quiet-boundaries-v1", audio::BACKEND)
        }
    }
    fn config(&self, url: String, version: String) -> Result<Config> {
        let model_fingerprint = self
            .model_paths()?
            .into_iter()
            .map(|(name, path)| Ok((name.to_owned(), storage::digest(&path)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let engine_environment = std::env::vars_os()
            .filter_map(|(name, value)| {
                let name = name.to_str()?;
                ["GIGASTT_", "ORT_"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                    .then(|| {
                        (
                            name.to_owned(),
                            format!("{:x}", Sha256::digest(value.as_encoded_bytes())),
                        )
                    })
            })
            .collect();
        Ok(Config {
            url,
            model_dir: fs::canonicalize(&self.options.model_dir)?,
            model_fingerprint,
            model_variant: self.options.model_variant,
            gigastt_version: version,
            gigastt_binary: storage::digest(&resolve_executable(&self.options.gigastt)?)?,
            chunk_seconds: self.options.chunk_seconds,
            boundary_window_ms: self.options.boundary_window_ms,
            engine_environment,
            media_backend: self.media_backend(),
            encoder_threads: self.options.encoder_threads,
        })
    }
    fn metadata(
        &self,
        runner: &Runner,
        checkpoint: &mut Checkpoint,
        id: &str,
    ) -> Result<serde_json::Value> {
        if !checkpoint.completed("source.json")? {
            crate::protocol::progress(
                "metadata",
                "[1/4] reading YouTube metadata",
                serde_json::json!({}),
            );
            let raw = self.downloader.metadata(id, &runner.cancelled)?;
            checkpoint.publish_bytes(&raw, "source.json")?;
        }
        let metadata: serde_json::Value = serde_json::from_str(&read_text(
            &self.options.work_dir.join("source.json"),
            JSON_LIMIT,
        )?)?;
        if metadata["videoDetails"]["videoId"]
            .as_str()
            .or_else(|| metadata["id"].as_str())
            != Some(id)
        {
            return Err("downloader returned an unexpected video id".into());
        }
        Ok(metadata)
    }
    fn download(&self, runner: &Runner, checkpoint: &mut Checkpoint, url: &str) -> Result<PathBuf> {
        for name in ["media/source.audio", "media/video.mkv"] {
            if checkpoint.completed(name)? {
                crate::protocol::progress(
                    "download",
                    "[1/4] verified saved source; skipping download",
                    serde_json::json!({}),
                );
                return Ok(self.options.work_dir.join(name));
            }
        }
        crate::protocol::progress(
            "download",
            "[1/4] downloading audio only",
            serde_json::json!({}),
        );
        let id = video_id(url)?;
        let (prefix, downloaded) = loop {
            let prefix = checkpoint.begin_download()?;
            validate_download_scratch(&prefix)?;
            match self.downloader.download(&id, &prefix, &runner.cancelled) {
                Ok(path) => break (prefix, path),
                Err(error)
                    if error
                        .downcast_ref::<crate::protocol::Failure>()
                        .is_some_and(|failure| {
                            failure.code == "download_representation_changed"
                        }) =>
                {
                    if runner.cancelled.load(Ordering::SeqCst) {
                        return Err(crate::protocol::failure(
                            "interrupted",
                            Some(true),
                            "download interrupted; media preserved",
                        ));
                    }
                    checkpoint.retire_download()?;
                    crate::protocol::progress(
                        "download",
                        "starting a separate audio acquisition; previous media preserved",
                        serde_json::json!({"reason":"download_representation_changed"}),
                    );
                }
                Err(error) => return Err(error),
            }
        };
        if let Some(original) = checkpoint.original_acquisition() {
            Downloader::same_track(&original, &prefix)?;
        }
        if !checkpoint.completed("download.json")? {
            checkpoint.publish_bytes(
                &Downloader::evidence(&prefix, &downloaded)?,
                "download.json",
            )?;
        }
        checkpoint.publish(&downloaded, "media/source.audio")?;
        Ok(self.options.work_dir.join("media/source.audio"))
    }
    fn extract_audio(
        &self,
        runner: &Runner,
        checkpoint: &mut Checkpoint,
        source: &Path,
        duration_seconds: Option<u64>,
    ) -> Result<DecodedAudio> {
        self.extract_audio_with_space(runner, checkpoint, source, duration_seconds, |path| {
            storage::disk_space_at(path, "decode_audio")
        })
    }
    fn extract_audio_with_space(
        &self,
        runner: &Runner,
        checkpoint: &mut Checkpoint,
        source: &Path,
        duration_seconds: Option<u64>,
        mut capacity: impl FnMut(&Path) -> Result<storage::DiskSpace>,
    ) -> Result<DecodedAudio> {
        // Existing unfinished runs keep their validated complete-WAV path.
        if checkpoint.verified("media/audio.wav")? {
            let wav = audio::Wav::open(&self.options.work_dir.join("media/audio.wav"))?;
            return Ok(DecodedAudio {
                frames: wav.frames,
                legacy: Some(wav),
            });
        }
        if checkpoint.verified("media/audio.json")? {
            let path = self.options.work_dir.join("media/audio.json");
            let invalid = |error: Box<dyn std::error::Error + Send + Sync>| {
                crate::checkpoint::evidence_context("decoded_receipt_invalid", &path, error)
            };
            let receipt: DecodedChunks =
                serde_json::from_str(&read_text(&path, 1024).map_err(invalid)?)
                    .map_err(|error| invalid(error.into()))?;
            if receipt.schema_version != 1
                || receipt.frames == 0
                || receipt.chunk_seconds != self.options.chunk_seconds
            {
                return Err(crate::checkpoint::evidence_failure(
                    "decoded_receipt_invalid",
                    &path,
                    "invalid decoded chunk receipt",
                ));
            }
            return Ok(DecodedAudio {
                frames: receipt.frames,
                legacy: None,
            });
        }
        let chunk_frames = u64::from(self.options.chunk_seconds) * u64::from(audio::SAMPLE_RATE);
        let directory = self.options.work_dir.join("chunks");
        let header = ryf::encode_s16(&[], audio::SAMPLE_RATE)?.len() as u64;
        let saved = checkpoint.latest_decode_resume();
        let mut resume = None;
        if let Some((index, name)) = &saved {
            checkpoint.completed(name)?;
            let path = self.options.work_dir.join(name);
            let invalid = |error: Box<dyn std::error::Error + Send + Sync>| {
                crate::checkpoint::evidence_context("decode_resume_invalid", &path, error)
            };
            crate::memory::require(
                fs::metadata(&path)?.len().saturating_mul(16),
                &path,
                "decode_resume_load",
            )?;
            let state: audio::Resume =
                serde_json::from_str(&read_text(&path, 1 << 20).map_err(invalid)?)
                    .map_err(|error| invalid(error.into()))?;
            if index.checked_add(1) != Some(state.completed_chunks()) {
                return Err(invalid("decoder state index mismatch".into()));
            }
            for index in 0..state.completed_chunks() {
                let name = format!("chunks/{index:06}.wav");
                if !checkpoint.completed(&name)? {
                    return Err(crate::checkpoint::evidence_failure(
                        "decoded_chunk_missing",
                        &self.options.work_dir.join(&name),
                        "decoder state requires a validated prefix chunk; media preserved",
                    ));
                }
            }
            resume = Some(state);
        }
        if let Some(seconds) = duration_seconds.filter(|s| *s > 0) {
            let frames = seconds
                .checked_mul(u64::from(audio::SAMPLE_RATE))
                .ok_or("audio duration estimate overflow")?;
            let count = frames.div_ceil(chunk_frames);
            let space = capacity(&directory)?;
            let allocation = |samples: u64| -> Result<u64> {
                space.allocation(
                    samples
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(header))
                        .ok_or("PCM estimate overflow")?,
                )
            };
            let last = frames - (count - 1) * chunk_frames;
            let last_allocation = allocation(last)?;
            let mut required = allocation(chunk_frames)?
                .checked_mul(count - 1)
                .and_then(|n| n.checked_add(last_allocation))
                .ok_or("PCM allocation estimate overflow")?;
            // Work scales with saved chunks, never with an untrusted duration field.
            for index in checkpoint.chunk_indices() {
                if runner.cancelled.load(Ordering::SeqCst) {
                    return Err("interrupted".into());
                }
                if (index as u64) < count
                    && checkpoint.verified(&format!("chunks/{index:06}.wav"))?
                {
                    required = required
                        .checked_sub(allocation(if index as u64 == count - 1 {
                            last
                        } else {
                            chunk_frames
                        })?)
                        .ok_or("saved PCM allocation estimate overflow")?;
                }
            }
            space.require_at(&directory, required, "decode_audio")?;
        }
        crate::protocol::progress(
            "decode",
            "[2/4] decoding AAC/M4A directly into resumable PCM chunks",
            serde_json::json!({}),
        );
        let frames =
            audio::decode_resumable(source, chunk_frames, &runner.cancelled, resume, |event| {
                match event {
                    audio::DecodeEvent::Checkpoint(index, state) => {
                        let name = format!("chunks/{index:06}.resume.json");
                        if !checkpoint.verified(&name)? {
                            checkpoint.publish_bytes(&serde_json::to_vec(state)?, &name)?;
                        }
                        Ok(())
                    }
                    audio::DecodeEvent::Chunk(index, pcm) => {
                        let name = format!("chunks/{index:06}.wav");
                        if checkpoint.verified(&name)? {
                            if audio::Wav::open(&self.options.work_dir.join(&name))?.frames
                                != pcm.len() as u64 / 2
                            {
                                return Err(crate::checkpoint::evidence_failure(
                                    "decoded_chunk_invalid",
                                    &self.options.work_dir.join(&name),
                                    "saved chunk length does not match decoded audio",
                                ));
                            }
                            return Ok(());
                        }
                        let space = capacity(&directory)?;
                        space.require_at(
                            &directory,
                            space.allocation(
                                (pcm.len() as u64)
                                    .checked_add(header)
                                    .ok_or("chunk estimate overflow")?,
                            )?,
                            "decode_audio",
                        )?;
                        let pending = checkpoint.pending(&name)?;
                        audio::write_chunk(&pending, pcm).map_err(|error| {
                            storage::audio_write_error(error, &directory, "decode_audio")
                        })?;
                        checkpoint.publish(&pending, &name)?;
                        crate::protocol::progress(
                            "decode",
                            "published decoded audio chunk",
                            serde_json::json!({"completed_chunks":index+1,"durable":true,"scope":"recording"}),
                        );
                        Ok(())
                    }
                }
            })
            .map_err(|error| {
                if error
                    .downcast_ref::<crate::protocol::Failure>()
                    .is_some_and(|e| e.code == "decode_resume_invalid")
                {
                    let path = saved
                        .as_ref()
                        .map(|(_, name)| self.options.work_dir.join(name))
                        .unwrap_or_else(|| source.to_owned());
                    crate::checkpoint::evidence_failure("decode_resume_invalid", &path, error)
                } else {
                    error
                }
            })?;
        if frames == 0 {
            return Err("source contains no decoded audio".into());
        }
        checkpoint.publish_bytes(
            &serde_json::to_vec(&DecodedChunks {
                schema_version: 1,
                frames,
                chunk_seconds: self.options.chunk_seconds,
            })?,
            "media/audio.json",
        )?;
        Ok(DecodedAudio {
            frames,
            legacy: None,
        })
    }
    fn prepare_chunks(
        &self,
        checkpoint: &mut Checkpoint,
        audio: &mut audio::Wav,
        indices: &[usize],
    ) -> Result<()> {
        self.prepare_chunks_with_space(checkpoint, audio, indices, storage::disk_space)
    }
    fn prepare_chunks_with_space(
        &self,
        checkpoint: &mut Checkpoint,
        audio: &mut audio::Wav,
        indices: &[usize],
        capacity: impl FnOnce(&Path) -> Result<storage::DiskSpace>,
    ) -> Result<()> {
        let chunk_frames = u64::from(self.options.chunk_seconds) * u64::from(audio::SAMPLE_RATE);
        let mut missing = Vec::new();
        for index in indices {
            if self.runner.cancelled.load(Ordering::SeqCst) {
                return Err("interrupted".into());
            }
            if !checkpoint.verified(&format!("chunks/{index:06}.wav"))? {
                missing.push(*index);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let directory = self.options.work_dir.join("chunks");
        let space = capacity(&directory)?;
        // Bounded pipeline chunks fit RIFF. Ask the same encoder for its header
        // rather than duplicating the codec's container layout.
        let header_bytes = ryf::encode_s16(&[], audio::SAMPLE_RATE)?.len() as u64;
        let mut required_bytes = 0_u64;
        for index in &missing {
            let start = (*index as u64)
                .checked_mul(chunk_frames)
                .ok_or("chunk offset overflow")?;
            let frames = audio
                .frames
                .checked_sub(start)
                .filter(|frames| *frames > 0)
                .ok_or("chunk outside audio")?
                .min(chunk_frames);
            let bytes = frames
                .checked_mul(2)
                .and_then(|bytes| bytes.checked_add(header_bytes))
                .ok_or("chunk size estimate overflow")?;
            required_bytes = required_bytes
                .checked_add(space.allocation(bytes)?)
                .ok_or("total chunk allocation estimate overflow")?;
        }
        space.require(&directory, required_bytes)?;
        for group in missing.chunks(PUBLICATION_BATCH) {
            let mut chunks = Vec::new();
            for index in group {
                if self.runner.cancelled.load(Ordering::SeqCst) {
                    return Err("interrupted".into());
                }
                let name = format!("chunks/{index:06}.wav");
                let start = (*index as u64)
                    .checked_mul(chunk_frames)
                    .ok_or("chunk offset overflow")?;
                let frames = audio
                    .frames
                    .checked_sub(start)
                    .ok_or("chunk outside audio")?
                    .min(chunk_frames);
                chunks.push((name, start, frames));
            }
            let names: Vec<_> = chunks.iter().map(|(name, _, _)| name.as_str()).collect();
            let paths = checkpoint.pending_many(&names)?;
            let mut files = Vec::new();
            for (pending, (name, start, frames)) in paths.into_iter().zip(chunks) {
                audio.chunk(&pending, start, frames, &self.runner.cancelled)?;
                files.push((pending, name));
            }
            checkpoint.publish_many(&files)?;
        }
        Ok(())
    }
    pub fn run(&self) -> Result<PathBuf> {
        Ok(self.run_report()?.transcript)
    }
    pub fn run_report(&self) -> Result<Outcome> {
        self.run_report_inner()
            .map_err(|e| storage::audio_write_error(e, &self.options.work_dir, "pipeline"))
    }
    fn run_report_inner(&self) -> Result<Outcome> {
        let control_lock = storage::Lock::control(&self.options.work_dir)?;
        let mut run_lock = storage::Lock::acquire(&self.options.work_dir)?;
        let id = video_id(&self.options.url)?;
        let url = format!("https://www.youtube.com/watch?v={id}");
        Checkpoint::validate_model(&self.options.work_dir, self.options.model_variant)?;
        let finished = Checkpoint::finished(&self.options.work_dir, &url)?;
        if finished.is_none() {
            Checkpoint::validate_backend(&self.options.work_dir, &self.media_backend())?;
        }
        let mut retry = crate::job::Retry::open(&self.options)?;
        let active = crate::job::Active::start(
            &self.options,
            self.runner.cancelled.clone(),
            finished.is_none(),
        )?;
        drop(control_lock);
        if let Some(checkpoint) = finished {
            let outcome = self.recover_published(checkpoint)?;
            retry.finish(true)?;
            return Ok(outcome);
        }
        let result = loop {
            retry.wait(&self.runner.cancelled)?;
            let result = self.run_locked(&run_lock);
            if result
                .as_ref()
                .err()
                .and_then(|e| e.downcast_ref::<crate::protocol::Failure>())
                .is_some_and(|e| {
                    matches!(
                        e.code,
                        "process_teardown_failed" | "web_runtime_teardown_failed"
                    )
                })
            {
                break result;
            }
            match result {
                Ok(outcome) => {
                    retry.finish(true)?;
                    break Ok(outcome);
                }
                Err(error) => {
                    if active.expired() {
                        break Err(retry.deadline_error());
                    }
                    if self.runner.cancelled.load(Ordering::SeqCst) {
                        break Err(error);
                    }
                    if !retry.schedule(error.as_ref())? {
                        retry.finish(false)?;
                        break Err(error);
                    }
                }
            }
        };
        if result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<crate::protocol::Failure>())
            .is_some_and(|e| e.code == "process_teardown_failed")
        {
            run_lock.leave_locked_for_child();
        }
        result
    }
    fn run_locked(&self, run_lock: &storage::Lock) -> Result<Outcome> {
        let runner = self.runner.with_lock(run_lock.child_handle()?);
        let id = video_id(&self.options.url)?;
        let url = format!("https://www.youtube.com/watch?v={id}");
        let output = self.options.work_dir.join("transcript.json");
        if let Some(checkpoint) = Checkpoint::finished(&self.options.work_dir, &url)? {
            return self.recover_published(checkpoint);
        }
        let version = self.doctor_with(&runner)?;
        let mut checkpoint = Checkpoint::open(
            &self.options.work_dir,
            self.config(url.clone(), version.clone())?,
        )?;
        crate::job::save(&self.options)?;
        checkpoint.recover()?;
        let metadata = self.metadata(&runner, &mut checkpoint, &id)?;
        let source = self.download(&runner, &mut checkpoint, &url)?;
        let declared_duration = checkpoint.source_duration()?;
        let mut audio =
            self.extract_audio(&runner, &mut checkpoint, &source, Some(declared_duration))?;
        checkpoint.validate_duration(audio.frames as f64 / f64::from(audio::SAMPLE_RATE))?;
        let rate = u64::from(audio::SAMPLE_RATE);
        let chunk_frames = u64::from(self.options.chunk_seconds) * rate;
        let last_start = (audio.frames - 1) / chunk_frames * chunk_frames;
        let header = Transcript {
            schema_version: 1,
            source: Source {
                url,
                video_id: id,
                title: metadata["videoDetails"]["title"]
                    .as_str()
                    .or_else(|| metadata["title"].as_str())
                    .map(str::to_owned),
            },
            language: self.options.model_variant.recognizer_language().into(),
            duration: last_start as f64 / rate as f64
                + (audio.frames - last_start) as f64 / rate as f64,
            engine: Engine {
                name: "gigastt".into(),
                version,
                model_variant: self.options.model_variant.as_str().into(),
            },
            segments: Vec::new(),
        };
        {
            let _slot = self.recognition_slot()?;
            let result = self.recognize(&runner, &mut checkpoint, &mut audio, &header);
            if result
                .as_ref()
                .err()
                .and_then(|e| e.downcast_ref::<crate::protocol::Failure>())
                .is_some_and(|e| e.code == "process_teardown_failed")
                && let Some(gate) = &self.recognition_gate
            {
                gate.aborted.store(true, Ordering::SeqCst);
                self.runner.cancelled.store(true, Ordering::SeqCst);
            }
            result?;
        }
        drop(audio);
        let manifest = checkpoint.ensure_manifest(&runner.cancelled)?;
        checkpoint.cleanup_media().map_err(|e| crate::protocol::failure_details("cleanup_pending", Some(true), e, serde_json::json!({"transcript":output,"manifest":manifest,"transcript_published":true,"cleanup_complete":false})))?;
        crate::protocol::progress(
            "complete",
            "temporary media removed; transcript and raw JSON retained",
            serde_json::json!({}),
        );
        Ok(Outcome {
            transcript: output,
            manifest,
            reused: false,
        })
    }
}

fn resolve_executable(program: &Path) -> Result<PathBuf> {
    if program.components().count() > 1 {
        return Ok(fs::canonicalize(program)?);
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join(program);
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&candidate).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let executable = true;
        if candidate.is_file() && executable {
            return Ok(fs::canonicalize(candidate)?);
        }
    }
    Err(format!("cannot find executable {} on PATH", program.display()).into())
}

fn validate_download_scratch(prefix: &Path) -> Result<()> {
    let media = prefix.parent().ok_or("invalid download namespace")?;
    let basename = prefix
        .file_name()
        .ok_or("invalid download namespace")?
        .to_string_lossy();
    for entry in fs::read_dir(media)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{basename}."))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        let mut unsafe_entry = !metadata.is_file() || metadata.file_type().is_symlink();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            unsafe_entry |= metadata.nlink() != 1;
        }
        if unsafe_entry {
            return Err(format!(
                "unsafe download scratch {}; remove the link explicitly before resuming",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}
#[cfg(test)]
#[path = "pipeline/evaluation.rs"]
mod evaluation;
#[cfg(test)]
mod tests;
