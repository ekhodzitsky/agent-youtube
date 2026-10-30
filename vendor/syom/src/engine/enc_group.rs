//! Short-window `scale_factor_grouping` (ISO/IEC 14496-3 Table 4.6).
//!
//! Default is eight groups of one window (`bits = 0`). Opt-in
//! [`Grouping::decide`] merges consecutive windows whose energy is within
//! 6 dB (or both below a floor) so they share scale factors / sections.
//! CPE uses the per-window max across channels (common_window).

use super::bits::BitWriter;
use super::enc_huff::spectral_emit;
use super::enc_quant::{MAX_FLAT_SHORT, QuantShort};
use super::ics::{WindowSequence, grouping_of};
use super::section::has_spectral;
use super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};

/// Merge when the louder window is at most 4× the quieter (≈ 6 dB).
const MERGE_RATIO: f32 = 4.0;
const ENERGY_FLOOR: f32 = 1e-6;

/// One short-frame grouping: `group_len[..n_groups]` sums to 8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grouping {
    pub n_groups: u8,
    pub group_len: [u8; 8],
}

impl Grouping {
    /// Eight groups of one window (`scale_factor_grouping = 0`).
    #[must_use]
    pub fn ungrouped() -> Self {
        Self {
            n_groups: 8,
            group_len: [1; 8],
        }
    }

    /// Decode the 7-bit ICS field with the same rules as [`grouping_of`].
    #[must_use]
    pub fn from_bits(gbits: u8) -> Self {
        let (_, n, lens) = grouping_of(WindowSequence::EightShort, Some(gbits));
        Self {
            n_groups: n,
            group_len: lens,
        }
    }

    /// Inverse of [`Self::from_bits`]: bit 6 groups window 1 with 0, …
    #[must_use]
    pub fn bits(self) -> u8 {
        let mut bits = 0u8;
        let mut w = 0u8;
        for g in 0..self.n_groups as usize {
            let len = self.group_len[g];
            for k in 1..len {
                let i = w + k; // window index 1..=7
                bits |= 1 << (7 - i);
            }
            w += len;
        }
        bits
    }

    /// Merge consecutive short windows with similar energy.
    ///
    /// `specs[..n_ch]` are window-major 8×128 MDCT spectra. Stereo uses
    /// the louder channel per window so common_window grouping is shared.
    #[must_use]
    pub fn decide(specs: &[[f32; LONG_WINDOW_LEN]], n_ch: usize, _offsets: &[u16]) -> Self {
        let mut e = [0.0f32; 8];
        for spec in specs.iter().take(n_ch) {
            for (w, slot) in e.iter_mut().enumerate() {
                let lo = w * SHORT_WINDOW_LEN;
                let hi = lo + SHORT_WINDOW_LEN;
                let mut s = 0.0f32;
                for &x in &spec[lo..hi] {
                    s += x * x;
                }
                if s > *slot {
                    *slot = s;
                }
            }
        }
        let mut bits = 0u8;
        for i in 1..8 {
            let mx = e[i - 1].max(e[i]);
            let mn = e[i - 1].min(e[i]);
            if mx <= ENERGY_FLOOR || mx <= MERGE_RATIO * mn {
                bits |= 1 << (7 - i);
            }
        }
        Self::from_bits(bits)
    }
}

/// Fold per-window coded/target_q/peaks into group-major slots
/// (`g * n_sfb + b`). Each group band takes the loudest window's `target_q`
/// and the max peak.
#[allow(clippy::too_many_arguments)]
pub fn fold_windows(
    grouping: Grouping,
    n_sfb: usize,
    win_coded: &[bool],
    win_tq: &[f32],
    win_peak: &[f32],
    out_coded: &mut [bool],
    out_tq: &mut [f32],
    out_peak: &mut [f32],
) {
    let mut wbase = 0usize;
    for g in 0..grouping.n_groups as usize {
        let glen = grouping.group_len[g] as usize;
        for b in 0..n_sfb {
            let mut coded = false;
            let mut best_peak = -1.0f32;
            let mut best_tq = 0.0f32;
            let mut gpeak = 0.0f32;
            for k in 0..glen {
                let idx = (wbase + k) * n_sfb + b;
                gpeak = gpeak.max(win_peak[idx]);
                if win_coded[idx] {
                    coded = true;
                    if win_peak[idx] >= best_peak {
                        best_peak = win_peak[idx];
                        best_tq = win_tq[idx];
                    }
                }
            }
            let o = g * n_sfb + b;
            out_coded[o] = coded;
            out_tq[o] = best_tq;
            out_peak[o] = gpeak;
        }
        wbase += glen;
    }
}

/// Huffman spectral data: per group, each sfb concatenates `group_len` windows.
pub fn emit_spectral_short(
    w: &mut BitWriter,
    offsets: &[u16],
    sfb_cb: &[u8; MAX_FLAT_SHORT],
    q: &QuantShort,
) {
    let mut wbase = 0usize;
    for g in 0..q.n_groups {
        let glen = q.group_len[g] as usize;
        for b in 0..q.n_sfb {
            let cb = sfb_cb[g * q.n_sfb + b];
            if !has_spectral(cb) {
                continue;
            }
            let start = usize::from(offsets[b]);
            let end = usize::from(offsets[b + 1]);
            let step = if cb <= 4 { 4 } else { 2 };
            let mut tmp = [0i32; LONG_WINDOW_LEN];
            let mut n = 0usize;
            for k in 0..glen {
                let lo = (wbase + k) * SHORT_WINDOW_LEN + start;
                let hi = (wbase + k) * SHORT_WINDOW_LEN + end;
                tmp[n..n + (hi - lo)].copy_from_slice(&q.quant[lo..hi]);
                n += hi - lo;
            }
            let mut i = 0usize;
            while i < n {
                spectral_emit(cb, &tmp[i..i + step], w);
                i += step;
            }
        }
        wbase += glen;
    }
}

#[cfg(test)]
#[path = "enc_group_tests.rs"]
mod enc_group_tests;
