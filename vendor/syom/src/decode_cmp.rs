//! Untimed decode-comparison preflight used by Criterion benches and tests.
//!
//! Not a product API. Timed comparisons must not rank failed, empty, or
//! unequal-shape work: a candidate is timed only after this preflight
//! accepts matching native rate, channel count, sample count, finite
//! samples, and consumed output.

use crate::options::DecodeOptions;
use crate::{decode_streaming, decode_with};

/// Named equal-work lanes. Primary comparisons use [`Lane::PlanarSplit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Planar split PCM at the bitstream's native rate.
    PlanarSplit,
    /// Speech mono downmix (`DecodeOptions::speech()` for syom).
    SpeechDownmix,
    /// Codec-only: preflight still collects PCM; timing may drop it.
    DiscardOutput,
}

impl Lane {
    /// Stable lane name used in reports and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::PlanarSplit => "planar_split",
            Lane::SpeechDownmix => "speech_downmix",
            Lane::DiscardOutput => "discard_output",
        }
    }
}

/// Output geometry after a successful collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: usize,
}

/// Planar f32 PCM collected outside the timed loop.
#[derive(Clone, Debug)]
pub struct Pcm {
    pub sample_rate: u32,
    pub planes: Vec<Vec<f32>>,
}

impl Pcm {
    pub fn shape(&self) -> Shape {
        Shape {
            sample_rate: self.sample_rate,
            channels: self.planes.len(),
            samples: self.planes.first().map(Vec::len).unwrap_or(0),
        }
    }
}

/// Result of one untimed collect.
#[derive(Clone, Debug)]
pub enum Collect {
    Pcm(Pcm),
    Failed(String),
    Unavailable(String),
}

/// One decoder enrolled in a comparison.
#[derive(Clone, Copy)]
pub struct Candidate {
    pub id: &'static str,
    pub version: &'static str,
    pub collect: fn(&[u8], Lane) -> Collect,
}

/// Outcome of classifying a collect against the reference shape.
#[derive(Clone, Debug)]
pub enum Status {
    Comparable {
        shape: Shape,
        checksum: u64,
    },
    NonComparable {
        reason: String,
        observed: Option<Shape>,
    },
    Failed {
        reason: String,
    },
}

impl Status {
    pub fn is_failed(&self) -> bool {
        matches!(self, Status::Failed { .. })
    }

    pub fn is_comparable(&self) -> bool {
        matches!(self, Status::Comparable { .. })
    }
}

/// One candidate row in a preflight report.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: &'static str,
    pub version: &'static str,
    pub status: Status,
}

/// Untimed comparison result. `timed_ids` is `None` when the group aborts.
#[derive(Clone, Debug)]
pub struct Preflight {
    pub fixture: &'static str,
    pub lane: Lane,
    pub rows: Vec<Row>,
}

impl Preflight {
    pub fn aborted(&self) -> bool {
        self.rows.iter().any(|r| r.status.is_failed()) || self.rows.is_empty()
    }

    /// Candidate ids allowed to enter Criterion. `None` means abort: do not
    /// emit a timed result for anyone in this group.
    pub fn timed_ids(&self) -> Option<Vec<&'static str>> {
        if self.aborted() {
            return None;
        }
        Some(
            self.rows
                .iter()
                .filter(|r| r.status.is_comparable())
                .map(|r| r.id)
                .collect(),
        )
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn report(&self) -> String {
        format_report(self)
    }
}

/// FNV-1a over IEEE-754 bits of every consumed sample (plane-major).
pub fn checksum(planes: &[Vec<f32>]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut h = OFFSET;
    for plane in planes {
        for &x in plane {
            for b in x.to_bits().to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(PRIME);
            }
        }
    }
    h
}

/// Mean of non-LFE planes. 5.1 lavc order skips index 3 (LFE).
pub fn downmix_speech(pcm: Pcm) -> Pcm {
    if pcm.planes.len() <= 1 {
        return pcm;
    }
    let n = pcm.planes[0].len();
    let skip = (pcm.planes.len() == 6).then_some(3);
    let mut mix = vec![0.0f32; n];
    let mut count = 0.0f32;
    for (i, plane) in pcm.planes.iter().enumerate() {
        if Some(i) == skip {
            continue;
        }
        count += 1.0;
        for (dst, &src) in mix.iter_mut().zip(plane.iter()) {
            *dst += src;
        }
    }
    if count > 0.0 {
        for s in &mut mix {
            *s /= count;
        }
    }
    Pcm {
        sample_rate: pcm.sample_rate,
        planes: vec![mix],
    }
}

fn valid_pcm(pcm: &Pcm) -> Result<Shape, String> {
    if pcm.sample_rate == 0 {
        return Err("zero sample rate".into());
    }
    if pcm.planes.is_empty() {
        return Err("empty output".into());
    }
    let n = pcm.planes[0].len();
    if n == 0 {
        return Err("empty output".into());
    }
    if pcm.planes.iter().any(|p| p.len() != n) {
        return Err("unequal plane lengths".into());
    }
    if pcm.planes.iter().any(|p| p.iter().any(|x| !x.is_finite())) {
        return Err("non-finite samples".into());
    }
    Ok(pcm.shape())
}

