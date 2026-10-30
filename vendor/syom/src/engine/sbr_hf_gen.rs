//! SBR high-frequency generation — ISO/IEC 14496-3 §4.6.18.6.
//!
//! Builds the `XHigh` subband matrix from the analysis-filterbank
//! output `XLow`:
//!
//! * **Patch construction** (§4.6.18.6.3 / Figure 4.48) — the
//!   `numPatches` / `patchStartSubband` / `patchNumSubbands` decision
//!   that maps consecutive low-band source ranges onto the SBR range,
//!   driven by `goalSb = NINT(2.048e6 / FsSBR)` and the `fMaster`
//!   grid, with the trailing small-patch trim.
//! * **Inverse filtering** (§4.6.18.6.2) — the covariance-method
//!   second-order linear prediction per low subband (`φk(i,j)` over
//!   `numTimeSlots·RATE + 6` samples, `d(k)` with `εInv = 1e-6`, the
//!   `α0(k)` / `α1(k)` solution, and the `|α| ≥ 4` reset), plus the
//!   Table 4.175 `newBw` transition function and the `bwArray` chirp
//!   blend (`0.75/0.25` attack, `0.90625/0.09375` decay, `< 0.015625`
//!   flush to zero).
//! * **HF generator** (§4.6.18.6.3) — `XHigh(k, l + tHFAdj) =
//!   XLow(p, …) + bw·α0(p)·XLow(p, l−1+…) + bw²·α1(p)·XLow(p, l−2+…)`
//!   over the patch mapping, with the chirp factor selected by the
//!   noise-floor band `g(k)`.
//!
//! Both `XLow` and `XHigh` are stored slot-major (`x[slot][band]`)
//! with the slot axis carrying the spec's absolute column index (the
//! `tHFGen`-slot history precedes the current frame, so spec index
//! `l + tHFAdj` is a direct column index).
//!
//! ## Provenance
//!
//! Every formula, constant, and branch is from the §4.6.18.6 text,
//! Table 4.175, and the Figure 4.48 flowchart of the staged spec. No
//! part of this implementation is derived from any external decoder.

use crate::engine::sbr_freq_bands::HiLoTables;
use crate::engine::sbr_qmf::Complex;
use crate::engine::{Error, Result};

/// `tHFAdj = 2` — the envelope-adjuster offset (§4.6.18.5).
pub const T_HF_ADJ: usize = 2;

/// `tHFGen = 8` — the HF-generator offset (§4.6.18.5).
pub const T_HF_GEN: usize = 8;

/// The §4.6.18.6.2 relaxation parameter `εInv`.
pub const EPS_INV: f64 = 1e-6;

/// §4.6.18.3.6: `numPatches ≤ 5`.
pub const MAX_PATCHES: usize = 5;

/// The §4.6.18.6.3 / Figure 4.48 patch layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patches {
    /// `patchStartSubband(i)` — first source QMF subband of patch `i`.
    pub start: Vec<usize>,
    /// `patchNumSubbands(i)` — subband count of patch `i`.
    pub num: Vec<usize>,
}

impl Patches {
    /// `numPatches`.
    #[cfg(test)]
    #[inline]
    #[must_use]
    pub fn num_patches(&self) -> usize {
        self.num.len()
    }

    /// The §4.6.18.3.2.3 patch borders: `patchBorders(0) = kx`,
    /// `patchBorders(k) = patchBorders(k-1) + patchNumSubbands(k-1)`.
    #[must_use]
    pub fn borders(&self, k_x: i32) -> Vec<i32> {
        let mut b = Vec::with_capacity(self.num.len() + 1);
        b.push(k_x);
        for &n in &self.num {
            b.push(b[b.len() - 1] + n as i32);
        }
        b
    }
}

