//! Noise-driven precision targets for the LC rate loop (TASK-113).
//!
//! Each coded band gets the quantized-peak magnitude `target_q` at which
//! its quantization noise meets the allowed noise `T` (the psy model's
//! masked threshold with floors). For step `G` in the `|x|^0.75` domain
//! the linear noise of one coefficient is `≈ 4·√|x| / (27·G²)`, so a band
//! needs `G² = 4·Σ√|x| / (27·T)` and `target_q = peak^0.75 · G`.
//!
//! The rate loop's `offset` moves the allowed noise (0.75 dB of energy per step):
//! `offset ≤ 0` lowers every band's `T` uniformly (spare bits refine all
//! bands, equal noise-to-mask); `offset > 0` raises a water level of
//! allowed noise **per coefficient** from the quietest coded band —
//! bands under water get coarser first and drop out once the water
//! reaches their energy, so a bit shortage costs quiet noise-like bands
//! before loud ones (band limiting falls out of this) instead of
//! degrading every band alike.

use super::det_math;
use super::enc_quant::MAX_BANDS;

/// Per-channel, per-frame allocation inputs for the long-window rate
/// loop, computed once per frame (`LcEncoder::prepare_alloc` /
/// `finish_alloc`) and reused by every `build` offset.
#[derive(Clone)]
pub struct AllocCache {
    pub coded: [bool; MAX_BANDS],
    /// Psy precision cap per band (`TARGET_Q`).
    pub cap: [f32; MAX_BANDS],
    /// Allowed noise per band (masked threshold with floors; M/S bands
    /// take the smaller of L and R; TNS scales by the residual ratio).
    pub noise: [f32; MAX_BANDS],
    /// Energy, `Σ √|x|` and peak of the spectrum being quantized.
    pub energy: [f32; MAX_BANDS],
    pub sum_sqrt: [f32; MAX_BANDS],
    pub peaks: [f32; MAX_BANDS],
    pub width: [f32; MAX_BANDS],
    peak34: [f32; MAX_BANDS],
    peak_sf_base: [f32; MAX_BANDS],
    safe_sf: [i32; MAX_BANDS],
    min_noise_per_bin: f32,
    reuse: super::enc_quant::BandReuse,
}

impl AllocCache {
    pub const fn new() -> Self {
        Self {
            coded: [false; MAX_BANDS],
            cap: [0.0; MAX_BANDS],
            noise: [0.0; MAX_BANDS],
            energy: [0.0; MAX_BANDS],
            sum_sqrt: [0.0; MAX_BANDS],
            peaks: [0.0; MAX_BANDS],
            width: [0.0; MAX_BANDS],
            peak34: [0.0; MAX_BANDS],
            peak_sf_base: [0.0; MAX_BANDS],
            safe_sf: [0; MAX_BANDS],
            min_noise_per_bin: f32::INFINITY,
            reuse: super::enc_quant::BandReuse::new(),
        }
    }

    /// Finish invariant math after the coded spectrum and allowed noise are
    /// final. Rebuilt on every long frame, including reset and MC paths.
    pub fn finish_invariants(&mut self, n: usize) {
        self.reuse.reset();
        self.min_noise_per_bin = (0..n)
            .filter(|&b| self.coded[b] && self.noise[b] > 0.0 && self.width[b] > 0.0)
            .map(|b| self.noise[b] / self.width[b])
            .fold(f32::INFINITY, f32::min);
        for b in 0..n {
            let peak = self.peaks[b];
            self.peak34[b] = det_math::pow_three_quarter(peak);
            self.peak_sf_base[b] = if peak <= 0.0 {
                0.0
            } else {
                super::spectrum::SF_OFFSET as f32 + 4.0 * det_math::log2(peak)
            };
            self.safe_sf[b] = super::enc_quant::sf_for_peak(peak, super::enc_quant::QUANT_SAFE);
        }
    }

