use super::{Engine, Segment, Source, Transcript};
use crate::Result;
use serde::{
    Serialize,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
    ser::{Error as _, SerializeSeq},
};
use std::{
    cell::RefCell,
    fmt,
    io::{Read, Write},
};

struct Segments<'a, I> {
    values: RefCell<I>,
    duration: f64,
    failure: &'a RefCell<Option<Box<dyn std::error::Error + Send + Sync>>>,
}

impl<I: Iterator<Item = Result<Segment>>> Serialize for Segments<'_, I> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        let mut previous = 0.0;
        for value in self.values.borrow_mut().by_ref() {
            let segment = match value.and_then(|segment| {
                segment.validate(previous, self.duration)?;
                Ok(segment)
            }) {
                Ok(segment) => segment,
                Err(error) => {
                    let message = S::Error::custom(&error);
                    *self.failure.borrow_mut() = Some(error);
                    return Err(message);
                }
            };
            previous = segment.end;
            seq.serialize_element(&segment)?;
        }
        seq.end()
    }
}

/// Serialize one validated segment at a time, preserving schema1 and exact text.
pub(crate) fn write_streaming(
    writer: impl Write,
    header: &Transcript,
    segments: impl Iterator<Item = Result<Segment>>,
) -> Result<()> {
    header.validate()?;
    #[derive(Serialize)]
    struct Document<'a, S> {
        schema_version: u32,
        source: &'a Source,
        language: &'a str,
        duration: f64,
        engine: &'a Engine,
        segments: S,
    }
    let failure = RefCell::new(None);
    let exceeded = std::cell::Cell::new(false);
    let writer = super::bounded::Writer::new(writer, &exceeded);
    let result = serde_json::to_writer_pretty(
        writer,
        &Document {
            schema_version: header.schema_version,
            source: &header.source,
            language: &header.language,
            duration: header.duration,
            engine: &header.engine,
            segments: Segments {
                values: RefCell::new(segments),
                duration: header.duration,
                failure: &failure,
            },
        },
    );
    if let Some(error) = failure.into_inner() {
        return Err(error);
    }
    if exceeded.get() {
        return Err(super::bounded::failure());
    }
    result?;
    Ok(())
}

struct Scan<'a, F>(&'a mut F);
struct SegmentScan<'a, F> {
    callback: &'a mut F,
}

