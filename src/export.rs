use std::str::FromStr;

use crate::transcript::{Segment, Transcript};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Md,
    Txt,
    Srt,
    Vtt,
}

impl Format {
    pub const ALL: [Self; 5] = [Self::Json, Self::Md, Self::Txt, Self::Srt, Self::Vtt];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == value)
            .ok_or_else(|| {
                format!("unsupported export format {value:?}; use json, md, txt, srt or vtt")
            })
    }
}

fn milliseconds(seconds: f64) -> crate::Result<u64> {
    let value = (seconds * 1000.0).round();
    // u64::MAX rounds up to 2^64 as f64; equality would saturate the cast.
    if !seconds.is_finite() || seconds < 0.0 || value >= u64::MAX as f64 {
        return Err(format!("timestamp {seconds} cannot be represented in milliseconds").into());
    }
    Ok(value as u64)
}

fn timestamp(seconds: f64, separator: char) -> crate::Result<String> {
    let milliseconds = milliseconds(seconds)?;
    let hours = milliseconds / 3_600_000;
    let minutes = milliseconds / 60_000 % 60;
    let seconds = milliseconds / 1000 % 60;
    let fraction = milliseconds % 1000;
    Ok(format!(
        "{hours:02}:{minutes:02}:{seconds:02}{separator}{fraction:03}"
    ))
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn subtitle_text(text: &str, speaker: Option<u32>) -> String {
    // A single escaped line prevents ASR text from becoming cue syntax or markup.
    let text = normalized(text)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    match speaker {
        Some(speaker) => format!("[Speaker {speaker}] {text}"),
        None => text,
    }
}

struct Cue {
    start: f64,
    end: f64,
    text: String,
    speaker: Option<u32>,
}

fn merge_cue(left: &mut Cue, right: Cue) {
    if left.speaker != right.speaker {
        let label = |text: &str, speaker: Option<u32>| match speaker {
            Some(speaker) => format!("[Speaker {speaker}] {text}"),
            None => text.to_owned(),
        };
        left.text = format!(
            "{} {}",
            label(&left.text, left.speaker),
            label(&right.text, right.speaker)
        );
        left.speaker = None;
    } else {
        left.text.push(' ');
        left.text.push_str(&right.text);
    }
    left.start = left.start.min(right.start);
    left.end = left.end.max(right.end);
}

fn representable(cue: &Cue) -> crate::Result<bool> {
    Ok(milliseconds(cue.end)? > milliseconds(cue.start)?)
}

fn valid_cues(cues: Vec<Cue>) -> crate::Result<Vec<Cue>> {
    let mut result: Vec<Cue> = Vec::new();
    for cue in cues {
        if let Some(last) = result.last_mut()
            && (!representable(last)? || !representable(&cue)?)
        {
            merge_cue(last, cue);
        } else {
            result.push(cue);
        }
    }
    for cue in &result {
        if !representable(cue)? {
            return Err("subtitle cue has no positive duration at millisecond precision; use JSON, TXT or Markdown to preserve instantaneous timing".into());
        }
    }
    Ok(result)
}

fn cues(segment: &Segment) -> crate::Result<Vec<Cue>> {
    let joined = segment
        .words
        .iter()
        .map(|w| w.word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if segment.words.is_empty() || normalized(&joined) != normalized(&segment.text) {
        return valid_cues(vec![Cue {
            start: segment.start,
            end: segment.end,
            text: segment.text.clone(),
            speaker: segment.speaker,
        }]);
    }
    if !segment.words.iter().any(|word| word.end > word.start) {
        return Err("subtitle segment contains only instantaneous words; use JSON, TXT or Markdown to preserve their timing".into());
    }
    let mut result: Vec<Cue> = Vec::new();
    for word in &segment.words {
        let speaker = word.speaker.or(segment.speaker);
        if let Some(last) = result.last_mut()
            && last.speaker == speaker
            && word.end - last.start <= 6.0
            && last.text.chars().count() + word.word.chars().count() < 42
        {
            last.text.push(' ');
            last.text.push_str(&word.word);
            last.end = last.end.max(word.end);
        } else {
            result.push(Cue {
                start: word.start,
                end: word.end,
                text: word.word.clone(),
                speaker,
            });
        }
    }
    valid_cues(result)
}

pub(crate) struct Renderer<W> {
    writer: W,
    format: Format,
    number: u64,
}

impl<W: std::io::Write> Renderer<W> {
    pub(crate) fn new(mut writer: W, format: Format, header: &Transcript) -> crate::Result<Self> {
        if format == Format::Md {
            let title = header.source.title.as_deref().unwrap_or("Transcript");
            write!(writer, "# {}\n\n", normalized(title))?;
            if let Some(range) = header.range {
                writeln!(
                    writer,
                    "Recognition range: {:.3} to {:.3} seconds in the original video.\n",
                    range.start_frame as f64 / 16000.0,
                    range.end_frame as f64 / 16000.0
                )?;
            }
        } else if format == Format::Vtt {
            writer.write_all(b"WEBVTT\n\n")?;
            if let Some(range) = header.range {
                writeln!(
                    writer,
                    "NOTE Recognition range: {:.3} to {:.3} seconds in the original video.\n",
                    range.start_frame as f64 / 16000.0,
                    range.end_frame as f64 / 16000.0
                )?;
            }
        }
        Ok(Self {
            writer,
            format,
            number: 0,
        })
    }

    pub(crate) fn segment(&mut self, segment: &Segment) -> crate::Result<()> {
        match self.format {
            Format::Json => return Err("JSON export requires the streaming agent view".into()),
            Format::Md | Format::Txt => {
                let start = timestamp(segment.start, '.')?;
                let speaker = segment
                    .speaker
                    .map(|s| format!(" [Speaker {s}]"))
                    .unwrap_or_default();
                writeln!(self.writer, "[{start}]{speaker} {}", segment.text)?;
                if self.format == Format::Md {
                    writeln!(self.writer)?;
                }
            }
            Format::Srt | Format::Vtt => {
                for cue in cues(segment)? {
                    self.number = self
                        .number
                        .checked_add(1)
                        .ok_or("subtitle cue count overflow")?;
                    let separator = if self.format == Format::Srt { ',' } else { '.' };
                    if self.format == Format::Srt {
                        writeln!(self.writer, "{}", self.number)?;
                    }
                    writeln!(
                        self.writer,
                        "{} --> {}\n{}\n",
                        timestamp(cue.start, separator)?,
                        timestamp(cue.end, separator)?,
                        subtitle_text(&cue.text, cue.speaker)
                    )?;
                }
            }
        }
        Ok(())
    }
}

pub fn render(transcript: &Transcript, format: Format) -> crate::Result<String> {
    transcript.validate()?;
    if format == Format::Json {
        return crate::agent::render(transcript);
    }
    let mut bytes = Vec::new();
    let mut renderer = Renderer::new(&mut bytes, format, transcript)?;
    for segment in &transcript.segments {
        renderer.segment(segment)?;
    }
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Timing, Word};

    #[test]
    fn exact_exports() {
        let transcript = crate::transcript::tests::sample();
        assert_eq!(
            render(&transcript, Format::Txt).unwrap(),
            "[00:00:00.000] Привет, мир!\n"
        );
        assert_eq!(
            render(&transcript, Format::Md).unwrap(),
            "# Transcript\n\n[00:00:00.000] Привет, мир!\n\n"
        );
        assert_eq!(
            render(&transcript, Format::Srt).unwrap(),
            "1\n00:00:00,000 --> 00:00:02,000\nПривет, мир!\n\n"
        );
        assert_eq!(
            render(&transcript, Format::Vtt).unwrap(),
            "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nПривет, мир!\n\n"
        );
    }

    #[test]
    fn hour_rollover_and_zero_duration_cues() {
        assert_eq!(timestamp(3599.9996, ',').unwrap(), "01:00:00,000");
        let mut transcript = crate::transcript::tests::sample();
        transcript.segments[0].end = 0.0;
        let original = transcript.clone();
        for format in [Format::Srt, Format::Vtt] {
            assert!(
                render(&transcript, format)
                    .unwrap_err()
                    .to_string()
                    .contains("no positive duration")
            );
        }
        assert_eq!(transcript, original);
        assert!(render(&transcript, Format::Txt).is_ok());
        transcript.segments[0].words = vec![Word {
            word: "Привет, мир!".into(),
            start: 0.0,
            end: 0.0,
            confidence: None,
            speaker: None,
        }];
        assert!(
            render(&transcript, Format::Srt)
                .unwrap_err()
                .to_string()
                .contains("only instantaneous words")
        );
    }

    #[test]
    fn rejects_timestamps_that_overflow_milliseconds() {
        for seconds in [
            f64::NAN,
            f64::INFINITY,
            -1.0,
            u64::MAX as f64 / 1000.0,
            f64::MAX,
        ] {
            assert!(timestamp(seconds, '.').is_err());
        }
        let mut transcript = crate::transcript::tests::sample();
        transcript.duration = 2e20;
        transcript.segments[0].start = 1e20;
        transcript.segments[0].end = 2e20;
        for format in [Format::Md, Format::Txt, Format::Srt, Format::Vtt] {
            assert!(render(&transcript, format).is_err());
        }
        transcript.segments[0].start = 0.0;
        for format in [Format::Srt, Format::Vtt] {
            assert!(render(&transcript, format).is_err());
        }
    }

    #[test]
    fn large_representable_hours_are_not_truncated() {
        let mut transcript = crate::transcript::tests::sample();
        transcript.duration = 360_000_001.0;
        transcript.segments[0].start = 360_000_000.0;
        transcript.segments[0].end = 360_000_001.0;
        assert!(
            render(&transcript, Format::Srt)
                .unwrap()
                .contains("100000:00:00,000 --> 100000:00:01,000")
        );
    }

    #[test]
    fn zero_duration_words_merge_into_real_interval_without_changing_canonical_data() {
        let mut transcript = crate::transcript::tests::sample();
        let segment = &mut transcript.segments[0];
        segment.text = "Начало середина конец".into();
        segment.words = vec![
            Word {
                word: "Начало".into(),
                start: 0.0,
                end: 0.0,
                confidence: None,
                speaker: Some(0),
            },
            Word {
                word: "середина".into(),
                start: 0.0,
                end: 1.0,
                confidence: None,
                speaker: Some(1),
            },
            Word {
                word: "конец".into(),
                start: 1.0,
                end: 1.0,
                confidence: None,
                speaker: Some(2),
            },
        ];
        let original = transcript.clone();
        assert_eq!(
            render(&transcript, Format::Vtt).unwrap(),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n[Speaker 0] Начало [Speaker 1] середина [Speaker 2] конец\n\n"
        );
        assert_eq!(transcript, original);
    }

    #[test]
    fn submillisecond_words_coalesce_and_unrepresentable_chunk_reports_error() {
        let mut transcript = crate::transcript::tests::sample();
        let segment = &mut transcript.segments[0];
        segment.text = "а б".into();
        segment.words = vec![
            Word {
                word: "а".into(),
                start: 0.0,
                end: 0.0001,
                confidence: None,
                speaker: Some(0),
            },
            Word {
                word: "б".into(),
                start: 0.0002,
                end: 0.002,
                confidence: None,
                speaker: Some(1),
            },
        ];
        let output = render(&transcript, Format::Srt).unwrap();
        assert!(output.contains("00:00:00,000 --> 00:00:00,002"));
        assert!(output.contains("[Speaker 0] а [Speaker 1] б"));
        transcript.segments[0].words.clear();
        transcript.segments[0].end = 0.0001;
        assert!(render(&transcript, Format::Vtt).is_err());
    }

    #[test]
    fn mismatching_words_do_not_replace_asr_text() {
        let mut transcript = crate::transcript::tests::sample();
        transcript.segments[0].words = vec![Word {
            word: "Другое".into(),
            start: 0.0,
            end: 1.0,
            confidence: None,
            speaker: None,
        }];
        assert!(
            render(&transcript, Format::Srt)
                .unwrap()
                .contains("Привет, мир!")
        );
    }

    #[test]
    fn word_cues_preserve_speakers_and_split_when_needed() {
        let mut transcript = crate::transcript::tests::sample();
        let segment = &mut transcript.segments[0];
        segment.timing = Timing::Words;
        segment.words = vec![
            Word {
                word: "Привет,".into(),
                start: 0.0,
                end: 0.5,
                confidence: None,
                speaker: Some(0),
            },
            Word {
                word: "мир!".into(),
                start: 1.0,
                end: 2.0,
                confidence: None,
                speaker: Some(1),
            },
        ];
        assert_eq!(
            render(&transcript, Format::Srt).unwrap(),
            "1\n00:00:00,000 --> 00:00:00,500\n[Speaker 0] Привет,\n\n2\n00:00:01,000 --> 00:00:02,000\n[Speaker 1] мир!\n\n"
        );
    }

    #[test]
    fn prevents_subtitle_markup_and_cue_injection() {
        let mut transcript = crate::transcript::tests::sample();
        transcript.segments[0].text = "<v bad>&\n\n00:00:00 --> 01:00:00\nWEBVTT".into();
        let output = render(&transcript, Format::Vtt).unwrap();
        assert_eq!(
            output,
            "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\n&lt;v bad&gt;&amp; 00:00:00 --&gt; 01:00:00 WEBVTT\n\n"
        );
    }

    #[test]
    fn format_parser_rejects_unknown_formats() {
        assert_eq!("md".parse::<Format>().unwrap(), Format::Md);
        assert_eq!("json".parse::<Format>().unwrap(), Format::Json);
        assert!("xml".parse::<Format>().is_err());
    }
}