/// Figure 4.48 — patch construction.
///
/// `f_master` is the §4.6.18.3.2.1 master table (`fMaster(0..=NMaster)`),
/// `k0` its first subband, `k_x` / `m` the SBR range, and `fs_sbr` the
/// SBR internal rate driving `goalSb = NINT(2.048e6 / FsSBR)`.
pub fn build_patches(f_master: &[i32], k0: i32, k_x: i32, m: i32, fs_sbr: u32) -> Result<Patches> {
    if f_master.len() < 2 || fs_sbr == 0 {
        return Err(Error::SbrFreqBandInvalid);
    }
    let n_master = f_master.len() - 1;

    let mut msb = k0;
    let mut usb = k_x;
    let mut start = Vec::new();
    let mut num = Vec::new();

    // goalSb = NINT(2.048e6 / Fs).
    let goal_sb = ((2.0 * 2.048e6 / f64::from(fs_sbr) + 1.0) / 2.0).floor() as i32;
    // k: the first master index at/after goalSb (NMaster if goalSb is
    // past the SBR stop border).
    let mut k = if goal_sb < k_x + m {
        let mut kk = 0usize;
        for (i, &f) in f_master.iter().enumerate() {
            if f < goal_sb {
                kk = i + 1;
            } else {
                break;
            }
        }
        kk
    } else {
        n_master
    };

    let mut sb;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 64 {
            return Err(Error::SbrFreqBandInvalid);
        }
        // Walk j downward from k until the patch source fits under the
        // first master subband: sb <= k0 - 1 + msb - odd.
        let mut j = k;
        let odd = loop {
            if j >= f_master.len() {
                return Err(Error::SbrFreqBandInvalid);
            }
            sb = f_master[j];
            let odd = (sb - 2 + k0).rem_euclid(2);
            if sb <= k0 - 1 + msb - odd {
                break odd;
            }
            if j == 0 {
                return Err(Error::SbrFreqBandInvalid);
            }
            j -= 1;
        };

        let n = (sb - usb).max(0);
        let s = k0 - odd - n;
        if n > 0 {
            if s < 0 || start.len() >= MAX_PATCHES {
                return Err(Error::SbrFreqBandInvalid);
            }
            start.push(s as usize);
            num.push(n as usize);
            usb = sb;
            msb = sb;
        } else {
            msb = k_x;
        }

        if f_master[k] - sb < 3 {
            k = n_master;
        }
        if sb == k_x + m {
            break;
        }
    }

    // Trailing small-patch trim: drop a final patch narrower than 3
    // subbands when more than one patch was built.
    if num.len() > 1 && num.last().is_some_and(|&n| n < 3) {
        num.pop();
        start.pop();
    }

    Ok(Patches { start, num })
}

/// Table 4.175 — `newBw(bs_invf_mode´, bs_invf_mode)`. Row is the
/// previous frame's mode, column the current one (both `0..=3` for
/// Off / Low / Intermediate / Strong).
#[must_use]
pub fn new_bw(prev_mode: u8, cur_mode: u8) -> f64 {
    const TABLE: [[f64; 4]; 4] = [
        [0.0, 0.6, 0.9, 0.98],
        [0.6, 0.75, 0.9, 0.98],
        [0.0, 0.75, 0.9, 0.98],
        [0.0, 0.75, 0.9, 0.98],
    ];
    TABLE[usize::from(prev_mode.min(3))][usize::from(cur_mode.min(3))]
}

/// §4.6.18.6.2 chirp-factor update: one `bwArray` entry per noise
/// band. `prev_invf` / `prev_bw` are the previous SBR frame's values
/// (all zero for the first frame).
#[cfg(test)]
#[must_use]
pub fn chirp_factors(cur_invf: &[u8], prev_invf: &[u8], prev_bw: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(cur_invf.len());
    chirp_factors_into(cur_invf, prev_invf, prev_bw, &mut out);
    out
}

/// Fill `out` with the §4.6.18.6.2 chirp factors (clears first; reuses
/// capacity).
pub fn chirp_factors_into(cur_invf: &[u8], prev_invf: &[u8], prev_bw: &[f64], out: &mut Vec<f64>) {
    out.clear();
    for (i, &cur) in cur_invf.iter().enumerate() {
        let prev_mode = prev_invf.get(i).copied().unwrap_or(0);
        let bw_prev = prev_bw.get(i).copied().unwrap_or(0.0);
        let nb = new_bw(prev_mode, cur);
        let temp = if nb < bw_prev {
            0.75 * nb + 0.25 * bw_prev
        } else {
            0.90625 * nb + 0.09375 * bw_prev
        };
        out.push(if temp < 0.015625 { 0.0 } else { temp });
    }
}