/// Classify one collect against the reference geometry.
pub fn classify(got: Collect, expect: Option<Shape>) -> Status {
    match got {
        Collect::Failed(reason) => Status::Failed { reason },
        Collect::Unavailable(reason) => Status::NonComparable {
            reason,
            observed: None,
        },
        Collect::Pcm(pcm) => match valid_pcm(&pcm) {
            Err(reason) => Status::Failed { reason },
            Ok(shape) => match expect {
                Some(exp) if exp != shape => Status::NonComparable {
                    reason: format!(
                        "mismatched output: {} Hz {} ch {} samples (expected {} Hz {} ch {} samples)",
                        shape.sample_rate,
                        shape.channels,
                        shape.samples,
                        exp.sample_rate,
                        exp.channels,
                        exp.samples
                    ),
                    observed: Some(shape),
                },
                _ => Status::Comparable {
                    checksum: checksum(&pcm.planes),
                    shape,
                },
            },
        },
    }
}

fn apply_lane(got: Collect, lane: Lane) -> Collect {
    match (lane, got) {
        (Lane::SpeechDownmix, Collect::Pcm(pcm)) if pcm.planes.len() > 1 => {
            Collect::Pcm(downmix_speech(pcm))
        }
        (_, got) => got,
    }
}

/// Run the untimed preflight. `candidates[0]` is the reference shape.
pub fn run_preflight(
    fixture: &'static str,
    bytes: &[u8],
    lane: Lane,
    candidates: &[Candidate],
) -> Preflight {
    let collect_lane = match lane {
        Lane::DiscardOutput => Lane::PlanarSplit,
        other => other,
    };
    let mut rows = Vec::with_capacity(candidates.len());
    let mut expect = None;
    for (i, cand) in candidates.iter().enumerate() {
        let got = apply_lane((cand.collect)(bytes, collect_lane), collect_lane);
        if i == 0
            && let Collect::Pcm(pcm) = &got
        {
            expect = valid_pcm(pcm).ok();
        }
        rows.push(Row {
            id: cand.id,
            version: cand.version,
            status: classify(got, expect),
        });
    }
    Preflight {
        fixture,
        lane,
        rows,
    }
}

fn format_report(pf: &Preflight) -> String {
    let mut out = format!(
        "preflight fixture={} lane={}\n",
        pf.fixture,
        pf.lane.as_str()
    );
    for row in &pf.rows {
        match &row.status {
            Status::Comparable { shape, checksum } => {
                out.push_str(&format!(
                    "  {} {}: comparable {} Hz {} ch {} samples checksum={checksum:#018x}\n",
                    row.id, row.version, shape.sample_rate, shape.channels, shape.samples
                ));
            }
            Status::NonComparable { reason, observed } => {
                let obs = observed.map(|s| {
                    format!(
                        " [{} Hz {} ch {} samples]",
                        s.sample_rate, s.channels, s.samples
                    )
                });
                out.push_str(&format!(
                    "  {} {}: non-comparable {reason}{}\n",
                    row.id,
                    row.version,
                    obs.as_deref().unwrap_or("")
                ));
            }
            Status::Failed { reason } => {
                out.push_str(&format!("  {} {}: failed {reason}\n", row.id, row.version));
            }
        }
    }
    match pf.timed_ids() {
        None => out.push_str("  abort: candidate failed before timing\n"),
        Some(ids) => {
            out.push_str("  timed: ");
            out.push_str(&ids.join(", "));
            out.push('\n');
        }
    }
    out
}

/// syom collect: split/unbounded, or `speech()` on the speech lane.
pub fn syom_collect(bytes: &[u8], lane: Lane) -> Collect {
    let opts = match lane {
        Lane::SpeechDownmix => DecodeOptions::speech(),
        Lane::PlanarSplit | Lane::DiscardOutput => DecodeOptions::unbounded(),
    };
    match decode_with(bytes, &opts) {
        Err(e) => Collect::Failed(e.to_string()),
        Ok(d) => Collect::Pcm(Pcm {
            sample_rate: d.sample_rate,
            planes: d.channels,
        }),
    }
}

/// Codec-only syom path: decode without accumulating PCM.
pub fn syom_discard(bytes: &[u8]) -> Result<usize, String> {
    let mut n = 0usize;
    decode_streaming(bytes, &DecodeOptions::unbounded(), |f| {
        n += f.samples;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(n)
}

/// Reference candidate (always first in a comparison group).
pub fn syom_candidate() -> Candidate {
    Candidate {
        id: "syom",
        version: env!("CARGO_PKG_VERSION"),
        collect: syom_collect,
    }
}
