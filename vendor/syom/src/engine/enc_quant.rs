//! Forward quantization, per-band scalefactors, and codebook cost table —
//! the inverse of [`super::spectrum::invquant`] / [`super::spectrum::sf_gain`]
//! (ISO/IEC 14496-3 §4.6.2–4.6.3).

use super::det_math;
use super::spectrum::SF_OFFSET;
use super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};

/// Largest quantized magnitude the bitstream carries (§4.6.2).
pub const QUANT_MAX: i32 = 8191;
/// Spectral books 1..=11; slot 0 is unused.
pub const BOOKS: usize = 12;
/// Long-window band ceiling (the 32 kHz table has 51 bands).
pub const MAX_BANDS: usize = 51;
/// `bits` sentinel: book cannot represent the band.
pub const UNREPRESENTABLE: u32 = u32::MAX;
/// ISO/IEC 14496-3 §4.6.2 quantizer rounding constant: `floor(v + 0.4054)`
/// rounds toward zero more than `round`, so coarsely quantized noise-like
/// bands do not gain energy (TASK-113).
pub const QUANT_ROUND: f32 = 0.4054;
/// Finest scalefactor floor: the band peak quantizes to at most this, so
/// the rounded `sf_for_peak` never lands on the `QUANT_MAX` clip plateau.
pub const QUANT_SAFE: f32 = 7600.0;

/// `floor(pow34 * gain + QUANT_ROUND)` via trunc when that sum is ≥ 0.
#[inline]
fn quant_mag(pow34: f32, gain: f32) -> i32 {
    let v = pow34 * gain + QUANT_ROUND;
    if v >= 0.0 {
        (v as i32).min(QUANT_MAX)
    } else {
        (v.floor() as i32).min(QUANT_MAX)
    }
}

/// Fill `mag` with `|x|^0.75` (det_math: two exactly-rounded sqrts).
pub fn cache_mags(spec: &[f32; 1024], mag: &mut [f32; 1024]) {
    for (m, &x) in mag.iter_mut().zip(spec.iter()) {
        *m = det_math::pow_three_quarter(x);
    }
}

/// One quantized long-window channel.
#[derive(Clone)]
pub struct QuantChannel {
    pub quant: [i32; 1024],
    /// `|x|^0.75` of the frame's spectrum ([`QuantChannel::cache_mags`]):
    /// fixed across the rate loop's re-quantizations (TASK-83).
    pub mag: [f32; 1024],
    /// `bits[band][book]` — spectral bits under each book, or
    /// [`UNREPRESENTABLE`].
    pub bits: [[u32; BOOKS]; MAX_BANDS],
    /// Absolute scalefactors actually quantized with (the transmitted ones).
    pub sf: [i32; MAX_BANDS],
    /// Bands with signal worth coding (the rest get ZERO_HCB).
    pub coded: [bool; MAX_BANDS],
    /// Coded bands whose quantized values all came out zero (TASK-113):
    /// `plan_books` gives them ZERO_HCB when the sf chain allows.
    pub zero: [bool; MAX_BANDS],
    /// PNS substitution (TASK-75). `plan_books` stamps `NOISE_HCB`.
    pub pns: [bool; MAX_BANDS],
    /// Absolute `noise_nrg` for PNS bands (independent of `sf` DPCM).
    pub noise_nrg: [i32; MAX_BANDS],
    /// Intensity stereo on the right channel (TASK-76).
    pub intensity: [bool; MAX_BANDS],
    /// Absolute `is_pos` (Table 4.53).
    pub is_pos: [i32; MAX_BANDS],
    /// `INTENSITY_HCB` or `INTENSITY_HCB2`.
    pub is_hcb: [u8; MAX_BANDS],
    pub n_bands: usize,
}

impl QuantChannel {
    pub fn new(n_bands: usize) -> Self {
        Self {
            quant: [0; 1024],
            mag: [0.0; 1024],
            bits: [[UNREPRESENTABLE; BOOKS]; MAX_BANDS],
            sf: [0; MAX_BANDS],
            coded: [false; MAX_BANDS],
            zero: [false; MAX_BANDS],
            pns: [false; MAX_BANDS],
            noise_nrg: [0; MAX_BANDS],
            intensity: [false; MAX_BANDS],
            is_pos: [0; MAX_BANDS],
            is_hcb: [0; MAX_BANDS],
            n_bands,
        }
    }
}