/// §4.6.18.6.2 covariance-method prediction coefficients
/// `(α0(k), α1(k))` for low subband `k`.
///
/// `x_low` is slot-major with the spec's absolute column index (the
/// covariance windows over `n − i + tHFAdj` for
/// `0 ≤ n < n_slots_frame + 6`), so `x_low` must carry at least
/// `n_slots_frame + 6 + tHFAdj` columns.
pub fn prediction_coefficients(
    x_low: &[[Complex; 32]],
    k: usize,
    n_slots_frame: usize,
) -> Result<(Complex, Complex)> {
    if k >= 32 || x_low.len() < n_slots_frame + 6 + T_HF_ADJ {
        return Err(Error::SbrFreqBandInvalid);
    }
    // φk(i, j) = Σ_n XLow(k, n - i + tHFAdj) · XLow*(k, n - j + tHFAdj).
    // One pass, each accumulator still summed in increasing `n`.
    let mut phi01 = Complex::default();
    let mut phi02 = Complex::default();
    let mut phi11 = Complex::default();
    let mut phi12 = Complex::default();
    let mut phi22 = Complex::default();
    for n in 0..(n_slots_frame + 6) {
        let x0 = x_low[n + T_HF_ADJ][k];
        let x1 = x_low[n + T_HF_ADJ - 1][k];
        let x2 = x_low[n + T_HF_ADJ - 2][k];
        phi01 += x0 * x1.conj();
        phi02 += x0 * x2.conj();
        phi11 += x1 * x1.conj();
        phi12 += x1 * x2.conj();
        phi22 += x2 * x2.conj();
    }
    Ok(alphas_from_phi(phi01, phi02, phi11, phi12, phi22))
}

/// `d(k)` and the magnitude reset. Same arithmetic as the scalar covariance.
fn alphas_from_phi(
    phi01: Complex,
    phi02: Complex,
    phi11: Complex,
    phi12: Complex,
    phi22: Complex,
) -> (Complex, Complex) {
    let d = phi22.re * phi11.re - phi12.norm_sqr() / (1.0 + EPS_INV);
    let alpha1 = if d != 0.0 {
        let numer = phi01 * phi12 - phi02 * phi11.re;
        Complex::new(numer.re / d, numer.im / d)
    } else {
        Complex::default()
    };
    let alpha0 = if phi11.re != 0.0 {
        let numer = phi01 + alpha1 * phi12.conj();
        Complex::new(-numer.re / phi11.re, -numer.im / phi11.re)
    } else {
        Complex::default()
    };
    if alpha0.norm_sqr() >= 16.0 || alpha1.norm_sqr() >= 16.0 {
        (Complex::default(), Complex::default())
    } else {
        (alpha0, alpha1)
    }
}

/// §4.6.18.6.3 — generate `XHigh` from `XLow` over the patch mapping.
///
/// * `x_low` — slot-major analysis output (spec absolute columns).
/// * `patches` — the Figure 4.48 layout.
/// * `bw_array` — the per-noise-band chirp factors.
/// * `bands` — the derived frequency tables (`fTableNoise`, `k_x`).
/// * `l_range` — the spec's `RATE·tE(0) .. RATE·tE(LE)` column range
///   (exclusive end, *before* the `tHFAdj` offset).
/// * `n_slots_frame` — `numTimeSlots · RATE` (covariance length).
///
/// Returns `XHigh` with the same slot-major layout and column count as
/// `x_low` (bands outside the patched range stay zero).
#[cfg(test)]
pub fn generate_hf(
    x_low: &[[Complex; 32]],
    patches: &Patches,
    bw_array: &[f64],
    bands: &HiLoTables,
    l_range: core::ops::Range<i32>,
    n_slots_frame: usize,
) -> Result<Vec<[Complex; 64]>> {
    let mut x_high = vec![[Complex::default(); 64]; x_low.len()];
    generate_hf_into(
        x_low,
        patches,
        bw_array,
        bands,
        l_range,
        n_slots_frame,
        &mut x_high,
    )?;
    Ok(x_high)
}

