//! SBR HF adjustment (envelope adjuster) — ISO/IEC 14496-3 §4.6.18.7.
//!
//! Takes the HF-generated subband matrix `XHigh` and produces the
//! output matrix `Y` over the `M` SBR subbands starting at `kx`:
//!
//! * **Mapping** (§4.6.18.7.2) — `EOrigMapped` / `QMapped` to QMF
//!   resolution, the `SIndexMapped` sinusoid placement (band middle,
//!   `δStep` start gate against `lA` and the previous frame's
//!   sinusoids) and the `SMapped` band flags.
//! * **Current envelope estimation** (§4.6.18.7.3) — `ECurr` by
//!   squared-magnitude averaging, per subband (`bs_interpol_freq = 1`)
//!   or per envelope band.
//! * **Additional-component levels** (§4.6.18.7.4) — `QM` / `SM`
//!   (amplitude domain, i.e. with the square root of the energy
//!   ratios).
//! * **Gain** (§4.6.18.7.5) — `G`, the limiter (`GMax` from the
//!   `fTableLim` band ratios and `limGain`), the noise-level limit
//!   `QM_Lim`, and the boost compensation `GBoost` capped at
//!   `1.584893192`.
//! * **Assembly** (§4.6.18.7.6) — the `hSmooth` gain/noise smoothing
//!   over `hSL` columns, `W1 = GFilt·XHigh`, the Table 4.A.91 noise
//!   mix `W2`, and the `φsin` sinusoid injection with the
//!   `(−1)^(m+kx)` imaginary alternation, producing `Y`.
//!
//! Cross-frame state (`EnvAdjustState`) carries the previous frame's
//! last-envelope `SIndexMapped`, `lA` / `LE`, the `GTemp` / `QTemp`
//! smoothing tails, and the running `indexNoise` / `indexSine`.
//!
//! ## Provenance
//!
//! Every formula (including the square roots the §4.6.18.7.4–7.5
//! equations carry) was read from the staged ISO/IEC 14496-3:2009 spec
//! PDF's typeset equations. No part of this implementation is derived
//! from any external decoder.

use crate::engine::sbr_freq_bands::HiLoTables;
use crate::engine::sbr_hf_gen::T_HF_ADJ;
use crate::engine::sbr_noise_table::NOISE_TABLE;
use crate::engine::sbr_qmf::Complex;
use crate::engine::{Error, Result};

/// `limGain = [0.70795, 1.0, 1.41254, 1e10]` (§4.6.18.7.5).
pub const LIM_GAIN: [f64; 4] = [0.70795, 1.0, 1.41254, 1e10];

/// `ε0 = 1e-12` (§4.6.18.7.5).
pub const EPS0: f64 = 1e-12;

/// `ε = 1` (§4.6.18.2.5) — the division-by-zero guard in the gain.
pub const EPS: f64 = 1.0;

/// The `GBoost` cap `1.584893192` (§4.6.18.7.5).
pub const MAX_BOOST: f64 = 1.584893192;

/// The `GMax` cap `10^5` (§4.6.18.7.5).
pub const G_MAX_CAP: f64 = 1e5;

/// `hSmooth` — the §4.6.18.7.6 smoothing filter.
pub const H_SMOOTH: [f64; 5] = [
    0.33333333333333,
    0.30150283239582,
    0.21816949906249,
    0.11516383427084,
    0.03183050093751,
];

/// `φRe,sin = [1, 0, −1, 0]`, `φIm,sin = [0, 1, 0, −1]` (§4.6.18.7.6).
pub const PHI_SIN: [(f64, f64); 4] = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];

/// Per-frame inputs to the envelope adjuster (one channel).
#[derive(Debug)]
pub struct EnvParams<'a> {
    /// Derived frequency band tables (`M`, `kx`, high/low/noise).
    pub bands: &'a HiLoTables,
    /// The §4.6.18.3.2.3 limiter band table `fTableLim(0..=NL)`.
    pub f_table_lim: &'a [i32],
    /// Envelope time borders `tE(0..=LE)` (slots).
    pub t_e: &'a [i32],
    /// Noise-floor time borders `tQ(0..=LQ)` (slots).
    pub t_q: &'a [i32],
    /// Per-envelope frequency resolution `r(l)` (`true` = high).
    pub freq_res: &'a [bool],
    /// Table 4.176 `lA` (`-1` = none).
    pub l_a: i32,
    /// Dequantized envelope energies `EOrig[l][k]`.
    pub e_orig: &'a [Vec<f64>],
    /// Dequantized noise-floor energies `QOrig[l][k]`.
    pub q_orig: &'a [Vec<f64>],
    /// `bs_add_harmonic` flags (`NHigh` entries; empty = none).
    pub add_harmonic: &'a [bool],
    /// `bs_interpol_freq`.
    pub interpol_freq: bool,
    /// `bs_smoothing_mode` (`true` ⇒ `hSL = 0`).
    pub smoothing_mode: bool,
    /// `bs_limiter_gains` (`0..=3`, indexes [`LIM_GAIN`]).
    pub limiter_gains: u8,
    /// The §4.6.18.3.3 reset flag (header band geometry changed).
    pub reset: bool,
}