/// Short-window band ceiling (the 8 kHz table has 15 bands).
pub const MAX_SFB_SHORT: usize = 15;
/// Window groups in a short frame (8 windows; grouping may merge them).
pub const MAX_GROUPS: usize = 8;
/// Flattened (group, band) ceiling: index `g * n_sfb + b`.
pub const MAX_FLAT_SHORT: usize = MAX_GROUPS * MAX_SFB_SHORT;

/// One quantized short-window channel: 8 windows × 128 bins, window-major,
/// with books / scalefactors per (group, band) flattened as `g * n_sfb + b`.
/// Default grouping is 8×1 (`scale_factor_grouping = 0`).
#[derive(Clone)]
pub struct QuantShort {
    pub quant: [i32; LONG_WINDOW_LEN],
    /// `|x|^0.75` cache, as in [`QuantChannel`].
    pub mag: [f32; LONG_WINDOW_LEN],
    /// `bits[g * n_sfb + band][book]`.
    pub bits: [[u32; BOOKS]; MAX_FLAT_SHORT],
    /// Absolute scalefactors actually quantized with (the transmitted ones).
    pub sf: [i32; MAX_FLAT_SHORT],
    /// Bands with signal worth coding (the rest get ZERO_HCB).
    pub coded: [bool; MAX_FLAT_SHORT],
    /// All-zero coded (group, band)s (TASK-113), as in `QuantChannel`.
    pub zero: [bool; MAX_FLAT_SHORT],
    /// Scalefactor bands per group (the rate's short-table size).
    pub n_sfb: usize,
    pub n_groups: usize,
    pub group_len: [u8; 8],
}

impl QuantShort {
    pub fn new(n_sfb: usize) -> Self {
        Self {
            quant: [0; LONG_WINDOW_LEN],
            mag: [0.0; LONG_WINDOW_LEN],
            bits: [[UNREPRESENTABLE; BOOKS]; MAX_FLAT_SHORT],
            sf: [0; MAX_FLAT_SHORT],
            coded: [false; MAX_FLAT_SHORT],
            zero: [false; MAX_FLAT_SHORT],
            n_sfb,
            n_groups: MAX_GROUPS,
            group_len: [1; 8],
        }
    }

    pub fn set_grouping(&mut self, g: super::enc_group::Grouping) {
        self.n_groups = g.n_groups as usize;
        self.group_len = g.group_len;
    }

    #[must_use]
    pub fn grouping_bits(&self) -> u8 {
        super::enc_group::Grouping {
            n_groups: self.n_groups as u8,
            group_len: self.group_len,
        }
        .bits()
    }
}

/// Per-band peak |coefficient| of a 1024-line long-window spectrum.
pub fn band_peaks(spec: &[f32; 1024], offsets: &[u16], out: &mut [f32; MAX_BANDS]) {
    let n_bands = offsets.len() - 1;
    for b in 0..n_bands {
        let lo = usize::from(offsets[b]);
        let hi = usize::from(offsets[b + 1]);
        out[b] = spec[lo..hi].iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    }
}

/// Desired scalefactor for a band peak so the peak quantizes to about
/// `target_q`: `q = |x|^0.75 · 2^(−0.1875·(sf−100))` solved for `sf`. The
/// log2s are [`det_math`](super::det_math)'s: libm `log2` is not bit-identical
/// across platforms, and a 1-ulp drift can flip the rounded scalefactor.
pub fn sf_for_peak(peak: f32, target_q: f32) -> i32 {
    if peak <= 0.0 || target_q <= 0.0 {
        return 0;
    }
    let sf =
        SF_OFFSET as f32 + 4.0 * det_math::log2(peak) - (16.0 / 3.0) * det_math::log2(target_q);
    sf.round() as i32
}