/// Write `XHigh` into `x_high` (zeros the used columns first). `x_high`
/// must be at least as long as `x_low`.
pub fn generate_hf_into(
    x_low: &[[Complex; 32]],
    patches: &Patches,
    bw_array: &[f64],
    bands: &HiLoTables,
    l_range: core::ops::Range<i32>,
    n_slots_frame: usize,
    x_high: &mut [[Complex; 64]],
) -> Result<()> {
    if x_high.len() < x_low.len() {
        return Err(Error::SbrFreqBandInvalid);
    }
    for col in &mut x_high[..x_low.len()] {
        col.fill(Complex::default());
    }
    let k_x = bands.k_x;

    // α cache per source subband (a subband may feed several patches).
    let mut alphas: [Option<(Complex, Complex)>; 32] = [None; 32];

    // g(k): the noise band containing QMF subband k.
    let g_of = |k: i32| -> Result<usize> {
        let nb = &bands.f_table_noise;
        for i in 0..nb.len() - 1 {
            if nb[i] <= k && k < nb[i + 1] {
                return Ok(i);
            }
        }
        Err(Error::SbrFreqBandInvalid)
    };

    let mut k_off = 0usize;
    for (&p_start, &p_num) in patches.start.iter().zip(patches.num.iter()) {
        let mut x = 0usize;
        while x < p_num {
            let k = k_x as usize + x + k_off;
            let p = p_start + x;
            if k >= 64 || p >= 32 {
                return Err(Error::SbrFreqBandInvalid);
            }
            // Four source bands share one covariance pass and one filter pass.
            // A short tail, or a band that would not fit, stays on the scalar step.
            let nband = if x + 4 <= p_num && k + 4 <= 64 && p + 4 <= 32 {
                4
            } else {
                1
            };
            if nband == 4 {
                fill_alphas4(&mut alphas, x_low, p, n_slots_frame)?;
            } else if alphas[p].is_none() {
                alphas[p] = Some(prediction_coefficients(x_low, p, n_slots_frame)?);
            }
            let mut a0b = [Complex::default(); 4];
            let mut a1b = [Complex::default(); 4];
            for i in 0..nband {
                let (a0, a1) = match alphas[p + i] {
                    Some(a) => a,
                    None => return Err(Error::SbrFreqBandInvalid),
                };
                let bw = *bw_array
                    .get(g_of((k + i) as i32)?)
                    .ok_or(Error::SbrFreqBandInvalid)?;
                let bw2 = bw * bw;
                a0b[i] = a0 * bw;
                a1b[i] = a1 * bw2;
            }
            apply_hf_bands(x_low, x_high, k, p, &a0b[..nband], &a1b[..nband], &l_range)?;
            x += nband;
        }
        k_off += p_num;
    }
    Ok(())
}

/// Resolve four uncached source bands. The vector path runs only when every
/// lane is still empty, so a band already filled by an earlier patch keeps
/// the coefficient it was given.
fn fill_alphas4(
    alphas: &mut [Option<(Complex, Complex)>; 32],
    x_low: &[[Complex; 32]],
    p: usize,
    n_slots_frame: usize,
) -> Result<()> {
    if (0..4).all(|i| alphas[p + i].is_none())
        && let Some(got) = prediction4_fast(x_low, p, n_slots_frame)
    {
        for i in 0..4 {
            alphas[p + i] = Some(got[i]);
        }
        return Ok(());
    }
    for i in 0..4 {
        if alphas[p + i].is_none() {
            alphas[p + i] = Some(prediction_coefficients(x_low, p + i, n_slots_frame)?);
        }
    }
    Ok(())
}

/// `(a0·bw)` / `(a1·bw²)` across `k..k+n` for every column in `l_range`.
/// An empty range performs no checks.
fn apply_hf_bands(
    x_low: &[[Complex; 32]],
    x_high: &mut [[Complex; 64]],
    k: usize,
    p: usize,
    a0b: &[Complex],
    a1b: &[Complex],
    l_range: &core::ops::Range<i32>,
) -> Result<()> {
    if l_range.is_empty() {
        return Ok(());
    }
    let l0 = usize::try_from(l_range.start).map_err(|_| Error::SbrFreqBandInvalid)?;
    let l1 = usize::try_from(l_range.end).map_err(|_| Error::SbrFreqBandInvalid)?;
    let c0 = l0 + T_HF_ADJ;
    let c1 = l1 + T_HF_ADJ;
    if c0 < 2 || c1 > x_low.len() {
        return Err(Error::SbrFreqBandInvalid);
    }
    let n = a0b.len();
    if n == 4 && a1b.len() == 4 && apply_hf4_fast(x_low, x_high, k, p, a0b, a1b, c0, c1) {
        return Ok(());
    }
    for i in 0..n {
        for c in c0..c1 {
            x_high[c][k + i] =
                x_low[c][p + i] + a0b[i] * x_low[c - 1][p + i] + a1b[i] * x_low[c - 2][p + i];
        }
    }
    Ok(())
}

