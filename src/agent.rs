//! Derived agent views never replace exact canonical or upstream evidence.
use crate::{
    Result, storage,
    transcript::{self, Segment},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufReader, Seek, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn build_identity() -> Value {
    let dirty = match env!("AGENT_YOUTUBE_SOURCE_DIRTY") {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    json!({"version":env!("CARGO_PKG_VERSION"),"revision":env!("AGENT_YOUTUBE_REVISION"),"source_fingerprint":env!("AGENT_YOUTUBE_SOURCE_FINGERPRINT"),"source_dirty":dirty})
}
pub(crate) fn producer() -> Value {
    let mut producer = build_identity();
    // Linux exposes the actual mapped inode even after a binary upgrade/unlink.
    // Other platforms retain source identity without claiming a path hash is the image.
    #[cfg(target_os = "linux")]
    let binary = storage::digest(Path::new("/proc/self/exe")).ok();
    #[cfg(not(target_os = "linux"))]
    let binary: Option<String> = None;
    producer["binary_sha256"] = json!(binary);
    producer
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Coverage {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<crate::time_range::SelectedRange>,
    pub transcript_sha256: String,
    pub frames: u64,
    pub sample_rate: u32,
    pub chunk_frames: u64,
    pub chunks: usize,
    pub boundary_window_ms: u32,
    pub producer: Value,
}
impl Coverage {
    pub fn load(input: &Path, duration: f64) -> Result<Option<Self>> {
        Self::read(input, duration).map_err(|e| {
            crate::checkpoint::evidence_context(
                "coverage_invalid",
                &input.with_file_name("coverage.json"),
                e,
            )
        })
    }
    fn read(input: &Path, duration: f64) -> Result<Option<Self>> {
        if input.file_name().is_none_or(|n| n != "transcript.json") {
            return Ok(None);
        }
        let path = input.with_file_name("coverage.json");
        storage::reject_nonregular(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let value: Self = serde_json::from_str(&storage::read_text(&path, 16384)?)?;
        let producer_valid = value.producer.as_object().is_some_and(|p| {
            ["version", "revision", "source_fingerprint"]
                .iter()
                .all(|key| {
                    p.get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                })
                && p.get("binary_sha256").is_some_and(|v| {
                    v.is_null()
                        || v.as_str().is_some_and(|s| {
                            s.len() == 64
                                && s.bytes()
                                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                        })
                })
                && p.get("source_dirty")
                    .is_none_or(|v| v.is_null() || v.is_boolean())
                && p.keys().all(|k| {
                    matches!(
                        k.as_str(),
                        "version"
                            | "revision"
                            | "source_fingerprint"
                            | "source_dirty"
                            | "binary_sha256"
                    )
                })
        });
        let resolved = value.range;
        if let Some(range) = resolved {
            range.validate(duration)?;
            if range.source_frames != value.frames {
                return Err("range coverage differs from source frame count".into());
            }
        }
        let processed = resolved
            .map(|r| r.end_frame - r.start_frame)
            .unwrap_or(value.frames);
        if value.schema_version != resolved.map_or(1, |range| range.schema_version())
            || !producer_valid
            || value.sample_rate != 16000
            || value.frames == 0
            || value.chunk_frames == 0
            || value.boundary_window_ms > 2000
            || u64::try_from(value.chunks).ok() != Some(processed.div_ceil(value.chunk_frames))
            || (value.frames as f64 / f64::from(value.sample_rate) - duration).abs()
                > duration.abs().max(1.0) * f64::EPSILON * 4.0
            || value.transcript_sha256 != storage::digest(input)?
        {
            return Err(crate::checkpoint::evidence_failure(
                "coverage_invalid",
                &path,
                "coverage does not match the canonical transcript",
            ));
        }
        Ok(Some(value))
    }
    fn summary(&self) -> Value {
        let (start, end) = self
            .range
            .map(|r| (r.start_frame, r.end_frame))
            .unwrap_or((0, self.frames));
        json!({"status":"complete","scope":if self.range.is_some(){"audio_range_processed"}else{"audio_processed"},"frames":end-start,"sample_rate":self.sample_rate,"start_frame":start,"end_frame":end,"chunks":self.chunks,"contiguous":true})
    }
    pub(crate) fn validate_range(&self, header: &transcript::Transcript) -> Result<()> {
        if self.range != header.range {
            return Err("coverage range differs from canonical transcript".into());
        }
        Ok(())
    }
}
/// Timestamp coverage is a diagnostic, never a speech detector or accuracy score.
/// Keep only the first 32 candidates while counting every gap in a streaming pass.
#[derive(Default)]
struct SpeechCoverage {
    cursor: f64,
    first_time: Option<f64>,
    words: u64,
    untimed_segments: u64,
    gaps: u64,
    examples: Vec<Value>,
}
impl SpeechCoverage {
    fn gap(&mut self, end: f64) {
        self.first_time.get_or_insert(end);
        if end - self.cursor >= 5.0 {
            self.gaps += 1;
            if self.examples.len() < 32 {
                self.examples.push(json!({"start":self.cursor,"end":end}));
            }
        }
    }
    fn segment(&mut self, segment: &Segment) {
        if segment.words.is_empty() && !segment.text.trim().is_empty() {
            // Text without alignment cannot establish either a word gap or silence.
            self.untimed_segments += 1;
            self.gap(segment.start);
            self.cursor = self.cursor.max(segment.end);
        } else {
            for word in &segment.words {
                self.gap(word.start);
                self.cursor = self.cursor.max(word.end);
                self.words += 1;
            }
        }
    }
    fn finish_header(mut self, header: &transcript::Transcript) -> Value {
        if let Some(range) = header.range {
            let start = range.start_frame as f64 / 16000.0;
            if let Some(first) = self.first_time {
                if first >= 5.0 {
                    if first - start >= 5.0 {
                        if let Some(gap) = self.examples.first_mut() {
                            gap["start"] = json!(start);
                        }
                    } else {
                        self.gaps -= 1;
                        if !self.examples.is_empty() {
                            self.examples.remove(0);
                        }
                    }
                }
            } else {
                self.cursor = start;
            }
            self.finish(range.end_frame as f64 / 16000.0)
        } else {
            self.finish(header.duration)
        }
    }
    fn finish(mut self, duration: f64) -> Value {
        self.gap(duration);
        json!({"status":"unverified","method":"asr_timestamp_gaps",
            "minimum_gap_seconds":5,"word_count":self.words,
            "unaligned_text_segments":self.untimed_segments,"gap_count":self.gaps,
            "gaps":self.examples,"truncated":self.gaps > self.examples.len() as u64,
            "interpretation":"Gaps may be pauses, music or missed speech; no gaps does not prove complete recognition"})
    }
}
fn passages(
    segment: &Segment,
    chunk: usize,
    mut emit: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let tokens: Vec<_> = segment.text.split_whitespace().collect();
    if tokens.is_empty()
        || !tokens
            .iter()
            .copied()
            .eq(segment.words.iter().map(|w| w.word.as_str()))
    {
        return emit(
            json!({"asr_chunk":chunk,"start":segment.start,"end":segment.end,"text":segment.text,"timing":if segment.words.is_empty() {"chunk_estimate"} else {"asr_estimate"}}),
        );
    }
    let starts: Vec<_> = segment
        .text
        .char_indices()
        .filter_map(|(i, c)| {
            (!c.is_whitespace()
                && (i == 0
                    || segment.text[..i]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace)))
            .then_some(i)
        })
        .collect();
    let mut first = 0;
    let mut text_start = 0;
    for (i, word) in segment.words.iter().enumerate() {
        let elapsed = word.end - segment.words[first].start;
        let sentence = word.word.ends_with(['.', '?', '!', '…']);
        if i + 1 == segment.words.len()
            || i + 1 - first >= 60
            || elapsed >= 30.0
            || (sentence && elapsed >= 15.0)
        {
            let end = starts.get(i + 1).copied().unwrap_or(segment.text.len());
            let word_end = segment.words[first..=i]
                .iter()
                .map(|w| w.end)
                .fold(word.end, f64::max);
            emit(
                json!({"asr_chunk":chunk,"start":segment.words[first].start,"end":word_end,"text":&segment.text[text_start..end],"timing":"asr_estimate"}),
            )?;
            first = i + 1;
            text_start = end;
        }
    }
    Ok(())
}
/// Validate before writing the response prefix; keep streaming memory bounded.
pub(crate) struct View {
    reader: BufReader<fs::File>,
    header: transcript::Transcript,
    coverage: Option<Coverage>,
    speech_coverage: Value,
}
impl View {
    pub(crate) fn open(input: &Path, cancelled: &AtomicBool) -> Result<Self> {
        let mut reader = BufReader::new(storage::open_regular_input(input)?);
        let mut speech = SpeechCoverage::default();
        let header = transcript::read_streaming(&mut reader, |segment| {
            check_cancelled(cancelled)?;
            speech.segment(segment);
            Ok(())
        })?;
        let speech_coverage = speech.finish_header(&header);
        let coverage = Coverage::load(input, header.duration)?;
        if let Some(coverage) = &coverage {
            coverage.validate_range(&header)?;
        }
        Ok(Self {
            reader,
            header,
            coverage,
            speech_coverage,
        })
    }
    pub(crate) fn write(
        mut self,
        out: impl Write,
        metadata_only: bool,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let mut writer = ViewWriter::new(
            out,
            &self.header,
            self.coverage.as_ref(),
            &self.speech_coverage,
            metadata_only,
        )?;
        if !metadata_only {
            self.reader.rewind()?;
            transcript::read_streaming(&mut self.reader, |segment| {
                check_cancelled(cancelled)?;
                writer.segment(segment)
            })?;
        }
        writer.finish()
    }
}
fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        Err("interrupted".into())
    } else {
        Ok(())
    }
}
/// Stream a compact schema2 projection. Legacy evidence remains explicitly unavailable.
pub(crate) fn write_view(
    out: impl Write,
    input: &Path,
    metadata_only: bool,
    cancelled: &AtomicBool,
) -> Result<()> {
    View::open(input, cancelled)?.write(out, metadata_only, cancelled)
}
struct ViewWriter<W> {
    out: W,
    metadata_only: bool,
    chunk: usize,
    first: bool,
}
impl<W: Write> ViewWriter<W> {
    fn new(
        mut out: W,
        header: &transcript::Transcript,
        coverage: Option<&Coverage>,
        speech_coverage: &Value,
        metadata_only: bool,
    ) -> Result<Self> {
        let mut value = json!({"schema_version":2,"source":header.source,"duration":header.duration,"engine":header.engine,
            "recognizer_language":header.language,"source_language":null,"accuracy":"unverified","timing":"asr_estimates","speaker_attribution":"unverified","content_trust":"untrusted_source",
            "speech_coverage":speech_coverage,"coverage":coverage.map(Coverage::summary).unwrap_or_else(|| json!({"status":"unavailable"})),"producer":coverage.map(|c| &c.producer)});
        if let Some(range) = header.range {
            value["range"] = range.summary();
        }
        let bytes = serde_json::to_vec(&value)?;
        if metadata_only {
            out.write_all(&bytes)?;
        } else {
            out.write_all(&bytes[..bytes.len() - 1])?;
            out.write_all(b",\"passages\":[")?;
        }
        Ok(Self {
            out,
            metadata_only,
            chunk: 0,
            first: true,
        })
    }
    fn segment(&mut self, segment: &Segment) -> Result<()> {
        passages(segment, self.chunk, |value| {
            if !self.first {
                self.out.write_all(b",")?;
            }
            self.first = false;
            serde_json::to_writer(&mut self.out, &value)?;
            Ok(())
        })?;
        self.chunk += 1;
        Ok(())
    }
    fn finish(mut self) -> Result<()> {
        if !self.metadata_only {
            self.out.write_all(b"]}")?;
        }
        Ok(())
    }
}
pub(crate) fn render(transcript: &transcript::Transcript) -> Result<String> {
    let mut bytes = Vec::new();
    let mut speech = SpeechCoverage::default();
    for segment in &transcript.segments {
        speech.segment(segment);
    }
    let mut writer = ViewWriter::new(
        &mut bytes,
        transcript,
        None,
        &speech.finish_header(transcript),
        false,
    )?;
    for segment in &transcript.segments {
        writer.segment(segment)?;
    }
    writer.finish()?;
    Ok(String::from_utf8(bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speech_diagnostics_include_edges_and_do_not_double_count_overlaps() {
        let mut s = crate::transcript::tests::sample().segments.remove(0);
        s.start = 5.0;
        s.end = 20.0;
        s.words = vec![
            crate::transcript::Word {
                word: "a".into(),
                start: 5.0,
                end: 12.0,
                confidence: None,
                speaker: None,
            },
            crate::transcript::Word {
                word: "b".into(),
                start: 6.0,
                end: 7.0,
                confidence: None,
                speaker: None,
            },
            crate::transcript::Word {
                word: "c".into(),
                start: 17.0,
                end: 20.0,
                confidence: None,
                speaker: None,
            },
        ];
        let mut speech = SpeechCoverage::default();
        speech.segment(&s);
        let result = speech.finish(25.0);
        assert_eq!(result["status"], "unverified");
        assert_eq!(result["gap_count"], 3);
        assert_eq!(
            result["gaps"],
            json!([{"start":0.0,"end":5.0},{"start":12.0,"end":17.0},{"start":20.0,"end":25.0}])
        );
    }
    #[test]
    fn speech_diagnostics_bound_examples_and_keep_untimed_text_unknown() {
        let mut s = crate::transcript::tests::sample().segments.remove(0);
        s.words.clear();
        s.start = 0.0;
        s.end = 100.0;
        let mut speech = SpeechCoverage::default();
        speech.segment(&s);
        let result = speech.finish(100.0);
        assert_eq!(result["gap_count"], 0);
        assert_eq!(result["unaligned_text_segments"], 1);
        assert_eq!(result["status"], "unverified");
        let mut speech = SpeechCoverage::default();
        for i in 0..100 {
            speech.gap(i as f64 * 10.0 + 6.0);
            speech.cursor = i as f64 * 10.0 + 7.0;
        }
        let result = speech.finish(1000.0);
        assert_eq!(result["gap_count"], 100);
        assert_eq!(result["gaps"].as_array().unwrap().len(), 32);
        assert_eq!(result["truncated"], true);
        let result = SpeechCoverage::default().finish(30.0);
        assert_eq!(result["gaps"], json!([{"start":0.0,"end":30.0}]));
        assert_eq!(result["word_count"], 0);
        assert_eq!(result["status"], "unverified");
    }
    #[test]
    fn coverage_rejects_corruption_without_changing_the_transcript() {
        let root =
            std::env::temp_dir().join(format!("agent-youtube-coverage-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("transcript.json");
        let original = serde_json::to_vec(&crate::transcript::tests::sample()).unwrap();
        fs::write(&path, &original).unwrap();
        assert!(Coverage::load(&path, 2.0).unwrap().is_none());
        let good = serde_json::to_value(Coverage {
            range: None,
            schema_version: 1,
            transcript_sha256: storage::digest(&path).unwrap(),
            frames: 32000,
            sample_rate: 16000,
            chunk_frames: 160000,
            chunks: 1,
            boundary_window_ms: 0,
            producer: producer(),
        })
        .unwrap();
        fs::write(
            root.join("coverage.json"),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();
        assert!(Coverage::load(&path, 2.0).unwrap().is_some());
        for field in [
            "frames",
            "transcript_sha256",
            "producer",
            "sample_rate",
            "chunks",
        ] {
            let mut bad = good.clone();
            bad[field] = json!(0);
            fs::write(
                root.join("coverage.json"),
                serde_json::to_vec(&bad).unwrap(),
            )
            .unwrap();
            let error = Coverage::load(&path, 2.0).err().unwrap();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "coverage_invalid"
            );
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn producer_identifies_unlinked_running_image() {
        if let Some(path) = std::env::var_os("AGENT_YOUTUBE_TEST_IMAGE_COPY") {
            fs::remove_file(path).unwrap();
            assert_eq!(
                producer()["binary_sha256"],
                std::env::var("AGENT_YOUTUBE_TEST_IMAGE_HASH").unwrap()
            );
            let mut worker = crate::web::Worker::start().unwrap();
            worker.close().unwrap();
            return;
        }
        let root = std::env::temp_dir().join(format!("agent-youtube-image-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let copy = root.join("test-image");
        fs::copy("/proc/self/exe", &copy).unwrap();
        let digest = storage::digest(&copy).unwrap();
        let result = std::process::Command::new(&copy)
            .args([
                "--exact",
                "agent::tests::producer_identifies_unlinked_running_image",
            ])
            .env("AGENT_YOUTUBE_TEST_IMAGE_COPY", &copy)
            .env("AGENT_YOUTUBE_TEST_IMAGE_HASH", digest)
            .output()
            .unwrap();
        assert!(result.status.success(), "{:?}", result);
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn passages_preserve_exact_text_and_never_invent_alignment() {
        let mut s = crate::transcript::tests::sample().segments.remove(0);
        s.text = (0..125).map(|_| "word\t ").collect::<String>();
        s.start = 0.0;
        s.end = 125.0;
        s.words = (0..125)
            .map(|i| crate::transcript::Word {
                word: "word".into(),
                start: i as f64,
                end: i as f64,
                confidence: None,
                speaker: None,
            })
            .collect();
        let mut out = Vec::new();
        passages(&s, 0, |v| {
            out.push(v);
            Ok(())
        })
        .unwrap();
        assert!(out.len() > 3);
        assert_eq!(
            out.iter()
                .map(|v| v["text"].as_str().unwrap())
                .collect::<String>(),
            s.text
        );
        assert!(
            out.iter()
                .all(|v| v["end"].as_f64().unwrap() - v["start"].as_f64().unwrap() <= 30.0)
        );
        s.text = "upstream text differs".into();
        out.clear();
        passages(&s, 0, |v| {
            out.push(v);
            Ok(())
        })
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["text"], s.text);
    }
}