/// Raw per-band scalefactors from peaks and the psy model's per-band
/// precision targets. `out.coded` is set by the psy model (a coded band
/// always has `target_q >= 1` and a nonzero peak).
pub fn raw_scalefactors(
    peaks: &[f32; MAX_BANDS],
    target_q: &[f32; MAX_BANDS],
    global_offset: i32,
    out: &mut QuantChannel,
) {
    let bands = peaks
        .iter()
        .zip(target_q.iter())
        .zip(out.coded.iter())
        .zip(out.sf.iter_mut())
        .take(out.n_bands);
    for (((&peak, &tq), &coded), sf) in bands {
        *sf = if coded && tq >= 1.0 {
            (sf_for_peak(peak, tq) + global_offset).max(sf_for_peak(peak, QUANT_SAFE))
        } else {
            0
        };
    }
}

/// Clamp `out.sf` onto the wire format: global_gain is 8-bit, DPCM deltas
/// are ±60. Returns `global_gain`. Values actually transmitted are written
/// back into `out.sf` so quantization matches what the decoder applies.
pub fn normalize_sf(out: &mut QuantChannel) -> u8 {
    let mut global_gain = 100u8;
    let mut prev: Option<i32> = None;
    let bands = out.coded.iter().zip(out.sf.iter_mut()).take(out.n_bands);
    for (&coded, sf) in bands {
        if !coded {
            continue;
        }
        let want = (*sf).clamp(0, 255);
        let v = match prev {
            None => {
                global_gain = want as u8;
                want
            }
            Some(p) => (p + (want - p).clamp(-60, 60)).clamp(0, 255),
        };
        *sf = v;
        prev = Some(v);
    }
    global_gain
}

/// Quantize `spec` with the (normalized) `out.sf` and fill the per-band,
/// per-book bit-cost table.
#[cfg(test)]
pub fn quantize(spec: &[f32; 1024], offsets: &[u16], out: &mut QuantChannel) {
    cache_mags(spec, &mut out.mag);
    quantize_cached(spec, offsets, out);
}

/// [`quantize`] with `out.mag` already holding this spectrum's
/// [`cache_mags`] (the rate loop re-quantizes one spectrum many times).
pub fn quantize_cached(spec: &[f32; 1024], offsets: &[u16], out: &mut QuantChannel) {
    out.quant = [0; 1024];
    let n_bands = out.n_bands.min(offsets.len() - 1);
    let bands = out
        .coded
        .iter()
        .zip(out.sf.iter())
        .zip(out.bits.iter_mut().zip(out.zero.iter_mut()))
        .zip(offsets.windows(2))
        .take(n_bands);
    for (((&coded, &sf), (bits, zero)), w) in bands {
        let (lo, hi) = (usize::from(w[0]), usize::from(w[1]));
        *zero = false;
        if !coded {
            *bits = [UNREPRESENTABLE; BOOKS];
            continue;
        }
        // det_math gain: libm exp2 is not bit-identical across platforms.
        let gain = quant_gain(sf);
        bits::write_quants(
            &mut out.quant[lo..hi],
            &spec[lo..hi],
            &out.mag[lo..hi],
            gain,
        );
        *zero = out.quant[lo..hi].iter().all(|&v| v == 0);
        fill_bits(bits, &out.quant[lo..hi]);
    }
}

/// Re-quantize one long-window band after an `sf` tweak.
pub fn requant_band(spec: &[f32; 1024], offsets: &[u16], out: &mut QuantChannel, b: usize) {
    if b >= out.n_bands || !out.coded[b] {
        return;
    }
    let lo = usize::from(offsets[b]);
    let hi = usize::from(offsets[b + 1]);
    let gain = quant_gain(out.sf[b]);
    for (q, &x) in out.quant[lo..hi].iter_mut().zip(spec[lo..hi].iter()) {
        let mag = quant_mag(det_math::pow_three_quarter(x), gain);
        *q = if x < 0.0 { -mag } else { mag };
    }
    out.zero[b] = out.quant[lo..hi].iter().all(|&v| v == 0);
    fill_bits(&mut out.bits[b], &out.quant[lo..hi]);
}

/// Per-(window, band) peak |coefficient| of a window-major short spectrum,
/// flattened as `w * n_sfb + band`.
pub fn band_peaks_short(
    spec: &[f32; LONG_WINDOW_LEN],
    offsets: &[u16],
    n_sfb: usize,
    out: &mut [f32; MAX_FLAT_SHORT],
) {
    for w in 0..MAX_GROUPS {
        for b in 0..n_sfb {
            let lo = w * SHORT_WINDOW_LEN + usize::from(offsets[b]);
            let hi = w * SHORT_WINDOW_LEN + usize::from(offsets[b + 1]);
            out[w * n_sfb + b] = spec[lo..hi].iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        }
    }
}