    /// [`noise_targets`] for an unmodified copy of this frame's coded mask.
    pub fn targets(&self, offset: i32, n: usize, coded: &mut [bool], target_q: &mut [f32]) {
        debug_assert_eq!(coded[..n], self.coded[..n]);
        let scale = det_math::exp2(offset as f32 * 0.25);
        let water = if offset > 0 && self.min_noise_per_bin.is_finite() {
            self.min_noise_per_bin * scale
        } else {
            0.0
        };
        for b in 0..n {
            if !coded[b] {
                target_q[b] = 0.0;
                continue;
            }
            let t = if offset > 0 {
                self.noise[b].max(water * self.width[b])
            } else {
                self.noise[b] * scale
            };
            if t >= self.energy[b] || self.peaks[b] <= 0.0 {
                coded[b] = false;
                target_q[b] = 0.0;
                continue;
            }
            let g2 = 4.0 * self.sum_sqrt[b] / (27.0 * t.max(1e-30));
            let tq = self.peak34[b] * g2.sqrt();
            target_q[b] = tq.clamp(1.0, self.cap[b].max(1.0));
        }
    }

    /// Original raw-scalefactor arithmetic with its peak-only subexpression
    /// cached; the add, subtract, round and max order remains unchanged.
    pub fn raw_scalefactors(
        &self,
        target_q: &[f32; MAX_BANDS],
        q: &mut super::enc_quant::QuantChannel,
    ) {
        for (b, &tq) in target_q.iter().enumerate().take(q.n_bands) {
            q.sf[b] = if q.coded[b] && tq >= 1.0 && self.peaks[b] > 0.0 {
                let sf = self.peak_sf_base[b] - (16.0 / 3.0) * det_math::log2(tq);
                (sf.round() as i32).max(self.safe_sf[b])
            } else {
                0
            };
        }
    }

    /// Reuse only within this frame; normalized sf determines quantized bins.
    pub fn quantize(
        &mut self,
        spec: &[f32; 1024],
        offsets: &[u16],
        q: &mut super::enc_quant::QuantChannel,
    ) {
        self.reuse.quantize(spec, offsets, q);
    }
    pub fn invalidate_band(&mut self, band: usize) {
        self.reuse.invalidate(band);
    }
}

#[cfg(test)]
#[path = "enc_alloc_tests.rs"]
mod tests;

/// Fill `target_q` (and clear `coded` for bands that drop out) for `n`
/// bands at rate-loop position `offset`; `cap` is the psy's per-band
/// precision cap, `width` the coefficient count per band. All arrays are
/// band-indexed (long bands, or flattened short groups).
#[allow(clippy::too_many_arguments)]
pub fn noise_targets(
    peaks: &[f32],
    energy: &[f32],
    sum_sqrt: &[f32],
    noise: &[f32],
    width: &[f32],
    cap: &[f32],
    offset: i32,
    n: usize,
    coded: &mut [bool],
    target_q: &mut [f32],
) {
    let scale = det_math::exp2(offset as f32 * 0.25);
    let water = if offset > 0 {
        let t_min = (0..n)
            .filter(|&b| coded[b] && noise[b] > 0.0 && width[b] > 0.0)
            .map(|b| noise[b] / width[b])
            .fold(f32::INFINITY, f32::min);
        if t_min.is_finite() {
            t_min * scale
        } else {
            0.0
        }
    } else {
        0.0
    };
    for b in 0..n {
        if !coded[b] {
            target_q[b] = 0.0;
            continue;
        }
        let t = if offset > 0 {
            noise[b].max(water * width[b])
        } else {
            noise[b] * scale
        };
        if t >= energy[b] || peaks[b] <= 0.0 {
            coded[b] = false;
            target_q[b] = 0.0;
            continue;
        }
        let g2 = 4.0 * sum_sqrt[b] / (27.0 * t.max(1e-30));
        let tq = det_math::pow_three_quarter(peaks[b]) * g2.sqrt();
        target_q[b] = tq.clamp(1.0, cap[b].max(1.0));
    }
}