fn prediction4_fast(
    x_low: &[[Complex; 32]],
    p: usize,
    n_slots_frame: usize,
) -> Option<[(Complex, Complex); 4]> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f")
        && p + 4 <= 32
        && x_low.len() >= n_slots_frame + 6 + T_HF_ADJ
    {
        // SAFETY: AVX-512F is probed and each load stays inside a 32-band column.
        return Some(unsafe { prediction4_avx512(x_low, p, n_slots_frame) });
    }
    let _ = (x_low, p, n_slots_frame);
    None
}

#[allow(clippy::too_many_arguments)]
fn apply_hf4_fast(
    x_low: &[[Complex; 32]],
    x_high: &mut [[Complex; 64]],
    k: usize,
    p: usize,
    a0b: &[Complex],
    a1b: &[Complex],
    c0: usize,
    c1: usize,
) -> bool {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") && k + 4 <= 64 && p + 4 <= 32 {
        // SAFETY: AVX-512F is probed. `c0 >= 2` and `c1 <= x_low.len()`, and
        // four bands sit inside both the 32-band source and the 64-band sink.
        unsafe { apply_hf4_avx512(x_low, x_high, k, p, a0b, a1b, c0, c1) };
        return true;
    }
    let _ = (x_low, x_high, k, p, a0b, a1b, c0, c1);
    false
}

/// Four complex products, AoS. `re = ar·br − ai·bi`, `im = ar·bi + ai·br`.
///
/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn cmul4(
    x: std::arch::x86_64::__m512d,
    y: std::arch::x86_64::__m512d,
) -> std::arch::x86_64::__m512d {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_mask_blend_pd, _mm512_mul_pd, _mm512_permutexvar_pd,
        _mm512_setr_epi64, _mm512_sub_pd,
    };
    {
        let swap = _mm512_setr_epi64(1, 0, 3, 2, 5, 4, 7, 6);
        let re_i = _mm512_setr_epi64(0, 0, 2, 2, 4, 4, 6, 6);
        let im_i = _mm512_setr_epi64(1, 1, 3, 3, 5, 5, 7, 7);
        let xs = _mm512_permutexvar_pd(swap, x);
        let y_re = _mm512_permutexvar_pd(re_i, y);
        let y_im = _mm512_permutexvar_pd(im_i, y);
        let re_part = _mm512_mul_pd(x, y_re);
        let im_part = _mm512_mul_pd(xs, y_im);
        let diff = _mm512_sub_pd(re_part, im_part);
        let sum = _mm512_add_pd(re_part, im_part);
        _mm512_mask_blend_pd(0b1010_1010, diff, sum)
    }
}

/// Conjugate: flip the sign bit of each imaginary lane.
///
/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn cconj4(v: std::arch::x86_64::__m512d) -> std::arch::x86_64::__m512d {
    use std::arch::x86_64::{
        _mm512_castpd_si512, _mm512_castsi512_pd, _mm512_setr_pd, _mm512_xor_si512,
    };
    // Integer xor: `_mm512_xor_pd` is marked AVX-512DQ and would not inline.
    let sign = _mm512_setr_pd(0.0, -0.0, 0.0, -0.0, 0.0, -0.0, 0.0, -0.0);
    _mm512_castsi512_pd(_mm512_xor_si512(
        _mm512_castpd_si512(v),
        _mm512_castpd_si512(sign),
    ))
}

/// # Safety
///
/// AVX-512F is available. `p + 3 < 32`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn load4(col: &[Complex; 32], p: usize) -> std::arch::x86_64::__m512d {
    use std::arch::x86_64::_mm512_loadu_pd;
    unsafe { _mm512_loadu_pd(col.as_ptr().add(p).cast()) }
}