/// Per-frame working tables of [`adjust_into`] (`8 × 64` envelope maps,
/// `48 × 64` smoothing buffers), reused across frames.
#[derive(Debug, Clone, Default)]
struct AdjScratch {
    e_map: Vec<f64>,
    q_map: Vec<f64>,
    s_index: Vec<bool>,
    s_map: Vec<bool>,
    e_curr: Vec<f64>,
    g_lim_boost: Vec<f64>,
    q_m_lim_boost: Vec<f64>,
    s_m_boost: Vec<f64>,
    g_temp: Vec<f64>,
    q_temp: Vec<f64>,
}

impl AdjScratch {
    fn reset(&mut self) {
        for v in [
            &mut self.e_map,
            &mut self.q_map,
            &mut self.e_curr,
            &mut self.g_lim_boost,
            &mut self.q_m_lim_boost,
            &mut self.s_m_boost,
        ] {
            v.clear();
            v.resize(8 * 64, 0.0);
        }
        for v in [&mut self.s_index, &mut self.s_map] {
            v.clear();
            v.resize(8 * 64, false);
        }
        for v in [&mut self.g_temp, &mut self.q_temp] {
            v.clear();
            v.resize(48 * 64, 0.0);
        }
    }
}

/// Cross-frame envelope-adjuster state for one channel, plus reused
/// per-frame mapping/gain scratch (TASK-79).
#[derive(Debug, Default)]
pub struct EnvAdjustState {
    /// Previous frame's last-envelope `SIndexMapped` (per SBR subband,
    /// `kx`-relative) plus its `kx`, for the `δStep` gate.
    s_index_prev: Vec<bool>,
    k_x_prev: i32,
    /// Previous frame's `lA` and `LE` (for `lAPrev`).
    l_a_prev_frame: i32,
    l_e_prev: i32,
    /// Previous frame's trailing `hSL` columns of `GTemp` / `QTemp`.
    g_temp_tail: Vec<Vec<f64>>,
    q_temp_tail: Vec<Vec<f64>>,
    /// Running noise / sine phase indices.
    index_noise: usize,
    index_sine: usize,
    started: bool,
    scratch: AdjScratch,
}

