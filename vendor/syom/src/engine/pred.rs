//! AAC Main backward-adaptive prediction — ISO/IEC 14496-3 §4.6.6.
//!
//! The predictor runs on inverse-quantised spectral lines. Each bin keeps a
//! second-order state. Quantities that the standard keeps at 16-bit float
//! precision are rounded the same way as a float AAC decoder: round, even,
//! and trunc on the top 16 bits of the IEEE value. A short window resets
//! every bin. `predictor_reset_group` clears one residue class after the
//! update, so this frame still uses the old state.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::ics::IcsInfo;

/// Bins that ever carry a predictor (highest `swb_offset[pred_sfb_max]`).
const MAX_PREDICTORS: usize = 672;
/// `pred_sfb_max` for samplingFrequencyIndex 0..=12.
const PRED_SFB_MAX: [u8; 13] = [33, 33, 38, 40, 40, 40, 41, 41, 37, 37, 37, 34, 34];
const A: f32 = 0.953125; // 61/64
const ALPHA: f32 = 0.90625; // 29/32

#[derive(Clone, Copy, Debug)]
struct Bin {
    r0: f32,
    r1: f32,
    cor0: f32,
    cor1: f32,
    var0: f32,
    var1: f32,
}

impl Bin {
    fn reset() -> Self {
        Self {
            r0: 0.0,
            r1: 0.0,
            cor0: 0.0,
            cor1: 0.0,
            var0: 1.0,
            var1: 1.0,
        }
    }
}

/// Per-channel predictor bank. Empty until the first Main frame.
#[derive(Clone, Debug, Default)]
pub(crate) struct PredChan {
    bins: Vec<Bin>,
}

impl PredChan {
    fn ensure(&mut self) {
        if self.bins.is_empty() {
            self.bins.resize(MAX_PREDICTORS, Bin::reset());
        }
    }

    /// Add the prediction into `spec` where the frame asked for it, and
    /// advance every bin up through `pred_sfb_max` either way.
    pub(crate) fn apply(&mut self, spec: &mut [f32], ics: &IcsInfo, fs_index: u8) -> Result<()> {
        let Some(limit) = pred_sfb_max(fs_index) else {
            return Err(Error::UnsupportedSampleRateIndex(fs_index));
        };
        self.ensure();
        if ics.window_sequence.is_eight_short() {
            for bin in &mut self.bins {
                *bin = Bin::reset();
            }
            return Ok(());
        }
        let offsets = ics.swb_offsets(fs_index)?;
        let nsfb = limit.min(offsets.len().saturating_sub(1));
        for sfb in 0..nsfb {
            let lo = offsets[sfb] as usize;
            let hi = offsets[sfb + 1] as usize;
            let enable = ics.predictor_present && ((ics.pred_used >> sfb) & 1) == 1;
            for k in lo..hi {
                if k >= spec.len() || k >= self.bins.len() {
                    break;
                }
                predict(&mut self.bins[k], &mut spec[k], enable);
            }
        }
        if ics.pred_reset_group > 0 {
            let mut i = usize::from(ics.pred_reset_group) - 1;
            while i < self.bins.len() {
                self.bins[i] = Bin::reset();
                i += 30;
            }
        }
        Ok(())
    }
}

/// `predictor_reset` + `prediction_used[]` after `predictor_data_present`.
pub(crate) fn read_main(br: &mut BitReader<'_>, max_sfb: u8, fs_index: u8) -> Result<(u8, u64)> {
    let mut group = 0u8;
    if br.read_bit()? {
        group = br.read(5)? as u8;
        if group == 0 || group > 30 {
            return Err(Error::Format("predictor reset group"));
        }
    }
    let Some(limit) = pred_sfb_max(fs_index) else {
        return Err(Error::UnsupportedSampleRateIndex(fs_index));
    };
    let n = limit.min(usize::from(max_sfb));
    let mut used = 0u64;
    for sfb in 0..n {
        if br.read_bit()? {
            used |= 1u64 << sfb;
        }
    }
    Ok((group, used))
}

pub(crate) fn pred_sfb_max(fs_index: u8) -> Option<usize> {
    PRED_SFB_MAX
        .get(usize::from(fs_index))
        .map(|n| usize::from(*n))
}

fn flt16_round(pf: f32) -> f32 {
    let bits = pf.to_bits().wrapping_add(0x0000_8000) & 0xFFFF_0000;
    f32::from_bits(bits)
}

fn flt16_even(pf: f32) -> f32 {
    let bits = pf.to_bits();
    // Same association as the float decoder: `>>` binds tighter than `&`.
    let tie = bits & (0x0001_0000 >> 16);
    f32::from_bits(bits.wrapping_add(0x0000_7FFF).wrapping_add(tie) & 0xFFFF_0000)
}

fn flt16_trunc(pf: f32) -> f32 {
    f32::from_bits(pf.to_bits() & 0xFFFF_0000)
}

fn predict(ps: &mut Bin, coef: &mut f32, output_enable: bool) {
    let r0 = ps.r0;
    let r1 = ps.r1;
    let cor0 = ps.cor0;
    let cor1 = ps.cor1;
    let var0 = ps.var0;
    let var1 = ps.var1;
    let k1 = if var0 > 1.0 {
        cor0 * flt16_even(A / var0)
    } else {
        0.0
    };
    let k2 = if var1 > 1.0 {
        cor1 * flt16_even(A / var1)
    } else {
        0.0
    };
    let pv = flt16_round(k1 * r0 + k2 * r1);
    if output_enable {
        *coef += pv;
    }
    let e0 = *coef;
    let e1 = e0 - k1 * r0;
    ps.cor1 = flt16_trunc(ALPHA * cor1 + r1 * e1);
    ps.var1 = flt16_trunc(ALPHA * var1 + 0.5 * (r1 * r1 + e1 * e1));
    ps.cor0 = flt16_trunc(ALPHA * cor0 + r0 * e0);
    ps.var0 = flt16_trunc(ALPHA * var0 + 0.5 * (r0 * r0 + e0 * e0));
    ps.r1 = flt16_trunc(A * (r0 - k1 * e0));
    ps.r0 = flt16_trunc(A * e0);
}