/// # Safety
///
/// AVX-512F is available. `k + 3 < 64`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn store4(col: &mut [Complex; 64], k: usize, v: std::arch::x86_64::__m512d) {
    use std::arch::x86_64::_mm512_storeu_pd;
    unsafe { _mm512_storeu_pd(col.as_mut_ptr().add(k).cast(), v) }
}

/// # Safety
///
/// AVX-512F is available. `a` holds four complexes.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn load_coeff4(a: &[Complex]) -> std::arch::x86_64::__m512d {
    use std::arch::x86_64::_mm512_loadu_pd;
    unsafe { _mm512_loadu_pd(a.as_ptr().cast()) }
}

/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn unpack4(v: std::arch::x86_64::__m512d) -> [Complex; 4] {
    use std::arch::x86_64::_mm512_storeu_pd;
    let mut b = [0.0f64; 8];
    unsafe { _mm512_storeu_pd(b.as_mut_ptr(), v) };
    [
        Complex::new(b[0], b[1]),
        Complex::new(b[2], b[3]),
        Complex::new(b[4], b[5]),
        Complex::new(b[6], b[7]),
    ]
}

/// Four independent covariances. Each lane matches [`prediction_coefficients`].
///
/// # Safety
///
/// AVX-512F is available. `p + 3 < 32` and `x_low` is long enough for the window.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn prediction4_avx512(
    x_low: &[[Complex; 32]],
    p: usize,
    n_slots_frame: usize,
) -> [(Complex, Complex); 4] {
    use std::arch::x86_64::{_mm512_add_pd, _mm512_setzero_pd};
    let mut phi01 = _mm512_setzero_pd();
    let mut phi02 = _mm512_setzero_pd();
    let mut phi11 = _mm512_setzero_pd();
    let mut phi12 = _mm512_setzero_pd();
    let mut phi22 = _mm512_setzero_pd();
    for n in 0..(n_slots_frame + 6) {
        unsafe {
            let x0 = load4(&x_low[n + T_HF_ADJ], p);
            let x1 = load4(&x_low[n + T_HF_ADJ - 1], p);
            let x2 = load4(&x_low[n + T_HF_ADJ - 2], p);
            phi01 = _mm512_add_pd(phi01, cmul4(x0, cconj4(x1)));
            phi02 = _mm512_add_pd(phi02, cmul4(x0, cconj4(x2)));
            phi11 = _mm512_add_pd(phi11, cmul4(x1, cconj4(x1)));
            phi12 = _mm512_add_pd(phi12, cmul4(x1, cconj4(x2)));
            phi22 = _mm512_add_pd(phi22, cmul4(x2, cconj4(x2)));
        }
    }
    let (a01, a02, a11, a12, a22) = unsafe {
        (
            unpack4(phi01),
            unpack4(phi02),
            unpack4(phi11),
            unpack4(phi12),
            unpack4(phi22),
        )
    };
    let mut out = [(Complex::default(), Complex::default()); 4];
    for i in 0..4 {
        out[i] = alphas_from_phi(a01[i], a02[i], a11[i], a12[i], a22[i]);
    }
    out
}