/// Raw per-(window, band) scalefactors, short-window twin of
/// [`raw_scalefactors`] (`out.coded` is set by the psy model).
pub fn raw_scalefactors_short(
    peaks: &[f32; MAX_FLAT_SHORT],
    target_q: &[f32; MAX_FLAT_SHORT],
    global_offset: i32,
    out: &mut QuantShort,
) {
    let n = out.n_groups * out.n_sfb;
    let bands = peaks
        .iter()
        .zip(target_q.iter())
        .zip(out.coded.iter())
        .zip(out.sf.iter_mut())
        .take(n);
    for (((&peak, &tq), &coded), sf) in bands {
        *sf = if coded && tq >= 1.0 {
            (sf_for_peak(peak, tq) + global_offset).max(sf_for_peak(peak, QUANT_SAFE))
        } else {
            0
        };
    }
}

/// Clamp `out.sf` onto the wire format, short windows: the DPCM chain runs
/// over the flattened (group, band) order — the decoder's `last_sf`
/// continues across groups (ISO/IEC 14496-3 Table 4.53).
pub fn normalize_sf_short(out: &mut QuantShort) -> u8 {
    let mut global_gain = 100u8;
    let mut prev: Option<i32> = None;
    let n = out.n_groups * out.n_sfb;
    let bands = out.coded.iter().zip(out.sf.iter_mut()).take(n);
    for (&coded, sf) in bands {
        if !coded {
            continue;
        }
        let want = (*sf).clamp(0, 255);
        let v = match prev {
            None => {
                global_gain = want as u8;
                want
            }
            Some(p) => (p + (want - p).clamp(-60, 60)).clamp(0, 255),
        };
        *sf = v;
        prev = Some(v);
    }
    global_gain
}

/// Quantize a window-major short spectrum with the (normalized) `out.sf`
/// and fill the per-(group, band) bit-cost table. Grouped bands concatenate
/// windows in decoder order before Huffman costing.
#[cfg(test)]
pub fn quantize_short(spec: &[f32; LONG_WINDOW_LEN], offsets: &[u16], out: &mut QuantShort) {
    cache_mags(spec, &mut out.mag);
    quantize_short_cached(spec, offsets, out);
}

/// [`quantize_short`] with `out.mag` already cached for this spectrum.
pub fn quantize_short_cached(spec: &[f32; LONG_WINDOW_LEN], offsets: &[u16], out: &mut QuantShort) {
    out.quant = [0; LONG_WINDOW_LEN];
    let mut wbase = 0usize;
    for g in 0..out.n_groups {
        let glen = out.group_len[g] as usize;
        for b in 0..out.n_sfb {
            let idx = g * out.n_sfb + b;
            let start = usize::from(offsets[b]);
            let end = usize::from(offsets[b + 1]);
            out.zero[idx] = false;
            if !out.coded[idx] {
                out.bits[idx] = [UNREPRESENTABLE; BOOKS];
                continue;
            }
            let gain = quant_gain(out.sf[idx]);
            let mut tmp = [0i32; LONG_WINDOW_LEN];
            let mut n = 0usize;
            for k in 0..glen {
                let w = wbase + k;
                let lo = w * SHORT_WINDOW_LEN + start;
                let hi = w * SHORT_WINDOW_LEN + end;
                let src = spec[lo..hi].iter().zip(out.mag[lo..hi].iter());
                for (q, (&x, &m)) in out.quant[lo..hi].iter_mut().zip(src) {
                    let mag = quant_mag(m, gain);
                    *q = if x < 0.0 { -mag } else { mag };
                }
                tmp[n..n + (hi - lo)].copy_from_slice(&out.quant[lo..hi]);
                n += hi - lo;
            }
            out.zero[idx] = tmp[..n].iter().all(|&v| v == 0);
            fill_bits(&mut out.bits[idx], &tmp[..n]);
        }
        wbase += glen;
    }
}

#[path = "enc_quant_bits.rs"]
mod bits;
pub use bits::dpcm_ok;
use bits::{fill_bits, quant_gain};

#[cfg(test)]
#[path = "enc_quant_tests.rs"]
mod enc_quant_tests;
