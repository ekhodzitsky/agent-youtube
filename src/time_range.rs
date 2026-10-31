//! Integer timestamps and frame-exact recognition selections.
use crate::{Result, protocol};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const RATE: u64 = 16000;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub start_millis: u64,
    pub end_millis: Option<u64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedRange {
    pub requested: TimeRange,
    pub start_frame: u64,
    pub end_frame: u64,
    pub source_frames: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_prefix: Option<DecodedPrefix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloaded_prefix: Option<DownloadedPrefix>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DownloadedPrefix {
    pub bytes: u64,
    pub source_bytes: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "source_frames_basis",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DecodedPrefix {
    ContainerIndex { through_frame: u64 },
}
impl DecodedPrefix {
    pub(crate) fn through_frame(self) -> u64 {
        match self {
            Self::ContainerIndex { through_frame } => through_frame,
        }
    }
}
fn invalid() -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure(
        "invalid_time_range",
        Some(false),
        "use nonnegative seconds or HH:MM:SS with at most three decimal places; end must exceed start and timestamps must fit the frame counter",
    )
}
pub(crate) fn timestamp(value: &str) -> Result<u64> {
    let parts: Vec<_> = value.split(':').collect();
    if !matches!(parts.len(), 1 | 3) {
        return Err(invalid());
    }
    let seconds = parts.last().copied().ok_or_else(invalid)?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let number = |s: &str| -> Result<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        s.parse().map_err(|_| invalid())
    };
    if fraction.len() > 3
        || (seconds.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let mut total = number(whole)?;
    if parts.len() == 3 {
        if parts[1].len() != 2 || whole.len() != 2 {
            return Err(invalid());
        }
        let minutes = number(parts[1])?;
        if minutes >= 60 || total >= 60 {
            return Err(invalid());
        }
        total = number(parts[0])?
            .checked_mul(3600)
            .and_then(|h| h.checked_add(minutes * 60))
            .and_then(|n| n.checked_add(total))
            .ok_or_else(invalid)?;
    }
    let millis = if fraction.is_empty() {
        0
    } else {
        number(fraction)? * 10_u64.pow(3 - fraction.len() as u32)
    };
    let result = total
        .checked_mul(1000)
        .and_then(|n| n.checked_add(millis))
        .ok_or_else(invalid)?;
    result.checked_mul(RATE / 1000).ok_or_else(invalid)?;
    Ok(result)
}
impl TimeRange {
    pub fn validate(self) -> Result<()> {
        self.start_millis
            .checked_mul(RATE / 1000)
            .ok_or_else(invalid)?;
        if let Some(end) = self.end_millis {
            end.checked_mul(RATE / 1000).ok_or_else(invalid)?;
            if end <= self.start_millis {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub(crate) fn resolve(self, frames: u64) -> Result<SelectedRange> {
        self.validate()?;
        let start_frame = self.start_millis * (RATE / 1000);
        let end_frame = self
            .end_millis
            .map(|v| v * (RATE / 1000))
            .unwrap_or(frames)
            .min(frames);
        if start_frame >= end_frame {
            return Err(protocol::failure(
                "range_out_of_bounds",
                Some(false),
                "requested range contains no decoded audio; owned media preserved",
            ));
        }
        Ok(SelectedRange {
            requested: self,
            start_frame,
            end_frame,
            source_frames: frames,
            decoded_prefix: None,
            downloaded_prefix: None,
        })
    }
}
impl SelectedRange {
    pub(crate) fn schema_version(self) -> u32 {
        if self.decoded_prefix.is_some() { 3 } else { 2 }
    }
    pub(crate) fn validate(self, duration: f64) -> Result<()> {
        let mut expected = self.requested.resolve(self.source_frames)?;
        expected.decoded_prefix = self.decoded_prefix;
        expected.downloaded_prefix = self.downloaded_prefix;
        if expected != self
            || (self.source_frames as f64 / RATE as f64 - duration).abs()
                > duration.abs().max(1.0) * f64::EPSILON * 4.0
        {
            return Err(invalid());
        }
        if let Some(prefix) = self.downloaded_prefix
            && (self.decoded_prefix.is_none()
                || prefix.bytes == 0
                || prefix.bytes >= prefix.source_bytes)
        {
            return Err(invalid());
        }
        if let Some(prefix) = self.decoded_prefix
            && (self.requested.end_millis.is_none()
                || prefix.through_frame() < self.end_frame
                || prefix.through_frame() >= self.source_frames)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn summary(self) -> Value {
        let mut value = json!({"requested_start_seconds":self.requested.start_millis as f64/1000.0,"requested_end_seconds":self.requested.end_millis.map(|v|v as f64/1000.0),"start_seconds":self.start_frame as f64/RATE as f64,"end_seconds":self.end_frame as f64/RATE as f64,"download":"full_audio"});
        if let Some(prefix) = self.decoded_prefix {
            value["decode"] = json!("prefix");
            value["decoded_through_seconds"] = json!(prefix.through_frame() as f64 / RATE as f64);
            value["source_frames_basis"] = json!("container_index");
        }
        if let Some(prefix) = self.downloaded_prefix {
            value["download"] = json!("prefix");
            value["downloaded_bytes"] = json!(prefix.bytes);
            value["source_bytes"] = json!(prefix.source_bytes);
        }
        value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_are_strict_and_exact() {
        for (v, n) in [
            ("0", 0),
            ("1.001", 1001),
            ("00:15:00", 900000),
            ("200:01:02.03", 720062030),
        ] {
            assert_eq!(timestamp(v).unwrap(), n);
        }
        for v in [
            "-1",
            "NaN",
            "inf",
            "1e3",
            "1.",
            "1.0001",
            " 1",
            "1:20",
            "00:60:00",
            "00:00:60",
            "00:0:01",
            "18446744073709551615",
        ] {
            assert!(timestamp(v).is_err(), "{v}");
        }
    }
    #[test]
    fn selection_clamps_only_the_end() {
        let r = TimeRange {
            start_millis: 1000,
            end_millis: Some(5000),
        }
        .resolve(32001)
        .unwrap();
        assert_eq!((r.start_frame, r.end_frame), (16000, 32001));
        r.validate(32001.0 / 16000.0).unwrap();
        assert!(
            TimeRange {
                start_millis: 3000,
                end_millis: None
            }
            .resolve(32001)
            .is_err()
        );
        assert!(
            TimeRange {
                start_millis: 10,
                end_millis: Some(10)
            }
            .validate()
            .is_err()
        );
    }
}