/// `X[c][k+i] = Xlow[c][p+i] + a0[i]·Xlow[c−1][p+i] + a1[i]·Xlow[c−2][p+i]`.
///
/// # Safety
///
/// AVX-512F is available. `c0 >= 2`, `c1 <= x_low.len()`, `k + 3 < 64`, `p + 3 < 32`,
/// and both coefficient slices hold four complexes.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[allow(clippy::too_many_arguments)]
unsafe fn apply_hf4_avx512(
    x_low: &[[Complex; 32]],
    x_high: &mut [[Complex; 64]],
    k: usize,
    p: usize,
    a0b: &[Complex],
    a1b: &[Complex],
    c0: usize,
    c1: usize,
) {
    use std::arch::x86_64::_mm512_add_pd;
    unsafe {
        let a0 = load_coeff4(a0b);
        let a1 = load_coeff4(a1b);
        for c in c0..c1 {
            let x = load4(&x_low[c], p);
            let xp = load4(&x_low[c - 1], p);
            let xp2 = load4(&x_low[c - 2], p);
            let acc = _mm512_add_pd(x, cmul4(a0, xp));
            store4(&mut x_high[c], k, _mm512_add_pd(acc, cmul4(a1, xp2)));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Table 4.175 spot values.
    #[test]
    fn new_bw_table() {
        assert_eq!(new_bw(0, 0), 0.0);
        assert_eq!(new_bw(0, 1), 0.6);
        assert_eq!(new_bw(1, 0), 0.6);
        assert_eq!(new_bw(1, 1), 0.75);
        assert_eq!(new_bw(2, 0), 0.0);
        assert_eq!(new_bw(2, 1), 0.75);
        assert_eq!(new_bw(3, 3), 0.98);
        assert_eq!(new_bw(0, 2), 0.9);
    }

    /// Chirp blend: rising values take the 0.90625/0.09375 mix,
    /// falling values the 0.75/0.25 mix, and tiny results flush to 0.
    #[test]
    fn chirp_blend_and_flush() {
        // First frame: prev all zero. newBw(0, 3) = 0.98 rising:
        // 0.90625·0.98 = 0.888125.
        let bw = chirp_factors(&[3], &[0], &[0.0]);
        assert!((bw[0] - 0.888125).abs() < 1e-12);
        // Falling: newBw(3, 0) = 0.0 < prev 0.888125:
        // 0.25·0.888125 = 0.22203125.
        let bw2 = chirp_factors(&[0], &[3], &bw);
        assert!((bw2[0] - 0.22203125).abs() < 1e-12);
        // Repeated Off decays geometrically to below 0.015625 → 0.
        let mut cur = bw2;
        for _ in 0..4 {
            cur = chirp_factors(&[0], &[0], &cur);
        }
        assert_eq!(cur[0], 0.0);
    }

    /// Figure 4.48 on a hand-walked geometry: fMaster = 8..=24 step 2,
    /// k0 = kx = 8, M = 16, goalSb past the range.
    #[test]
    fn patch_construction_hand_walked() {
        let f_master: Vec<i32> = (0..=8).map(|i| 8 + 2 * i).collect();
        // fs_sbr small enough that goalSb = NINT(2.048e6/fs) ≥ 24.
        let p = build_patches(&f_master, 8, 8, 16, 85_000).unwrap();
        // Iter 1: sb = 14 → patch (start 2, num 6);
        // iter 2: sb = 20 → patch (2, 6); iter 3: sb = 24 → (4, 4).
        assert_eq!(p.start, vec![2, 2, 4]);
        assert_eq!(p.num, vec![6, 6, 4]);
        assert_eq!(p.borders(8), vec![8, 14, 20, 24]);
    }

    /// The patch trim drops a trailing patch narrower than 3 subbands.
    #[test]
    fn patch_trim_drops_small_tail() {
        // fMaster reaching kx + M = 22 with a final 2-wide step.
        let f_master = vec![8, 10, 12, 14, 16, 20, 22];
        let p = build_patches(&f_master, 8, 8, 14, 85_000).unwrap();
        // Walk: msb=8,usb=8 → sb=14 (odd 0) num 6 start 2;
        // then sb=20? 20 ≤ 7+14-0=21 → num 6 start 2; then sb=22:
        // 22 ≤ 7+20-0=27 → num 2 start 6 → trimmed.
        assert_eq!(p.num, vec![6, 6]);
        assert_eq!(p.start, vec![2, 2]);
    }

    /// Patch invariants on a spec-derived master table (44.1 kHz
    /// HE-AAC geometry).
    #[test]
    fn patch_invariants_on_derived_master() {
        let fs_sbr = 44_100;
        let k0 = crate::engine::sbr_freq_bands::k0(fs_sbr, 5).unwrap();
        let k2 = crate::engine::sbr_freq_bands::k2(fs_sbr, 5, k0).unwrap();
        let fm = crate::engine::sbr_freq_bands::master_table(k0, k2, 2, true).unwrap();
        let bands = HiLoTables::derive(&fm, 0, 2).unwrap();
        let p = build_patches(&fm, k0, bands.k_x, bands.m, fs_sbr).unwrap();
        assert!(p.num_patches() >= 1 && p.num_patches() <= MAX_PATCHES);
        for (&s, &n) in p.start.iter().zip(p.num.iter()) {
            assert!(n > 0);
            // Source range lies below the first master subband.
            assert!((s + n) as i32 <= k0);
        }
        // Borders start at kx and stay within kx + M.
        let borders = p.borders(bands.k_x);
        assert_eq!(borders[0], bands.k_x);
        assert!(*borders.last().unwrap() <= bands.k_x + bands.m);
    }

    /// Build a slot-major XLow whose band `k` carries an exact
    /// second-order recursion `x[n] = a1·x[n-1] + a2·x[n-2]`.
    fn ar2_xlow(k: usize, a1: Complex, a2: Complex, cols: usize) -> Vec<[Complex; 32]> {
        let mut x = vec![[Complex::default(); 32]; cols];
        x[0][k] = Complex::new(1.0, 0.3);
        x[1][k] = Complex::new(0.2, -0.5);
        for n in 2..cols {
            let v = a1 * x[n - 1][k] + a2 * x[n - 2][k];
            x[n][k] = v;
        }
        x
    }

    /// The covariance method recovers an exact AR(2) recursion:
    /// α0 = −a1, α1 = −a2.
    #[test]
    fn prediction_recovers_ar2() {
        let a1 = Complex::new(0.9, 0.1);
        let a2 = Complex::new(-0.5, 0.05);
        let x = ar2_xlow(3, a1, a2, 40);
        let (al0, al1) = prediction_coefficients(&x, 3, 32).unwrap();
        // The εInv = 1e-6 relaxation perturbs the exact solution by
        // O(εInv), so the recovery is pinned to that scale.
        assert!((al0 + a1).norm_sqr() < 1e-10, "{al0:?}");
        assert!((al1 + a2).norm_sqr() < 1e-10, "{al1:?}");
    }

    /// |α| ≥ 4 resets both coefficients.
    #[test]
    fn prediction_resets_large_coefficients() {
        // An unstable recursion with |a1| > 4 forces the reset.
        let a1 = Complex::new(4.5, 0.0);
        let a2 = Complex::new(0.0, 0.0);
        let mut x = vec![[Complex::default(); 32]; 40];
        x[0][0] = Complex::new(1e-6, 0.0);
        for n in 1..40 {
            let v = a1 * x[n - 1][0];
            x[n][0] = v;
        }
        let _ = a2;
        let (al0, al1) = prediction_coefficients(&x, 0, 32).unwrap();
        assert_eq!(al0, Complex::default());
        assert_eq!(al1, Complex::default());
    }

    fn tiny_bands() -> HiLoTables {
        HiLoTables {
            f_table_high: vec![8, 12, 16],
            f_table_low: vec![8, 16],
            f_table_noise: vec![8, 16],
            m: 8,
            k_x: 8,
        }
    }

    /// bw = 0 copies the source band; bw = 1 on a perfectly
    /// predictable source whitens it to (near) zero.
    #[test]
    fn generate_copies_and_whitens() {
        let a1 = Complex::new(0.8, 0.2);
        let a2 = Complex::new(-0.4, 0.0);
        let x = ar2_xlow(2, a1, a2, 40);
        let patches = Patches {
            start: vec![2],
            num: vec![8],
        };
        let bands = tiny_bands();
        // bw = 0: XHigh(k) == XLow(p) on the generated range. Patch
        // maps source 2..10 → 8..16; k = 8 comes from p = 2.
        let hi = generate_hf(&x, &patches, &[0.0], &bands, 0..32, 32).unwrap();
        for l in 0..32usize {
            let c = l + T_HF_ADJ;
            assert_eq!(hi[c][8], x[c][2]);
        }
        // bw = 1: the inverse filter cancels the AR(2) recursion (to
        // the O(εInv) accuracy of the relaxed covariance solution).
        let hi = generate_hf(&x, &patches, &[1.0], &bands, 0..32, 32).unwrap();
        let sig: f64 = (0..32).map(|l| x[l + T_HF_ADJ][2].norm_sqr()).sum();
        let res: f64 = (0..32).map(|l| hi[l + T_HF_ADJ][8].norm_sqr()).sum();
        assert!(res < 1e-10 * sig, "residual {res} vs signal {sig}");
        // Un-patched bands stay zero.
        for col in &hi {
            assert_eq!(col[20], Complex::default());
        }
    }
}
