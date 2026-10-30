//! Untimed encode-comparison preflight (Criterion + tests).
//!
//! Not a product API. Timing an encoder requires a successful bitstream
//! that an independent decode can consume with a finite native-rate shape.

use crate::options::{DecodeOptions, EncodeOptions};
use crate::{decode_with, encode_with};

/// One encoder enrolled in a comparison.
#[derive(Clone, Copy)]
pub struct EncodeCandidate {
    pub id: &'static str,
    pub version: &'static str,
    pub encode: fn(&[Vec<f32>], u32, u32) -> EncodeCollect,
}

/// Result of one untimed encode.
#[derive(Clone, Debug)]
pub enum EncodeCollect {
    Adts(Vec<u8>),
    Failed(String),
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodeMetrics {
    pub adts_bytes: usize,
    pub payload_bytes: usize,
    pub input_samples: usize,
    pub decoded_samples: usize,
    pub decoded_rate: u32,
    pub decoded_ch: usize,
    pub requested_bps: u32,
    pub achieved_bps: f64,
}

#[derive(Clone, Debug)]
pub enum EncodeStatus {
    Comparable {
        metrics: EncodeMetrics,
    },
    NonMatchedRate {
        metrics: EncodeMetrics,
        reason: String,
    },
    Failed {
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

impl EncodeStatus {
    pub fn is_failed(&self) -> bool {
        matches!(self, EncodeStatus::Failed { .. })
    }
    pub fn is_timed(&self) -> bool {
        matches!(
            self,
            EncodeStatus::Comparable { .. } | EncodeStatus::NonMatchedRate { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct EncodeRow {
    pub id: &'static str,
    pub version: &'static str,
    pub status: EncodeStatus,
}

#[derive(Clone, Debug)]
pub struct EncodePreflight {
    pub fixture: &'static str,
    pub rows: Vec<EncodeRow>,
}

impl EncodePreflight {
    pub fn aborted(&self) -> bool {
        self.rows.is_empty() || self.rows.iter().any(|r| r.status.is_failed())
    }

    pub fn timed_ids(&self) -> Option<Vec<&'static str>> {
        if self.aborted() {
            return None;
        }
        Some(
            self.rows
                .iter()
                .filter(|r| r.status.is_timed())
                .map(|r| r.id)
                .collect(),
        )
    }

    pub fn report(&self) -> String {
        format_report(self)
    }
}

/// ADTS payload bytes (frame length minus 7-byte header), if parseable.
pub fn adts_payload_bytes(adts: &[u8]) -> Option<usize> {
    let adts = match crate::gapless::id3_at(adts, u64::MAX) {
        Ok(crate::gapless::Id3At::Ready { len, .. }) if len <= adts.len() => &adts[len..],
        _ => adts,
    };
    let mut pos = 0usize;
    let mut payload = 0usize;
    while pos + 7 <= adts.len() {
        if adts[pos] != 0xff || adts[pos + 1] & 0xf0 != 0xf0 {
            return None;
        }
        let len = ((usize::from(adts[pos + 3] & 0x03) << 11)
            | (usize::from(adts[pos + 4]) << 3)
            | (usize::from(adts[pos + 5]) >> 5))
            .max(7);
        if pos + len > adts.len() {
            return None;
        }
        payload += len - 7;
        pos += len;
    }
    (pos == adts.len()).then_some(payload)
}

fn achieved_bps(bytes: usize, samples: usize, rate: u32) -> f64 {
    if samples == 0 || rate == 0 {
        return 0.0;
    }
    8.0 * bytes as f64 * f64::from(rate) / samples as f64
}

pub fn classify_adts(
    adts: &[u8],
    pcm: &[Vec<f32>],
    rate: u32,
    requested_bps: u32,
    expect_ch: usize,
) -> EncodeStatus {
    let Some(payload) = adts_payload_bytes(adts) else {
        return EncodeStatus::Failed {
            reason: "ADTS parse failed".into(),
        };
    };
    if adts.is_empty() {
        return EncodeStatus::Failed {
            reason: "empty bitstream".into(),
        };
    }
    let decoded = match decode_with(adts, &DecodeOptions::unbounded()) {
        Err(e) => {
            return EncodeStatus::Failed {
                reason: format!("decode of encode failed: {e}"),
            };
        }
        Ok(d) => d,
    };
    if decoded.sample_rate != rate {
        return EncodeStatus::Failed {
            reason: format!(
                "wrong output rate {} (expected {rate})",
                decoded.sample_rate
            ),
        };
    }
    if decoded.channels.len() != expect_ch {
        return EncodeStatus::Failed {
            reason: format!(
                "wrong channel count {} (expected {expect_ch})",
                decoded.channels.len()
            ),
        };
    }
    let n_in = pcm.first().map(Vec::len).unwrap_or(0);
    let n_out = decoded.channels.first().map(Vec::len).unwrap_or(0);
    if n_out == 0
        || decoded
            .channels
            .iter()
            .any(|p| p.iter().any(|x| !x.is_finite()))
    {
        return EncodeStatus::Failed {
            reason: "empty or non-finite decode".into(),
        };
    }
    let ach = achieved_bps(payload, n_in, rate);
    let metrics = EncodeMetrics {
        adts_bytes: adts.len(),
        payload_bytes: payload,
        input_samples: n_in,
        decoded_samples: n_out,
        decoded_rate: decoded.sample_rate,
        decoded_ch: decoded.channels.len(),
        requested_bps,
        achieved_bps: ach,
    };
    let rel = (ach - f64::from(requested_bps)).abs() / f64::from(requested_bps.max(1));
    if rel > 0.01 {
        EncodeStatus::NonMatchedRate {
            reason: format!(
                "achieved {:.0} bps vs requested {requested_bps} (Δ {:.1}%)",
                ach,
                rel * 100.0
            ),
            metrics,
        }
    } else {
        EncodeStatus::Comparable { metrics }
    }
}

pub fn run_encode_preflight(
    fixture: &'static str,
    pcm: &[Vec<f32>],
    rate: u32,
    requested_bps: u32,
    candidates: &[EncodeCandidate],
) -> EncodePreflight {
    let expect_ch = pcm.len();
    let mut rows = Vec::new();
    for cand in candidates {
        let status = match (cand.encode)(pcm, rate, requested_bps) {
            EncodeCollect::Failed(reason) => EncodeStatus::Failed { reason },
            EncodeCollect::Unavailable(reason) => EncodeStatus::Unavailable { reason },
            EncodeCollect::Adts(adts) => classify_adts(&adts, pcm, rate, requested_bps, expect_ch),
        };
        rows.push(EncodeRow {
            id: cand.id,
            version: cand.version,
            status,
        });
    }
    EncodePreflight { fixture, rows }
}

fn format_report(pf: &EncodePreflight) -> String {
    let mut out = format!("encode-preflight fixture={}\n", pf.fixture);
    for row in &pf.rows {
        match &row.status {
            EncodeStatus::Comparable { metrics: m } => {
                out.push_str(&format!(
                    "  {} {}: comparable ADTS {} B payload {} B in {} samp dec {} @ {} Hz {} ch req {} bps ach {:.0} bps\n",
                    row.id, row.version, m.adts_bytes, m.payload_bytes, m.input_samples,
                    m.decoded_samples, m.decoded_rate, m.decoded_ch, m.requested_bps, m.achieved_bps
                ));
            }
            EncodeStatus::NonMatchedRate { metrics: m, reason } => {
                out.push_str(&format!(
                    "  {} {}: non-matched-rate {reason}; ADTS {} B payload {} B ach {:.0} bps (not an equal-rate cell)\n",
                    row.id, row.version, m.adts_bytes, m.payload_bytes, m.achieved_bps
                ));
            }
            EncodeStatus::Failed { reason } => {
                out.push_str(&format!("  {} {}: failed {reason}\n", row.id, row.version));
            }
            EncodeStatus::Unavailable { reason } => {
                out.push_str(&format!(
                    "  {} {}: unavailable {reason}\n",
                    row.id, row.version
                ));
            }
        }
    }
    match pf.timed_ids() {
        None => out.push_str("  abort: encoder failed before timing\n"),
        Some(ids) => {
            out.push_str("  timed: ");
            out.push_str(&ids.join(", "));
            out.push('\n');
        }
    }
    out
}

pub fn syom_encode_collect(pcm: &[Vec<f32>], rate: u32, requested_bps: u32) -> EncodeCollect {
    let opts = EncodeOptions::adts().with_bitrate_bps(requested_bps);
    match encode_with(pcm, rate, &opts) {
        Ok(b) => EncodeCollect::Adts(b),
        Err(e) => EncodeCollect::Failed(e.to_string()),
    }
}

pub fn syom_encode_candidate() -> EncodeCandidate {
    EncodeCandidate {
        id: "syom",
        version: env!("CARGO_PKG_VERSION"),
        encode: syom_encode_collect,
    }
}

/// HE encode is not a product of this crate (LC only).
pub fn syom_he_unavailable(_: &[Vec<f32>], _: u32, _: u32) -> EncodeCollect {
    EncodeCollect::Unavailable("syom encodes AAC-LC only; HE encode is a separate cell".into())
}