impl<'de, F: FnMut(&Segment) -> Result<()>> DeserializeSeed<'de> for SegmentScan<'_, F> {
    type Value = f64;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<f64, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de, F: FnMut(&Segment) -> Result<()>> Visitor<'de> for SegmentScan<'_, F> {
    type Value = f64;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("transcript segments")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<f64, A::Error> {
        let mut previous = 0.0;
        while let Some(segment) = seq.next_element::<Segment>()? {
            segment
                .validate(previous, f64::MAX)
                .map_err(de::Error::custom)?;
            previous = segment.end;
            (self.callback)(&segment).map_err(de::Error::custom)?;
        }
        Ok(previous)
    }
}
impl<'de, F: FnMut(&Segment) -> Result<()>> DeserializeSeed<'de> for Scan<'_, F> {
    type Value = Transcript;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Transcript, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de, F: FnMut(&Segment) -> Result<()>> Visitor<'de> for Scan<'_, F> {
    type Value = Transcript;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a transcript object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Transcript, A::Error> {
        let (mut version, mut source, mut language, mut duration, mut engine, mut end) =
            (None, None, None, None, None, None);
        macro_rules! field {
            ($target:ident, $name:literal) => {{
                if $target.is_some() {
                    return Err(de::Error::duplicate_field($name));
                }
                $target = Some(map.next_value()?);
            }};
        }
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema_version" => field!(version, "schema_version"),
                "source" => field!(source, "source"),
                "language" => field!(language, "language"),
                "duration" => field!(duration, "duration"),
                "engine" => field!(engine, "engine"),
                "segments" => {
                    if end.is_some() {
                        return Err(de::Error::duplicate_field("segments"));
                    }
                    end = Some(map.next_value_seed(SegmentScan { callback: self.0 })?);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let result = Transcript {
            schema_version: version.ok_or_else(|| de::Error::missing_field("schema_version"))?,
            source: source.ok_or_else(|| de::Error::missing_field("source"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            duration: duration.ok_or_else(|| de::Error::missing_field("duration"))?,
            engine: engine.ok_or_else(|| de::Error::missing_field("engine"))?,
            segments: Vec::new(),
        };
        result.validate().map_err(de::Error::custom)?;
        if end.ok_or_else(|| de::Error::missing_field("segments"))? > result.duration {
            return Err(de::Error::custom("segments exceed transcript duration"));
        }
        Ok(result)
    }
}

/// Read in any field order. Callbacks must not publish until the full scan succeeds.
pub(crate) fn read_streaming(
    reader: impl Read,
    mut callback: impl FnMut(&Segment) -> Result<()>,
) -> Result<Transcript> {
    let exceeded = std::cell::Cell::new(false);
    let reader = super::bounded::Reader::new(reader, &exceeded);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let mut failure = None;
    let result = Scan(&mut |segment: &Segment| {
        callback(segment).map_err(|error| {
            failure = Some(error);
            "transcript callback failed".into()
        })
    })
    .deserialize(&mut deserializer);
    if let Some(error) = failure {
        return Err(error);
    }
    let result = result.and_then(|header| deserializer.end().map(|()| header));
    if exceeded.get() {
        return Err(super::bounded::failure());
    }
    let header = result?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn long_streams_keep_per_segment_budgets_and_roundtrip_escapes() {
        let mut sample = crate::transcript::tests::sample();
        sample.segments[0].text = "\"\\,:{}[]".repeat(100_000);
        sample.duration = 80.0;
        let segments = (0..40).map(|i| {
            let mut segment = sample.segments[0].clone();
            segment.start = i as f64 * 2.0;
            segment.end = segment.start + 2.0;
            segment.words.clear();
            Ok(segment)
        });
        let mut output = Vec::new();
        write_streaming(&mut output, &sample, segments).unwrap();
        assert!(output.len() > super::super::bounded::LIMIT);
        let mut count = 0;
        read_streaming(Cursor::new(output), |segment| {
            assert_eq!(segment.text, sample.segments[0].text);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 40);
        sample.segments[0].text = "x".repeat(super::super::bounded::LIMIT + 1);
        let error = write_streaming(
            io::sink(),
            &sample,
            sample.segments.clone().into_iter().map(Ok),
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "resource_limit"
        );
    }

    #[test]
    fn oversized_strings_and_segments_return_resource_errors() {
        let mut sample = crate::transcript::tests::sample();
        sample.segments[0].text = "x".repeat(16 * 1024 * 1024 + 1);
        let bytes = serde_json::to_vec(&sample).unwrap();
        let error = read_streaming(Cursor::new(bytes), |_| Ok(())).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "resource_limit"
        );
        sample.segments[0].text.clear();
        sample.source.title = Some("x".repeat(16 * 1024 * 1024 + 1));
        let error = read_streaming(
            Cursor::new(serde_json::to_vec(&sample).unwrap()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "resource_limit"
        );
    }

    #[test]
    fn streaming_callbacks_preserve_typed_and_io_failures() {
        let sample = crate::transcript::tests::sample();
        let bytes = serde_json::to_vec(&sample).unwrap();
        for reading in [false, true] {
            for os_error in [false, true] {
                let failure = || -> Box<dyn std::error::Error + Send + Sync> {
                    if os_error {
                        return io::Error::from_raw_os_error(libc::ENOSPC).into();
                    }
                    crate::protocol::failure_details(
                        "checkpoint_integrity_mismatch",
                        Some(false),
                        "retained evidence changed",
                        serde_json::json!({"retry_after_seconds":123,"preserved":true}),
                    )
                };
                let error = if reading {
                    read_streaming(Cursor::new(&bytes), |_| Err(failure())).unwrap_err()
                } else {
                    write_streaming(Vec::new(), &sample, std::iter::once(Err(failure())))
                        .unwrap_err()
                };
                if os_error {
                    assert_eq!(
                        error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
                        Some(libc::ENOSPC)
                    );
                } else {
                    let typed = error.downcast_ref::<crate::protocol::Failure>().unwrap();
                    assert_eq!(typed.code, "checkpoint_integrity_mismatch");
                    assert_eq!(typed.retryable, Some(false));
                    assert_eq!(typed.retry_after_seconds(), Some(123));
                }
            }
        }
    }

    #[test]
    fn streaming_serialization_matches_existing_json_and_preserves_text() {
        let sample = crate::transcript::tests::sample();
        let mut out = Vec::new();
        write_streaming(
            &mut out,
            &sample,
            sample.segments.clone().into_iter().map(Ok),
        )
        .unwrap();
        assert_eq!(out, serde_json::to_vec_pretty(&sample).unwrap());
        let mut seen = Vec::new();
        let header = read_streaming(Cursor::new(out), |s| {
            seen.push(s.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, sample.segments);
        assert_eq!(header.duration, sample.duration);
        let reordered = serde_json::to_value(&sample).unwrap().to_string();
        let mut count = 0;
        read_streaming(Cursor::new(reordered), |_| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, sample.segments.len());
    }

    #[test]
    fn validation_has_no_whole_document_256_mib_limit() {
        let bytes = serde_json::to_vec(&crate::transcript::tests::sample()).unwrap();
        let reader = io::repeat(b' ')
            .take(256 * 1024 * 1024 + 1)
            .chain(Cursor::new(bytes));
        let mut count = 0;
        read_streaming(io::BufReader::new(reader), |_| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn streamed_validation_rejects_invalid_segment_and_word_metadata() {
        for case in [
            "overlap",
            "negative",
            "word_bounds",
            "confidence",
            "missing_words",
        ] {
            let mut value = serde_json::to_value(crate::transcript::tests::sample()).unwrap();
            match case {
                "overlap" => {
                    let extra = value["segments"][0].clone();
                    value["segments"].as_array_mut().unwrap().push(extra);
                }
                "negative" => value["segments"][0]["start"] = serde_json::json!(-1.0),
                _ => {
                    value["segments"][0]["timing"] = serde_json::json!("words");
                    if case != "missing_words" {
                        value["segments"][0]["words"] = serde_json::json!([{"word":"a","start":0.0,"end":if case == "word_bounds" {3.0} else {1.0},"confidence":if case == "confidence" {2.0} else {1.0},"speaker":null}]);
                    }
                }
            }
            assert!(
                read_streaming(Cursor::new(value.to_string()), |_| Ok(())).is_err(),
                "{case}"
            );
        }
    }

    #[test]
    fn rejects_late_invalid_headers_duplicates_missing_segments_and_trailing_data() {
        let sample = crate::transcript::tests::sample();
        let bytes = serde_json::to_string(&sample).unwrap();
        for bad in [
            bytes.replace("\"schema_version\":1", "\"schema_version\":2"),
            bytes.replace("\"duration\":2.0", "\"duration\":1.0"),
            bytes.replacen('{', "{\"schema_version\":1,", 1),
            format!("{bytes} null"),
        ] {
            assert!(read_streaming(Cursor::new(bad), |_| Ok(())).is_err());
        }
        let mut value = serde_json::to_value(sample).unwrap();
        value.as_object_mut().unwrap().remove("segments");
        assert!(read_streaming(Cursor::new(value.to_string()), |_| Ok(())).is_err());
        value["segments"] = serde_json::json!([]);
        read_streaming(Cursor::new(value.to_string()), |_| Ok(())).unwrap();
    }
}
