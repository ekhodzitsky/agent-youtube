mod bounded;
mod stream;
use serde::{Deserialize, Serialize};
pub(crate) use stream::{read_streaming, write_streaming};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transcript {
    pub schema_version: u32,
    pub source: Source,
    pub language: String,
    pub duration: f64,
    pub engine: Engine,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub url: String,
    pub video_id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Engine {
    pub name: String,
    pub version: String,
    pub model_variant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub timing: Timing,
    #[serde(default)]
    pub words: Vec<Word>,
    pub speaker: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    Words,
    Chunk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub confidence: Option<f64>,
    pub speaker: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GigasttResult {
    pub duration: f64,
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

fn interval(start: f64, end: f64, duration: f64) -> crate::Result<()> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start || end > duration {
        return Err(format!("invalid time interval {start}..{end} (duration {duration})").into());
    }
    Ok(())
}

fn validate_words(words: &[Word], start: f64, end: f64) -> crate::Result<()> {
    let mut previous_start = start;
    for word in words {
        interval(word.start, word.end, end)?;
        if word.start < previous_start {
            return Err("word timestamps are out of order or precede their segment".into());
        }
        if let Some(confidence) = word.confidence
            && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err("word confidence must be finite and between zero and one".into());
        }
        previous_start = word.start;
    }
    Ok(())
}

impl GigasttResult {
    pub fn validate(&self) -> crate::Result<()> {
        interval(0.0, self.duration, self.duration)?;
        validate_words(&self.words, 0.0, self.duration)
    }
}

impl Segment {
    pub(crate) fn validate(&self, previous_end: f64, duration: f64) -> crate::Result<()> {
        interval(self.start, self.end, duration)?;
        if self.start < previous_end {
            return Err("segments overlap or are out of order".into());
        }
        validate_words(&self.words, self.start, self.end)?;
        if self.timing == Timing::Words && self.words.is_empty() {
            return Err("word timing requires words".into());
        }
        Ok(())
    }
}

impl Transcript {
    pub fn validate(&self) -> crate::Result<()> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported transcript schema version {}",
                self.schema_version
            )
            .into());
        }
        if self.source.url.trim().is_empty()
            || self.source.video_id.trim().is_empty()
            || self.language.trim().is_empty()
            || self.engine.name.trim().is_empty()
            || self.engine.version.trim().is_empty()
            || self.engine.model_variant.trim().is_empty()
        {
            return Err("source, language and engine metadata must not be empty".into());
        }
        interval(0.0, self.duration, self.duration)?;
        let mut previous_end = 0.0;
        for segment in &self.segments {
            segment.validate(previous_end, self.duration)?;
            previous_end = segment.end;
        }
        Ok(())
    }

    pub fn from_chunks(
        source: Source,
        engine: Engine,
        chunks: Vec<(f64, GigasttResult)>,
    ) -> crate::Result<Self> {
        let mut duration = 0.0;
        let mut segments = Vec::with_capacity(chunks.len());
        for (offset, chunk) in chunks {
            chunk.validate()?;
            if !offset.is_finite() || offset < duration {
                return Err("chunk offsets must be finite, nonnegative and nonoverlapping".into());
            }
            duration = offset + chunk.duration;
            if !duration.is_finite() {
                return Err("chunk duration overflow".into());
            }
            let words: Vec<Word> = chunk
                .words
                .into_iter()
                .map(|mut word| {
                    word.start += offset;
                    word.end += offset;
                    word
                })
                .collect();
            let (start, end, timing) = match (words.first(), words.last()) {
                (Some(first), Some(_)) => (
                    first.start,
                    words.iter().map(|w| w.end).fold(first.end, f64::max),
                    Timing::Words,
                ),
                _ => (offset, duration, Timing::Chunk),
            };
            segments.push(Segment {
                start,
                end,
                text: chunk.text,
                timing,
                words,
                speaker: None,
            });
        }
        let result = Self {
            schema_version: 1,
            source,
            language: "ru".into(),
            duration,
            engine,
            segments,
        };
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Transcript {
        Transcript::from_chunks(
            Source {
                url: "https://youtu.be/example".into(),
                video_id: "example".into(),
                title: None,
            },
            Engine {
                name: "gigastt".into(),
                version: "test".into(),
                model_variant: "v3".into(),
            },
            vec![(
                0.0,
                GigasttResult {
                    duration: 2.0,
                    text: "Привет, мир!".into(),
                    words: vec![],
                },
            )],
        )
        .unwrap()
    }

    #[test]
    fn json_roundtrip_preserves_exact_text_and_absent_metadata() {
        let original = sample();
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: Transcript = serde_json::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(original, decoded);
        assert_eq!(decoded.source.title, None);
        assert_eq!(decoded.segments[0].speaker, None);
        assert!(decoded.segments[0].words.is_empty());
        assert_eq!(decoded.segments[0].timing, Timing::Chunk);
    }

    #[test]
    fn json_preserves_asr_confidence_and_time_float_roundtrips() {
        // Real recognizer confidence that the fast JSON parser rounded up by one ULP.
        let raw = r#"{"duration":1.0,"text":"word","words":[{"word":"word","start":0.1,"end":0.9989326596260071,"confidence":0.9989326596260071}]}"#;
        let chunk: GigasttResult = serde_json::from_str(raw).unwrap();
        let expected = "0.9989326596260071".parse::<f64>().unwrap();
        assert_eq!(
            chunk.words[0].confidence.unwrap().to_bits(),
            expected.to_bits()
        );
        assert_eq!(chunk.words[0].end.to_bits(), expected.to_bits());
        let template = sample();
        let transcript =
            Transcript::from_chunks(template.source, template.engine, vec![(0.0, chunk)]).unwrap();
        let encoded = serde_json::to_vec(&transcript).unwrap();
        let decoded: Transcript = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, transcript);
        assert_eq!(
            decoded.segments[0].words[0].confidence.unwrap().to_bits(),
            expected.to_bits()
        );
    }

    #[test]
    fn rejects_schema_and_nonfinite_or_invalid_times() {
        let mut transcript = sample();
        transcript.schema_version = 2;
        assert!(transcript.validate().is_err());
        transcript.schema_version = 1;
        for value in [f64::NAN, f64::INFINITY, -1.0] {
            transcript.duration = value;
            assert!(transcript.validate().is_err());
        }
        transcript.duration = 2.0;
        transcript.segments[0].end = 3.0;
        assert!(transcript.validate().is_err());
    }

    #[test]
    fn words_keep_metadata_and_zero_duration_without_inventing_segment_speaker() {
        let mut transcript = sample();
        let chunk = GigasttResult {
            duration: 1.0,
            text: " Да.\n".into(),
            words: vec![Word {
                word: "Да.".into(),
                start: 0.5,
                end: 0.5,
                confidence: Some(0.8),
                speaker: Some(0),
            }],
        };
        transcript =
            Transcript::from_chunks(transcript.source, transcript.engine, vec![(2.0, chunk)])
                .unwrap();
        assert_eq!(transcript.segments[0].text, " Да.\n");
        assert_eq!(transcript.segments[0].start, 2.5);
        assert_eq!(transcript.segments[0].end, 2.5);
        assert_eq!(transcript.segments[0].words[0].speaker, Some(0));
        assert_eq!(transcript.segments[0].speaker, None);
    }

    #[test]
    fn rejects_invalid_words_and_overlapping_chunks() {
        let mut chunk = GigasttResult {
            duration: 1.0,
            text: "a".into(),
            words: vec![Word {
                word: "a".into(),
                start: 0.0,
                end: 1.0,
                confidence: Some(2.0),
                speaker: None,
            }],
        };
        assert!(chunk.validate().is_err());
        chunk.words[0].confidence = None;
        let sample = sample();
        assert!(
            Transcript::from_chunks(
                sample.source,
                sample.engine,
                vec![(0.0, chunk.clone()), (0.5, chunk)]
            )
            .is_err()
        );
    }
}
