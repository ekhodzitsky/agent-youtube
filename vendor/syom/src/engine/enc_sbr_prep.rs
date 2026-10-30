//! HE v1 core-rate preparation: 2:1 halfband downsample + 64-band analysis QMF.
//!
//! Crate-internal (TASK-86). Does not change [`crate::encode`].

use super::adts::ADTS_SAMPLE_RATES_HZ;
use super::enc_sbr_qmf::{EncAnalysisQmf, EncSlot};
use super::error::{Error, Result};

/// Output-rate samples per LC/SBR frame (2× 1024 core).
pub(crate) const OUT_FRAME: usize = 2048;
/// Core samples per frame.
pub(crate) const CORE_FRAME: usize = 1024;
/// Analysis QMF slot length (64 bands → 64 output-rate samples; 32
/// slots per frame, the decoder's `X` grid).
pub(crate) const QMF_SLOT: usize = 64;
/// QMF history in samples; drain this many zero samples after the last
/// real sample so the prototype delay appears in the slots.
pub(crate) const QMF_DRAIN: usize = 640;
/// FIR delay at the output rate (centre of [`HALF_BAND`]).
pub(crate) const FIR_DELAY: usize = 8;

/// 17-tap Hann-windowed sinc, cutoff 12 kHz @ 48 kHz (π/2 of a 2:1
/// downsampler). DC gain 1. Group delay [`FIR_DELAY`].
const HALF_BAND: [f32; 17] = [
    0.0,
    -1.728_078_9e-3,
    0.0,
    1.961_995_9e-2,
    0.0,
    -7.324_225e-2,
    0.0,
    3.057_3e-1,
    4.992_407_6e-1,
    3.057_3e-1,
    0.0,
    -7.324_225e-2,
    0.0,
    1.961_995_9e-2,
    0.0,
    -1.728_078_9e-3,
    0.0,
];

const HE_OUTPUT_RATES: [u32; 6] = [16_000, 22_050, 24_000, 32_000, 44_100, 48_000];

/// Core rate for dual-rate HE, or `UnsupportedSampleRateIndex` if the
/// output rate is not in the v1 set / not 2× an ADTS table rate.
pub(crate) fn he_core_rate(output_hz: u32) -> Result<u32> {
    if !HE_OUTPUT_RATES.contains(&output_hz) {
        return Err(Error::UnsupportedSampleRateIndex(0xFF));
    }
    let core = output_hz / 2;
    if !ADTS_SAMPLE_RATES_HZ.contains(&core) {
        return Err(Error::UnsupportedSampleRateIndex(0xFF));
    }
    Ok(core)
}

/// Running 2:1 halfband. Emits one core sample every two output samples.
struct Halfband {
    z: [f32; 16],
    /// Number of output samples seen (parity selects the kept phase).
    n: u64,
}

impl Halfband {
    fn new() -> Self {
        Self { z: [0.0; 16], n: 0 }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn push(&mut self, x: f32) -> Option<f32> {
        let mut acc = HALF_BAND[0] * x;
        for i in 0..16 {
            acc += HALF_BAND[i + 1] * self.z[i];
        }
        for i in (1..16).rev() {
            self.z[i] = self.z[i - 1];
        }
        self.z[0] = x;
        let keep = self.n.is_multiple_of(2);
        self.n += 1;
        if keep { Some(acc) } else { None }
    }
}

/// Incremental source/core/slot counters (exact, not estimated).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrepCounts {
    pub source: u64,
    pub core: u64,
    pub slots: u64,
}

/// Per-channel HE analysis + core downsample.
pub(crate) struct SbrPrep {
    fir: Halfband,
    qmf: EncAnalysisQmf,
    qmf_buf: [f32; QMF_SLOT],
    qmf_n: usize,
    counts: PrepCounts,
}

impl SbrPrep {
    pub(crate) fn new() -> Self {
        Self {
            fir: Halfband::new(),
            qmf: EncAnalysisQmf::new(),
            qmf_buf: [0.0; QMF_SLOT],
            qmf_n: 0,
            counts: PrepCounts::default(),
        }
    }

    /// Drop signal; keep QMF table intern and array storage.
    pub(crate) fn reset(&mut self) {
        self.fir.reset();
        self.qmf.reset();
        self.qmf_buf = [0.0; QMF_SLOT];
        self.qmf_n = 0;
        self.counts = PrepCounts::default();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn counts(&self) -> PrepCounts {
        self.counts
    }

    /// Consume output-rate PCM. Core samples append to `core`; each
    /// complete 64-sample slot is delivered to `on_slot`.
    pub(crate) fn push<F>(&mut self, pcm: &[f32], core: &mut Vec<f32>, mut on_slot: F) -> Result<()>
    where
        F: FnMut(&EncSlot),
    {
        for &x in pcm {
            self.counts.source += 1;
            if let Some(c) = self.fir.push(x) {
                core.push(c);
                self.counts.core += 1;
            }
            self.qmf_buf[self.qmf_n] = x;
            self.qmf_n += 1;
            if self.qmf_n == QMF_SLOT {
                let slot = self.qmf.push_slot(&self.qmf_buf)?;
                on_slot(&slot);
                self.qmf_n = 0;
                self.counts.slots += 1;
            }
        }
        Ok(())
    }

    /// Pad FIR delay and QMF history with zeros. After this, `core` and
    /// slot deliveries include the delayed tail of `pcm`.
    pub(crate) fn finish<F>(&mut self, core: &mut Vec<f32>, mut on_slot: F) -> Result<PrepCounts>
    where
        F: FnMut(&EncSlot),
    {
        let pad = FIR_DELAY + QMF_DRAIN;
        let zeros = [0.0f32; 64];
        let mut left = pad;
        while left > 0 {
            let n = left.min(zeros.len());
            self.push(&zeros[..n], core, &mut on_slot)?;
            left -= n;
        }
        Ok(self.counts)
    }
}

impl Default for SbrPrep {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "enc_sbr_prep_tests.rs"]
mod enc_sbr_prep_tests;