impl EnvAdjustState {
    /// Fresh state (first frame / after a stream reset).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Run the §4.6.18.7 HF adjustment for one channel's SBR frame.
///
/// `x_high` is the slot-major HF-generator output (spec absolute
/// columns, i.e. spec index `i + tHFAdj` is a direct column index).
/// Returns `Y` in the same layout, filled for the SBR range and the
/// frame's envelope span; other cells are zero.
#[cfg(test)]
pub fn adjust(
    x_high: &[[Complex; 64]],
    p: &EnvParams<'_>,
    st: &mut EnvAdjustState,
) -> Result<Vec<[Complex; 64]>> {
    let mut y = x_high.to_vec();
    adjust_into(&mut y, p, st)?;
    Ok(y)
}

/// Write `Y` into `x_high` in place (ECurr is computed first).
pub fn adjust_into(
    x_high: &mut [[Complex; 64]],
    p: &EnvParams<'_>,
    st: &mut EnvAdjustState,
) -> Result<()> {
    let m_cnt = usize::try_from(p.bands.m).map_err(|_| Error::SbrFreqBandInvalid)?;
    let k_x = p.bands.k_x;
    let l_e = p
        .t_e
        .len()
        .checked_sub(1)
        .ok_or(Error::SbrFreqBandInvalid)?;
    if l_e == 0
        || p.freq_res.len() != l_e
        || p.e_orig.len() != l_e
        || p.q_orig.len() + 1 != p.t_q.len()
        || p.f_table_lim.len() < 2
        || usize::from(p.limiter_gains) >= LIM_GAIN.len()
    {
        return Err(Error::SbrFreqBandInvalid);
    }

    let rate = 2i32; // RATE (§4.6.18.2.5)
    let i0 = rate * p.t_e[0];
    let i_end = rate * p.t_e[l_e];
    let n_cols = usize::try_from(i_end - i0).map_err(|_| Error::SbrFreqBandInvalid)?;
    if i0 < 0
        || usize::try_from(i_end).map_err(|_| Error::SbrFreqBandInvalid)? + T_HF_ADJ > x_high.len()
    {
        return Err(Error::SbrFreqBandInvalid);
    }

    if p.reset || !st.started {
        st.index_noise = 0;
        st.index_sine = 0;
        st.s_index_prev.clear();
        st.g_temp_tail.clear();
        st.q_temp_tail.clear();
        st.l_a_prev_frame = -1;
        st.l_e_prev = 0;
        st.started = true;
    }

    // lAPrev: 0 if the previous frame's transient sat on its trailing
    // border, else -1.
    let l_a_prev = if st.l_a_prev_frame == st.l_e_prev {
        0i32
    } else {
        -1
    };

    // ---- §4.6.18.7.2 mapping -------------------------------------
    let h_sl: usize = if p.smoothing_mode { 0 } else { 4 };
    if l_e > 8 || m_cnt > 64 || n_cols + h_sl > 48 {
        return Err(Error::SbrFreqBandInvalid);
    }
    // Frame-sized tables live in the state, zeroed per frame: as locals
    // they were 77 KiB of stack (TASK-118).
    let mut sc = std::mem::take(&mut st.scratch);
    sc.reset();
    let AdjScratch {
        e_map,
        q_map,
        s_index,
        s_map,
        e_curr,
        g_lim_boost,
        q_m_lim_boost,
        s_m_boost,
        g_temp,
        q_temp,
    } = &mut sc;
    let mut q_m = [0.0f64; 64];
    let mut s_m = [0.0f64; 64];
    let mut g = [0.0f64; 64];
    let mut g_max = [0.0f64; 64];
    let mut q_m_lim = [0.0f64; 64];
    let mut g_lim = [0.0f64; 64];
    let ix = |l: usize, m: usize| l * m_cnt + m;

    // Envelope band table per resolution.
    let f_of = |high: bool| -> &Vec<i32> {
        if high {
            &p.bands.f_table_high
        } else {
            &p.bands.f_table_low
        }
    };
    // Band index of QMF subband `k` in border table `f`.
    let band_of = |f: &[i32], k: i32| -> Result<usize> {
        for i in 0..f.len() - 1 {
            if f[i] <= k && k < f[i + 1] {
                return Ok(i);
            }
        }
        Err(Error::SbrFreqBandInvalid)
    };

    let n_high = p.bands.n_high();
    for l in 0..l_e {
        let f = f_of(p.freq_res[l]);
        if p.e_orig[l].len() + 1 != f.len() {
            return Err(Error::SbrFreqBandInvalid);
        }
        // k(l): the noise floor whose span contains envelope l.
        let mut kq = None;
        for q in 0..p.t_q.len() - 1 {
            if p.t_q[q] <= p.t_e[l] && p.t_e[l + 1] <= p.t_q[q + 1] {
                kq = Some(q);
                break;
            }
        }
        let kq = kq.ok_or(Error::SbrFreqBandInvalid)?;
        if p.q_orig[kq].len() + 1 != p.bands.f_table_noise.len() {
            return Err(Error::SbrFreqBandInvalid);
        }
        for m in 0..m_cnt {
            let k = k_x + i32::try_from(m).map_err(|_| Error::SbrFreqBandInvalid)?;
            e_map[ix(l, m)] = p.e_orig[l][band_of(f, k)?];
            q_map[ix(l, m)] = p.q_orig[kq][band_of(&p.bands.f_table_noise, k)?];
        }

        // SIndexMapped: sinusoid in the middle subband of each
        // high-resolution band, gated by δStep.
        if !p.add_harmonic.is_empty() {
            if p.add_harmonic.len() != n_high {
                return Err(Error::SbrFreqBandInvalid);
            }
            for (i, &on) in p.add_harmonic.iter().enumerate() {
                if !on {
                    continue;
                }
                let mid = (p.bands.f_table_high[i + 1] + p.bands.f_table_high[i]) / 2;
                let m_rel = mid - k_x;
                if m_rel < 0 || m_rel as usize >= m_cnt {
                    continue;
                }
                // δStep: on from lA, or already ringing in the
                // previous frame's last envelope.
                let prev_on = {
                    let prev_rel = mid - st.k_x_prev;
                    prev_rel >= 0
                        && st
                            .s_index_prev
                            .get(prev_rel as usize)
                            .copied()
                            .unwrap_or(false)
                };
                if (l as i32) >= p.l_a || prev_on {
                    s_index[ix(l, m_rel as usize)] = true;
                }
            }
        }
        // SMapped: any sinusoid within the envelope band.
        for i in 0..f.len() - 1 {
            let any = ((f[i] - k_x).max(0)..(f[i + 1] - k_x).max(0))
                .any(|j| (j as usize) < m_cnt && s_index[ix(l, j as usize)]);
            if any {
                for j in (f[i] - k_x).max(0)..(f[i + 1] - k_x).max(0) {
                    if (j as usize) < m_cnt {
                        s_map[ix(l, j as usize)] = true;
                    }
                }
            }
        }
    }

    // ---- §4.6.18.7.3 current envelope ----------------------------
    for l in 0..l_e {
        let lo = (rate * p.t_e[l] + T_HF_ADJ as i32) as usize;
        let hi = (rate * p.t_e[l + 1] + T_HF_ADJ as i32) as usize;
        let width = (hi - lo) as f64;
        if p.interpol_freq {
            for m in 0..m_cnt {
                let k = (k_x as usize) + m;
                let sum: f64 = x_high[lo..hi].iter().map(|col| col[k].norm_sqr()).sum();
                e_curr[ix(l, m)] = sum / width;
            }
        } else {
            let f = f_of(p.freq_res[l]);
            for pband in 0..f.len() - 1 {
                let kl = f[pband];
                let kh = f[pband + 1] - 1;
                let mut sum = 0.0;
                for j in kl..=kh {
                    sum += x_high[lo..hi]
                        .iter()
                        .map(|col| col[j as usize].norm_sqr())
                        .sum::<f64>();
                }
                let avg = sum / (width * f64::from(kh - kl + 1));
                for j in kl..=kh {
                    let m_rel = j - k_x;
                    if m_rel >= 0 && (m_rel as usize) < m_cnt {
                        e_curr[ix(l, m_rel as usize)] = avg;
                    }
                }
            }
        }
    }

    // ---- §4.6.18.7.4 / 7.5 gain, limiter, boost ------------------
    let lim_gain = LIM_GAIN[usize::from(p.limiter_gains)];
    let n_l = p.f_table_lim.len() - 1;

    for l in 0..l_e {
        let li = l as i32;
        let delta_l = if li == p.l_a || li == l_a_prev {
            0.0
        } else {
            1.0
        };

        for m in 0..m_cnt {
            let e_o = e_map[ix(l, m)];
            let q = q_map[ix(l, m)];
            q_m[m] = (e_o * q / (1.0 + q)).sqrt();
            s_m[m] = if s_index[ix(l, m)] {
                (e_o / (1.0 + q)).sqrt()
            } else {
                0.0
            };
            g[m] = if s_map[ix(l, m)] {
                ((e_o / (EPS + e_curr[ix(l, m)])) * (q / (1.0 + q))).sqrt()
            } else {
                (e_o / ((EPS + e_curr[ix(l, m)]) * (1.0 + delta_l * q))).sqrt()
            };
        }

        for k in 0..n_l {
            let lo = (p.f_table_lim[k] - k_x).max(0) as usize;
            let hi = ((p.f_table_lim[k + 1] - k_x).max(0) as usize).min(m_cnt);
            let num: f64 = EPS0 + e_map[ix(l, lo)..ix(l, hi)].iter().sum::<f64>();
            let den: f64 = EPS0 + e_curr[ix(l, lo)..ix(l, hi)].iter().sum::<f64>();
            let gmax = ((num / den).sqrt() * lim_gain).min(G_MAX_CAP);
            for gm in &mut g_max[lo..hi] {
                *gm = gmax;
            }
        }

        for m in 0..m_cnt {
            q_m_lim[m] = if g[m] > 0.0 {
                q_m[m].min(q_m[m] * g_max[m] / g[m])
            } else {
                q_m[m]
            };
            g_lim[m] = g[m].min(g_max[m]);
        }

        for k in 0..n_l {
            let lo = (p.f_table_lim[k] - k_x).max(0) as usize;
            let hi = ((p.f_table_lim[k + 1] - k_x).max(0) as usize).min(m_cnt);
            let mut num = EPS0;
            let mut den = EPS0;
            for i in lo..hi {
                num += e_map[ix(l, i)];
                let delta_s = if s_m[i] != 0.0 || li == p.l_a || li == l_a_prev {
                    0.0
                } else {
                    1.0
                };
                den += e_curr[ix(l, i)] * g_lim[i] * g_lim[i]
                    + s_m[i] * s_m[i]
                    + delta_s * q_m_lim[i] * q_m_lim[i];
            }
            let boost = (num / den).sqrt().min(MAX_BOOST);
            for i in lo..hi {
                g_lim_boost[ix(l, i)] = g_lim[i] * boost;
                q_m_lim_boost[ix(l, i)] = q_m_lim[i] * boost;
                s_m_boost[ix(l, i)] = s_m[i] * boost;
            }
        }
    }

    // ---- §4.6.18.7.6 assembly ------------------------------------
    for j in 0..h_sl {
        let dst_g = j * m_cnt;
        if st.g_temp_tail.len() == h_sl && st.g_temp_tail[j].len() == m_cnt {
            g_temp[dst_g..dst_g + m_cnt].copy_from_slice(&st.g_temp_tail[j]);
            q_temp[dst_g..dst_g + m_cnt].copy_from_slice(&st.q_temp_tail[j]);
        } else {
            g_temp[dst_g..dst_g + m_cnt].copy_from_slice(&g_lim_boost[..m_cnt]);
            q_temp[dst_g..dst_g + m_cnt].copy_from_slice(&q_m_lim_boost[..m_cnt]);
        }
    }
    let env_of = |i: i32| -> usize {
        let mut l = l_e - 1;
        for e in 0..l_e {
            if i >= rate * p.t_e[e] && i < rate * p.t_e[e + 1] {
                l = e;
                break;
            }
        }
        l
    };
    for c in 0..n_cols {
        let l = env_of(i0 + c as i32);
        let dst = (c + h_sl) * m_cnt;
        let src = l * m_cnt;
        g_temp[dst..dst + m_cnt].copy_from_slice(&g_lim_boost[src..src + m_cnt]);
        q_temp[dst..dst + m_cnt].copy_from_slice(&q_m_lim_boost[src..src + m_cnt]);
    }

    let mut f_index_noise = 0usize;
    let mut f_index_sine = 0usize;
    for c in 0..n_cols {
        let i = i0 + c as i32;
        let l = env_of(i);
        let li = l as i32;
        let col = (i + T_HF_ADJ as i32) as usize;
        let smooth_gain = li != p.l_a && li != l_a_prev && h_sl != 0;
        f_index_sine = (st.index_sine + c) % 4;
        let noise_killed = li == p.l_a || li == l_a_prev;
        if h_sl == 0 && column_plain(s_index, s_m_boost, l, m_cnt, noise_killed) {
            let grow = &g_temp[(c + h_sl) * m_cnt..(c + h_sl) * m_cnt + m_cnt];
            let qrow = &q_temp[(c + h_sl) * m_cnt..(c + h_sl) * m_cnt + m_cnt];
            let noise0 = (st.index_noise + c * m_cnt + 1) % 512;
            apply_plain_column(
                &mut x_high[col],
                k_x.max(0) as usize,
                grow,
                qrow,
                noise0,
                noise_killed,
            );
            if m_cnt > 0 {
                f_index_noise = (st.index_noise + (c + 1) * m_cnt) % 512;
            }
            continue;
        }
        let (sin_re, sin_im) = PHI_SIN[f_index_sine];
        for m in 0..m_cnt {
            let k = (k_x as usize) + m;
            let g_filt = if smooth_gain {
                let mut acc = 0.0;
                for j in 0..=h_sl {
                    acc += g_temp[(c + h_sl - j) * m_cnt + m] * H_SMOOTH[j];
                }
                acc
            } else {
                g_temp[(c + h_sl) * m_cnt + m]
            };
            let q_filt = if li == p.l_a || li == l_a_prev || s_m_boost[ix(l, m)] != 0.0 {
                0.0
            } else if h_sl != 0 {
                let mut acc = 0.0;
                for j in 0..=h_sl {
                    acc += q_temp[(c + h_sl - j) * m_cnt + m] * H_SMOOTH[j];
                }
                acc
            } else {
                q_temp[(c + h_sl) * m_cnt + m]
            };

            let w1 = x_high[col][k] * g_filt;
            f_index_noise = (st.index_noise + c * m_cnt + m + 1) % 512;
            let (v_re, v_im) = NOISE_TABLE[f_index_noise];
            let mut out = Complex::new(w1.re + q_filt * v_re, w1.im + q_filt * v_im);

            if s_index[ix(l, m)] {
                let s = s_m_boost[ix(l, m)];
                let alt = if (m + k_x as usize) % 2 == 1 {
                    -1.0
                } else {
                    1.0
                };
                out.re += s * sin_re;
                out.im += s * alt * sin_im;
            }
            x_high[col][k] = out;
        }
    }

    let written_lo = (i0 + T_HF_ADJ as i32) as usize;
    let written_hi = (i_end + T_HF_ADJ as i32) as usize;
    let kx_u = k_x.max(0) as usize;
    for (i, col) in x_high.iter_mut().enumerate() {
        if i < written_lo || i >= written_hi {
            *col = [Complex::default(); 64];
            continue;
        }
        for cell in col.iter_mut().take(kx_u) {
            *cell = Complex::default();
        }
        for cell in col.iter_mut().skip(kx_u + m_cnt) {
            *cell = Complex::default();
        }
    }

    // ---- thread cross-frame state --------------------------------
    st.index_noise = if n_cols > 0 {
        f_index_noise
    } else {
        st.index_noise
    };
    st.index_sine = if n_cols > 0 {
        (f_index_sine + 1) % 4
    } else {
        st.index_sine
    };
    if st.g_temp_tail.len() < h_sl {
        st.g_temp_tail.resize(h_sl, Vec::new());
        st.q_temp_tail.resize(h_sl, Vec::new());
    }
    st.g_temp_tail.truncate(h_sl);
    st.q_temp_tail.truncate(h_sl);
    for j in 0..h_sl {
        let src = (n_cols + j) * m_cnt;
        st.g_temp_tail[j].clear();
        st.g_temp_tail[j].extend_from_slice(&g_temp[src..src + m_cnt]);
        st.q_temp_tail[j].clear();
        st.q_temp_tail[j].extend_from_slice(&q_temp[src..src + m_cnt]);
    }
    st.s_index_prev.clear();
    st.s_index_prev
        .extend_from_slice(&s_index[ix(l_e - 1, 0)..ix(l_e - 1, 0) + m_cnt]);
    st.k_x_prev = k_x;
    st.l_a_prev_frame = p.l_a;
    st.l_e_prev = l_e as i32;

    st.scratch = sc;
    Ok(())
}

/// No sinusoid in the envelope, and noise is either off for every band or
/// present on every band. The per-band branches then do not change the formula.
#[allow(clippy::ptr_arg)]
fn column_plain(
    s_index: &Vec<bool>,
    s_boost: &[f64],
    l: usize,
    m_cnt: usize,
    noise_killed: bool,
) -> bool {
    let base = l * m_cnt;
    for m in 0..m_cnt {
        if s_index[base + m] {
            return false;
        }
        if !noise_killed && s_boost[base + m] != 0.0 {
            return false;
        }
    }
    true
}

/// `Y = G·X` or `Y = G·X + Q·noise`, four bands at a time when AVX-512F is up.
/// `hSL = 0`, so `G` and `Q` are the current column with no smoothing sum.
fn apply_plain_column(
    col: &mut [Complex; 64],
    kx: usize,
    g: &[f64],
    q: &[f64],
    noise0: usize,
    noise_killed: bool,
) {
    let m_cnt = g.len();
    if kx.saturating_add(m_cnt) > 64 || q.len() != m_cnt {
        apply_plain_scalar(col, kx, g, q, noise0, noise_killed);
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F is probed and `kx + m_cnt <= 64`.
        unsafe { apply_plain_avx512(col, kx, g, q, noise0, noise_killed) };
        return;
    }
    apply_plain_scalar(col, kx, g, q, noise0, noise_killed);
}

fn apply_plain_scalar(
    col: &mut [Complex; 64],
    kx: usize,
    g: &[f64],
    q: &[f64],
    mut noise: usize,
    noise_killed: bool,
) {
    for m in 0..g.len() {
        let x = col[kx + m];
        let gv = g[m];
        if noise_killed {
            col[kx + m] = Complex::new(x.re * gv, x.im * gv);
        } else {
            if noise == 512 {
                noise = 0;
            }
            let (vr, vi) = NOISE_TABLE[noise];
            let qv = q[m];
            col[kx + m] = Complex::new(x.re * gv + qv * vr, x.im * gv + qv * vi);
            noise += 1;
        }
    }
}

/// # Safety
///
/// AVX-512F is available. `kx + g.len() <= 64` and `q.len() == g.len()`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn apply_plain_avx512(
    col: &mut [Complex; 64],
    kx: usize,
    g: &[f64],
    q: &[f64],
    mut noise: usize,
    noise_killed: bool,
) {
    use std::arch::x86_64::{
        _mm256_loadu_pd, _mm512_add_pd, _mm512_castpd256_pd512, _mm512_loadu_pd, _mm512_mul_pd,
        _mm512_permutexvar_pd, _mm512_setr_epi64, _mm512_storeu_pd,
    };
    let m_cnt = g.len();
    let mut m = 0usize;
    let dup = _mm512_setr_epi64(0, 0, 1, 1, 2, 2, 3, 3);
    while m + 4 <= m_cnt {
        if !noise_killed && noise + 4 > 512 {
            break;
        }
        unsafe {
            let x = _mm512_loadu_pd(col.as_ptr().add(kx + m).cast());
            let gv = _mm512_castpd256_pd512(_mm256_loadu_pd(g.as_ptr().add(m)));
            let gp = _mm512_permutexvar_pd(dup, gv);
            let mut y = _mm512_mul_pd(x, gp);
            if !noise_killed {
                let qv = _mm512_castpd256_pd512(_mm256_loadu_pd(q.as_ptr().add(m)));
                let qp = _mm512_permutexvar_pd(dup, qv);
                let np = NOISE_TABLE.as_ptr().cast::<f64>().add(noise * 2);
                let nv = _mm512_loadu_pd(np);
                y = _mm512_add_pd(y, _mm512_mul_pd(nv, qp));
                noise += 4;
            }
            _mm512_storeu_pd(col.as_mut_ptr().add(kx + m).cast(), y);
        }
        m += 4;
    }
    if m < m_cnt {
        apply_plain_scalar(col, kx + m, &g[m..], &q[m..], noise % 512, noise_killed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn bands() -> HiLoTables {
        HiLoTables {
            f_table_high: vec![8, 10, 12, 14, 16],
            f_table_low: vec![8, 12, 16],
            f_table_noise: vec![8, 16],
            m: 8,
            k_x: 8,
        }
    }

    fn flat_x_high(amp: f64, cols: usize) -> Vec<[Complex; 64]> {
        let mut x = vec![[Complex::default(); 64]; cols];
        for (ci, col) in x.iter_mut().enumerate() {
            for (k, cell) in col.iter_mut().enumerate().take(16).skip(8) {
                // A deterministic unit-magnitude phase pattern.
                let ph = (ci * 7 + k) as f64 * 0.37;
                *cell = Complex::new(amp * ph.cos(), amp * ph.sin());
            }
        }
        x
    }

    #[allow(clippy::too_many_arguments)]
    fn params<'a>(
        b: &'a HiLoTables,
        lim: &'a [i32],
        t_e: &'a [i32],
        t_q: &'a [i32],
        freq_res: &'a [bool],
        e_orig: &'a [Vec<f64>],
        q_orig: &'a [Vec<f64>],
        add: &'a [bool],
    ) -> EnvParams<'a> {
        EnvParams {
            bands: b,
            f_table_lim: lim,
            t_e,
            t_q,
            freq_res,
            l_a: -1,
            e_orig,
            q_orig,
            add_harmonic: add,
            interpol_freq: true,
            smoothing_mode: true,
            limiter_gains: 3,
            reset: false,
        }
    }

    /// A flat XHigh with EOrig = G²·|X|² reproduces gain G on every
    /// sample (no noise, no sinusoids, limiter wide open).
    #[test]
    fn flat_gain_reproduces_target_envelope() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 16];
        let t_q = [0, 16];
        let fr = [true];
        let amp = 100.0;
        let target_gain = 3.0;
        // EOrig is an energy: G = sqrt(EOrig / (ε + |X|²)).
        let e_target = target_gain * target_gain * (amp * amp + EPS);
        let e_orig = vec![vec![e_target; 4]];
        let q_orig = vec![vec![0.0]];
        let p = params(&b, &lim, &t_e, &t_q, &fr, &e_orig, &q_orig, &[]);
        let x = flat_x_high(amp, 40);
        let mut st = EnvAdjustState::new();
        let y = adjust(&x, &p, &mut st).unwrap();
        // The boost ratio uses the raw energies (no ε), so the exact
        // applied gain is target·sqrt((amp² + ε)/amp²); pin to 1e-3.
        for c in 0..32usize {
            let col = c + T_HF_ADJ;
            for k in 8..16 {
                let g = (y[col][k].norm_sqr() / x[col][k].norm_sqr()).sqrt();
                assert!(
                    (g - target_gain).abs() < 1e-3 * target_gain,
                    "col {col} k {k}: gain {g}"
                );
            }
        }
    }

    /// Per-envelope gains switch exactly at the tE border.
    #[test]
    fn gain_switches_at_envelope_border() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 8, 16];
        let t_q = [0, 8, 16];
        let fr = [true, true];
        let amp = 50.0;
        let e0 = 4.0 * (amp * amp + EPS);
        let e1 = 25.0 * (amp * amp + EPS);
        let e_orig = vec![vec![e0; 4], vec![e1; 4]];
        let q_orig = vec![vec![0.0], vec![0.0]];
        let p = params(&b, &lim, &t_e, &t_q, &fr, &e_orig, &q_orig, &[]);
        let x = flat_x_high(amp, 40);
        let mut st = EnvAdjustState::new();
        let y = adjust(&x, &p, &mut st).unwrap();
        // Slots 0..16 → gain 2; slots 16..32 → gain 5.
        let g_at = |c: usize| {
            let col = c + T_HF_ADJ;
            (y[col][9].norm_sqr() / x[col][9].norm_sqr()).sqrt()
        };
        assert!((g_at(3) - 2.0).abs() < 1e-2);
        assert!((g_at(15) - 2.0).abs() < 1e-2);
        assert!((g_at(16) - 5.0).abs() < 2e-2);
        assert!((g_at(31) - 5.0).abs() < 2e-2);
    }

    /// The limiter clamps a runaway per-subband gain to the
    /// limiter-band average, and the boost compensates the band's
    /// total energy (up to the 1.584893192 cap).
    #[test]
    fn limiter_clamps_and_boost_compensates() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 16];
        let t_q = [0, 16];
        let fr = [true];
        // Band 0 demands a huge gain (XHigh is tiny there), bands 1..4
        // are ordinary. limiter_gains = 1 → limGain = 1.0.
        let amp = 10.0;
        let mut x = flat_x_high(amp, 40);
        for col in x.iter_mut() {
            for cell in &mut col[8..10] {
                *cell = *cell * 1e-6;
            }
        }
        let e_orig = vec![vec![
            400.0 * (amp * amp),
            400.0 * (amp * amp),
            400.0 * (amp * amp),
            400.0 * (amp * amp),
        ]];
        let q_orig = vec![vec![0.0]];
        let mut p = params(&b, &lim, &t_e, &t_q, &fr, &e_orig, &q_orig, &[]);
        p.limiter_gains = 1;
        let mut st = EnvAdjustState::new();
        let y = adjust(&x, &p, &mut st).unwrap();
        // Unclamped G in the dead band would be ≈ 2e7; the limiter-band
        // average cap is far smaller, so the dead band's output stays
        // bounded by GMax·|X| ≪ 1 with boost ≤ MAX_BOOST.
        for c in 0..32usize {
            let col = c + T_HF_ADJ;
            assert!(y[col][8].norm_sqr() < 1.0);
            // The healthy bands keep a finite, boosted gain.
            assert!(y[col][12].norm_sqr().is_finite());
        }
    }

    /// A pure noise band (XHigh = 0, QOrig ≫) synthesises Table 4.A.91
    /// noise at the QM level, and the running index threads across
    /// frames.
    #[test]
    fn noise_floor_synthesis_and_index_threading() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 16];
        let t_q = [0, 16];
        let fr = [true];
        let e_orig = vec![vec![64.0; 4]];
        let q_orig = vec![vec![1.0]]; // QMapped = 1 → QM = sqrt(64/2)
        let p = params(&b, &lim, &t_e, &t_q, &fr, &e_orig, &q_orig, &[]);
        let x = vec![[Complex::default(); 64]; 40];
        let mut st = EnvAdjustState::new();
        let y = adjust(&x, &p, &mut st).unwrap();
        // First sample: fIndexNoise = 0·8 + 0 + 1 = 1.
        let qm = (64.0f64 * 1.0 / 2.0).sqrt();
        // Boost over the limiter band: num = Σ EOrig = 8·64, den =
        // Σ QM² = 8·32 → GBoost = √2 (below the cap).
        let expect = qm * 2.0f64.sqrt();
        let (v_re, v_im) = NOISE_TABLE[1];
        let got = y[T_HF_ADJ][8];
        assert!((got.re - expect * v_re).abs() < 1e-9, "{got:?}");
        assert!((got.im - expect * v_im).abs() < 1e-9);
        // Last index this frame: (31·8 + 7 + 1) mod 512 = 256.
        assert_eq!(st.index_noise, 256);
        // Second frame continues from 256.
        let y2 = adjust(&x, &p, &mut st).unwrap();
        let (v_re2, v_im2) = NOISE_TABLE[257];
        let got2 = y2[T_HF_ADJ][8];
        assert!((got2.re - expect * v_re2).abs() < 1e-9);
        assert!((got2.im - expect * v_im2).abs() < 1e-9);
    }

    /// An additional sinusoid lands in the middle subband of its
    /// high-res band with the [1, 0, −1, 0] / (−1)^(m+kx) pattern and
    /// the cross-frame indexSine advance.
    #[test]
    fn sinusoid_injection_pattern() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 16];
        let t_q = [0, 16];
        let fr = [true];
        let e_orig = vec![vec![64.0; 4]];
        let q_orig = vec![vec![0.0]];
        // Harmonic in high band 1 → mid subband (10 + 12)/2 = 11.
        let add = [false, true, false, false];
        let p = params(&b, &lim, &t_e, &t_q, &fr, &e_orig, &q_orig, &add);
        let x = vec![[Complex::default(); 64]; 40];
        let mut st = EnvAdjustState::new();
        let y = adjust(&x, &p, &mut st).unwrap();
        let s = 64.0f64.sqrt() * MAX_BOOST; // SM boosted (ECurr = 0)
        // m + kx = 11 (odd) → imaginary part sign-flipped.
        // c = 0: φ = (1, 0); c = 1: φ = (0, 1) → im = −s.
        let y0 = y[T_HF_ADJ][11];
        let y1 = y[T_HF_ADJ + 1][11];
        assert!((y0.re - s).abs() < 1e-9 && y0.im.abs() < 1e-12, "{y0:?}");
        assert!(y1.re.abs() < 1e-12 && (y1.im + s).abs() < 1e-9, "{y1:?}");
        // Other bands carry no sinusoid.
        assert_eq!(y[T_HF_ADJ][9], Complex::default());
        // indexSine advances past the frame: (31 % 4 + 1) % 4 = 0.
        assert_eq!(st.index_sine, 0);
        // Next frame: still ringing (prev SIndexMapped carries over)
        // even though l_a stays -1.
        let y2 = adjust(&x, &p, &mut st).unwrap();
        assert!(y2[T_HF_ADJ][11].norm_sqr() > 0.0);
    }

    /// Smoothing mode 0 (hSL = 4) filters a gain step across the
    /// carry, and the second frame consumes the previous tail.
    #[test]
    fn smoothing_carries_across_frames() {
        let b = bands();
        let lim = [8, 16];
        let t_e = [0, 16];
        let t_q = [0, 16];
        let fr = [true];
        let amp = 10.0;
        let x = flat_x_high(amp, 40);
        let e_lo = vec![vec![1.0 * (amp * amp + EPS); 4]];
        let e_hi = vec![vec![100.0 * (amp * amp + EPS); 4]];
        let q_orig = vec![vec![0.0]];
        let mut p1 = params(&b, &lim, &t_e, &t_q, &fr, &e_lo, &q_orig, &[]);
        p1.smoothing_mode = false;
        let mut st = EnvAdjustState::new();
        let _ = adjust(&x, &p1, &mut st).unwrap();
        assert_eq!(st.g_temp_tail.len(), 4);
        // Second frame jumps to gain 10; the first output columns are
        // still pulled down by the smoothing history (gain < 10).
        let mut p2 = params(&b, &lim, &t_e, &t_q, &fr, &e_hi, &q_orig, &[]);
        p2.smoothing_mode = false;
        let y = adjust(&x, &p2, &mut st).unwrap();
        let g0 = (y[T_HF_ADJ][9].norm_sqr() / x[T_HF_ADJ][9].norm_sqr()).sqrt();
        let g_late = (y[T_HF_ADJ + 20][9].norm_sqr() / x[T_HF_ADJ + 20][9].norm_sqr()).sqrt();
        assert!(g0 < 6.0, "g0 = {g0}");
        assert!((g_late - 10.0).abs() < 0.1, "g_late = {g_late}");
    }
}
