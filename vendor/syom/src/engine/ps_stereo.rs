//! PS stereo processing — ISO/IEC 14496-3:2009 §8.6.4.6.
//!
//! Converts the mono hybrid signal `s_k(n)` and its de-correlation
//! `d_k(n)` into left/right hybrid signals through the 2×2 mixing
//!
//! ```text
//! l_k(n) = H11(k,n)·s_k(n) + H21(k,n)·d_k(n)
//! r_k(n) = H12(k,n)·s_k(n) + H22(k,n)·d_k(n)
//! ```
//!
//! Per parameter position (envelope border) the vectors `h11..h22`
//! are derived per stereo band from the dequantized cues:
//!
//! * IID: `c(b) = 10^(iid(b)/20)` on the Table 8.25 (default) or
//!   8.26 (fine) dB grid, read from a const gain table;
//! * ICC: `ρ(b)` on the Table 8.28 grid, driving **mixing procedure
//!   Ra** (`icc_mode 0..2`: scale factors `c1 = √(2/(1+c²))`,
//!   `c2 = √2·c/√(1+c²)`, rotation `α = ½·arccos(ρ)`,
//!   `β = α·(c1−c2)/√2`) or **Rb** (`icc_mode 3..5`: `ρ` floored at
//!   0.05, `α = ½·arctan(2cρ/(c²−1))` with the `c = 1` and
//!   modulo-π/2 corrections, `μ`/`γ` per §8.6.4.6.2.2);
//! * IPD/OPD (§8.6.4.6.3.2, when enabled): the three-position
//!   smoothing `φ = ∠(¼e^(j·prev2) + ½e^(j·prev1) + e^(j·cur))` on
//!   the Table 8.31 `π/4` ladder, applied as `e^(jφ1)` on `h11/h21`
//!   and `e^(jφ2)` (`φ2 = φ_opd − φ_ipd`) on `h12/h22`; the
//!   `*`-marked negative-frequency hybrid channels take the complex
//!   conjugate.
//!
//! Between borders the four H matrices are linearly interpolated
//! (§8.6.4.6.4), the first region interpolating from the previous
//! frame's final coefficients (zeros on the very first frame), the
//! region after the last border holding. FIX_BORDERS positions are
//! `⌊32·(e+1)/num_env⌋ − 1`; VAR_BORDERS come from the bitstream.
//! `num_env == 0` holds the previous frame's coefficients for the
//! whole frame (§8.6.4.6.5). A stereo-band-count switch (Table 8.47)
//! re-maps the retained coefficients through Tables 8.45 / 8.46.
//!
//! All truth from ISO/IEC 14496-3:2009 §8.6.4.6 staged under
//! `docs/audio/aac/`.

use crate::engine::ps_data::{PsData, PsIndices};
use crate::engine::ps_hybrid::{HybridConfig, NUM_QMF_SLOTS};
use crate::engine::ps_map::{
    MAP_20_TO_34, MAP_34_TO_20, conjugate_flags, map_indices, parameter_map,
};
use crate::engine::sbr_qmf::Complex;
use crate::engine::{Error, Result};

/// Table 8.25 — default IID quantization grid, dB, index −7..7.
const IID_DB_COARSE: [f64; 15] = [
    -25.0, -18.0, -14.0, -10.0, -7.0, -4.0, -2.0, 0.0, 2.0, 4.0, 7.0, 10.0, 14.0, 18.0, 25.0,
];

/// Table 8.26 — fine IID quantization grid, dB, index −15..15.
const IID_DB_FINE: [f64; 31] = [
    -50.0, -45.0, -40.0, -35.0, -30.0, -25.0, -22.0, -19.0, -16.0, -13.0, -10.0, -8.0, -6.0, -4.0,
    -2.0, 0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 13.0, 16.0, 19.0, 22.0, 25.0, 30.0, 35.0, 40.0, 45.0,
    50.0,
];

/// `10^(dB/20)` for one IID grid, from `exp10_f64`.
const fn iid_gains<const N: usize>(db: &[f64; N]) -> [f64; N] {
    let mut g = [0.0; N];
    let mut i = 0;
    while i < N {
        g[i] = super::det_math::exp10_f64(db[i] / 20.0);
        i += 1;
    }
    g
}

/// Table 8.25 as linear gains `c = 10^(dB/20)`.
const IID_GAIN_COARSE: [f64; 15] = iid_gains(&IID_DB_COARSE);
/// Table 8.26 as linear gains `c = 10^(dB/20)`.
const IID_GAIN_FINE: [f64; 31] = iid_gains(&IID_DB_FINE);

/// Table 8.28 — ICC quantization grid `ρ`.
const ICC_RHO: [f64; 8] = [1.0, 0.937, 0.84118, 0.60092, 0.36764, 0.0, -0.589, -1.0];

/// One stereo band's mixing coefficients.
type H4 = [Complex; 4]; // h11, h12, h21, h22

/// A stereo pair of hybrid-domain frames.
#[cfg(test)]
pub type HybridPair = (Vec<Vec<Complex>>, Vec<Vec<Complex>>);

/// §8.6.4.6 stereo processor: per-band coefficient state across
/// frames plus the IPD/OPD smoothing history.
#[derive(Debug, Clone)]
pub struct PsStereo {
    /// Stereo band count in force (20 or 34).
    n_bands: usize,
    /// `H(·, n_{−1})` — coefficients at the previous frame's last
    /// slot, per stereo band.
    h_prev: Vec<H4>,
    /// IPD/OPD angle history: `[.., e−1]` and `[.., e]` positions
    /// (radians), per stereo band.
    ipd_hist: [Vec<f64>; 2],
    opd_hist: [Vec<f64>; 2],
    h_from: Vec<H4>,
    h_to: Vec<H4>,
    /// Per-slot mixing coefficients of the current frame: 70 KiB, kept
    /// off the stack (TASK-118).
    h_slots: Vec<[H4; 34]>,
}

impl PsStereo {
    /// Fresh state (first frame interpolates from zero coefficients).
    #[must_use]
    pub fn new(n_bands: usize) -> Self {
        PsStereo {
            n_bands,
            h_prev: vec![[Complex::default(); 4]; n_bands],
            ipd_hist: [vec![0.0; n_bands], vec![0.0; n_bands]],
            opd_hist: [vec![0.0; n_bands], vec![0.0; n_bands]],
            h_from: Vec::new(),
            h_to: Vec::new(),
            h_slots: Vec::new(),
        }
    }

    fn fit_h_tmp(&mut self) {
        let nb = self.n_bands;
        if self.h_from.len() != nb {
            self.h_from.clear();
            self.h_from.resize(nb, [Complex::default(); 4]);
        }
        if self.h_to.len() != nb {
            self.h_to.clear();
            self.h_to.resize(nb, [Complex::default(); 4]);
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn fill_h_slots(
        &mut self,
        ps: &PsData,
        idx: &PsIndices,
        h_slots: &mut [[H4; 34]],
    ) -> Result<()> {
        self.fit_h_tmp();
        let nb = self.n_bands;
        if ps.num_env == 0 {
            for slot in h_slots.iter_mut() {
                slot[..nb].copy_from_slice(&self.h_prev);
            }
            return Ok(());
        }
        let mut borders = [0usize; 8];
        let n_env = ps.num_env.min(8);
        if ps.frame_class {
            for (i, &b) in ps.border_position.iter().take(n_env).enumerate() {
                borders[i] = usize::from(b).min(NUM_QMF_SLOTS - 1);
            }
        } else {
            for e in 0..n_env {
                borders[e] = NUM_QMF_SLOTS * (e + 1) / ps.num_env - 1;
            }
        }
        self.h_from.clone_from(&self.h_prev);
        let mut n_from: isize = -1;
        for e in 0..n_env {
            let n_e = borders[e];
            self.fill_envelope_h(ps, idx, e)?;
            let (den, base) = if e == 0 {
                ((n_e + 1).max(1) as f64, -1isize)
            } else {
                (((n_e as isize - n_from).max(1)) as f64, n_from)
            };
            let lo = ((n_from + 1).max(0)) as usize;
            let hi = n_e.min(NUM_QMF_SLOTS - 1);
            for n in lo..=hi {
                let t = (n as isize - base) as f64 / den;
                let slot = &mut h_slots[n];
                for b in 0..nb {
                    for c in 0..4 {
                        slot[b][c] = self.h_from[b][c] + (self.h_to[b][c] - self.h_from[b][c]) * t;
                    }
                }
            }
            self.h_from.clone_from(&self.h_to);
            n_from = n_e as isize;
        }
        let lo = ((n_from + 1).max(0)) as usize;
        for slot in h_slots.iter_mut().skip(lo) {
            slot[..nb].copy_from_slice(&self.h_from);
        }
        self.h_prev.clone_from(&self.h_from);
        Ok(())
    }

    /// Mix hybrid `s`/`d` into reused QMF output (no hybrid L/R matrices).
    #[allow(clippy::too_many_arguments)]
    pub fn mix_to_qmf(
        &mut self,
        ps: &PsData,
        idx: &PsIndices,
        config: HybridConfig,
        s: &[Vec<Complex>],
        d: &[Vec<Complex>],
        l_qmf: &mut [[Complex; 64]],
        r_qmf: &mut [[Complex; 64]],
    ) -> Result<()> {
        let nb = self.n_bands;
        let expected = match config {
            HybridConfig::Bands1020 => 20,
            HybridConfig::Bands34 => 34,
        };
        if expected != nb || s.len() != NUM_QMF_SLOTS || d.len() != NUM_QMF_SLOTS {
            return Err(Error::PsDataInvalid);
        }
        let b_k = parameter_map(config);
        let conj_k = conjugate_flags(config);
        let nr_hyb = config.nr_bands();
        if s.iter().chain(d.iter()).any(|row| row.len() != nr_hyb)
            || l_qmf.len() < NUM_QMF_SLOTS
            || r_qmf.len() < NUM_QMF_SLOTS
        {
            return Err(Error::PsDataInvalid);
        }
        let mut h_slots = std::mem::take(&mut self.h_slots);
        h_slots.clear();
        h_slots.resize(NUM_QMF_SLOTS, [[Complex::default(); 4]; 34]);
        let filled = self.fill_h_slots(ps, idx, &mut h_slots);
        if let Err(e) = filled {
            self.h_slots = h_slots;
            return Err(e);
        }
        let mut lrow = [Complex::default(); 91];
        let mut rrow = [Complex::default(); 91];
        for n in 0..NUM_QMF_SLOTS {
            for k in 0..nr_hyb {
                let b = usize::from(b_k[k]);
                let mut h = h_slots[n][b];
                if conj_k.contains(&k) {
                    for c in h.iter_mut() {
                        *c = c.conj();
                    }
                }
                lrow[k] = h[0] * s[n][k] + h[2] * d[n][k];
                rrow[k] = h[1] * s[n][k] + h[3] * d[n][k];
            }
            crate::engine::ps_hybrid::synthesize_slot(config, &lrow[..nr_hyb], &mut l_qmf[n]);
            crate::engine::ps_hybrid::synthesize_slot(config, &rrow[..nr_hyb], &mut r_qmf[n]);
        }
        self.h_slots = h_slots;
        Ok(())
    }

    /// Table 8.47 — switch the stereo band count, re-mapping the
    /// retained coefficients through Table 8.45 / 8.46 and resetting
    /// the phase-smoothing history.
    pub fn switch_bands(&mut self, n_bands: usize) {
        if n_bands == self.n_bands {
            return;
        }
        let map = |vals: Vec<f64>| -> Vec<f64> {
            if n_bands == 34 {
                MAP_20_TO_34.iter().map(|m| m.apply_f64(&vals)).collect()
            } else {
                MAP_34_TO_20.iter().map(|m| m.apply_f64(&vals)).collect()
            }
        };
        let mut new_h = vec![[Complex::default(); 4]; n_bands];
        for c in 0..4 {
            let re: Vec<f64> = self.h_prev.iter().map(|h| h[c].re).collect();
            let im: Vec<f64> = self.h_prev.iter().map(|h| h[c].im).collect();
            let re = map(re);
            let im = map(im);
            for (b, h) in new_h.iter_mut().enumerate() {
                h[c] = Complex::new(re[b], im[b]);
            }
        }
        self.h_prev = new_h;
        self.n_bands = n_bands;
        self.ipd_hist = [vec![0.0; n_bands], vec![0.0; n_bands]];
        self.opd_hist = [vec![0.0; n_bands], vec![0.0; n_bands]];
    }

    /// Process one stereo frame: mix `s` (mono hybrid) and `d`
    /// (de-correlated hybrid) into `(l, r)` hybrid signals per the
    /// resolved parameters. `config` must agree with `n_bands`.
    #[cfg(test)]
    pub fn process(
        &mut self,
        ps: &PsData,
        idx: &PsIndices,
        config: HybridConfig,
        s: &[Vec<Complex>],
        d: &[Vec<Complex>],
    ) -> Result<HybridPair> {
        let nb = self.n_bands;
        let expected = match config {
            HybridConfig::Bands1020 => 20,
            HybridConfig::Bands34 => 34,
        };
        if expected != nb || s.len() != NUM_QMF_SLOTS || d.len() != NUM_QMF_SLOTS {
            return Err(Error::PsDataInvalid);
        }
        let b_k = parameter_map(config);
        let conj_k = conjugate_flags(config);
        let nr_hyb = config.nr_bands();
        if s.iter().chain(d.iter()).any(|row| row.len() != nr_hyb) {
            return Err(Error::PsDataInvalid);
        }

        let mut h_slots = std::mem::take(&mut self.h_slots);
        h_slots.clear();
        h_slots.resize(NUM_QMF_SLOTS, [[Complex::default(); 4]; 34]);
        let filled = self.fill_h_slots(ps, idx, &mut h_slots);
        if let Err(e) = filled {
            self.h_slots = h_slots;
            return Err(e);
        }

        // Mix.
        let mut l = vec![vec![Complex::default(); nr_hyb]; NUM_QMF_SLOTS];
        let mut r = vec![vec![Complex::default(); nr_hyb]; NUM_QMF_SLOTS];
        for n in 0..NUM_QMF_SLOTS {
            for k in 0..nr_hyb {
                let b = usize::from(b_k[k]);
                let mut h = h_slots[n][b];
                if conj_k.contains(&k) {
                    for c in h.iter_mut() {
                        *c = c.conj();
                    }
                }
                l[n][k] = h[0] * s[n][k] + h[2] * d[n][k];
                r[n][k] = h[1] * s[n][k] + h[3] * d[n][k];
            }
        }
        self.h_slots = h_slots;
        Ok((l, r))
    }

    /// Derive `h11..h22` per stereo band for envelope `e`
    /// (§8.6.4.6.2 + §8.6.4.6.3), advancing the phase history.
    fn fill_envelope_h(&mut self, ps: &PsData, idx: &PsIndices, e: usize) -> Result<()> {
        let nb = self.n_bands;
        if self.h_to.len() != nb {
            self.h_to.clear();
            self.h_to.resize(nb, [Complex::default(); 4]);
        }

        // Map the parameter vectors to the stereo band count; a
        // disabled parameter kind is index 0 (§8.5.2 defaults).
        let iid = match idx.iid.get(e) {
            Some(v) => map_indices(v, nb),
            None => vec![0; nb],
        };
        let icc = match idx.icc.get(e) {
            Some(v) => map_indices(v, nb),
            None => vec![0; nb],
        };
        if iid.len() != nb || icc.len() != nb {
            return Err(Error::PsDataInvalid);
        }

        let fine = ps.config.iid_quant_fine();
        let rb = ps.config.icc_mode >= 3;

        for b in 0..nb {
            let c = if fine {
                *IID_GAIN_FINE
                    .get((iid[b] + 15) as usize)
                    .ok_or(Error::PsDataInvalid)?
            } else {
                *IID_GAIN_COARSE
                    .get((iid[b] + 7) as usize)
                    .ok_or(Error::PsDataInvalid)?
            };
            let rho = *ICC_RHO.get(icc[b] as usize).ok_or(Error::PsDataInvalid)?;

            let (h11, h12, h21, h22) = if rb { mix_rb(c, rho) } else { mix_ra(c, rho) };
            self.h_to[b] = [
                Complex::new(h11, 0.0),
                Complex::new(h12, 0.0),
                Complex::new(h21, 0.0),
                Complex::new(h22, 0.0),
            ];
        }

        if ps.enable_ipdopd {
            // Zero-extended, band-count-mapped phase indices.
            let nr = ps.config.nr_ipdopd_par();
            let native = if ps.config.iid_mode.is_multiple_of(3) {
                10
            } else if ps.config.iid_mode % 3 == 1 {
                20
            } else {
                34
            };
            let extend = |v: Option<&Vec<i32>>| -> Vec<f64> {
                let mut full = vec![0i32; native];
                if let Some(v) = v {
                    full[..nr.min(v.len())].copy_from_slice(&v[..nr.min(v.len())]);
                }
                map_indices(&full, nb)
                    .iter()
                    .map(|&i| f64::from(i) * core::f64::consts::FRAC_PI_4)
                    .collect()
            };
            let ipd_cur = extend(idx.ipd.get(e));
            let opd_cur = extend(idx.opd.get(e));
            for b in 0..nb {
                let sm = |h: &[Vec<f64>; 2], cur: f64| -> f64 {
                    let mut acc = Complex::default();
                    for (w, ang) in [(0.25, h[0][b]), (0.5, h[1][b]), (1.0, cur)] {
                        let (si, co) = super::det_math::sincos_f64(ang);
                        acc += Complex::new(co * w, si * w);
                    }
                    super::det_math::atan2_f64(acc.im, acc.re)
                };
                let phi_opd = sm(&self.opd_hist, opd_cur[b]);
                let phi_ipd = sm(&self.ipd_hist, ipd_cur[b]);
                let phi1 = phi_opd;
                let phi2 = phi_opd - phi_ipd;
                let (s1, c1) = super::det_math::sincos_f64(phi1);
                let (s2, c2) = super::det_math::sincos_f64(phi2);
                let r1 = Complex::new(c1, s1);
                let r2 = Complex::new(c2, s2);
                self.h_to[b][0] = self.h_to[b][0] * r1;
                self.h_to[b][2] = self.h_to[b][2] * r1;
                self.h_to[b][1] = self.h_to[b][1] * r2;
                self.h_to[b][3] = self.h_to[b][3] * r2;
            }
            // Advance the history.
            self.ipd_hist[0] = core::mem::take(&mut self.ipd_hist[1]);
            self.ipd_hist[1] = ipd_cur;
            self.opd_hist[0] = core::mem::take(&mut self.opd_hist[1]);
            self.opd_hist[1] = opd_cur;
        }
        Ok(())
    }
}

/// §8.6.4.6.2.1 mixing procedure Ra.
fn mix_ra(c: f64, rho: f64) -> (f64, f64, f64, f64) {
    let denom = (1.0 + c * c).sqrt();
    let c1 = core::f64::consts::SQRT_2 / denom;
    let c2 = core::f64::consts::SQRT_2 * c / denom;
    let alpha = 0.5 * super::det_math::acos_f64(rho.clamp(-1.0, 1.0));
    let beta = alpha * (c1 - c2) / core::f64::consts::SQRT_2;
    let (s_sum, c_sum) = super::det_math::sincos_f64(alpha + beta);
    let (s_diff, c_diff) = super::det_math::sincos_f64(beta - alpha);
    (c_sum * c2, c_diff * c1, s_sum * c2, s_diff * c1)
}

/// §8.6.4.6.2.2 mixing procedure Rb.
fn mix_rb(c: f64, rho: f64) -> (f64, f64, f64, f64) {
    let rho = rho.max(0.05);
    let mut alpha = if (c - 1.0).abs() < 1e-12 {
        core::f64::consts::FRAC_PI_4
    } else {
        0.5 * super::det_math::atan_f64(2.0 * c * rho / (c * c - 1.0))
    };
    // `½·atan` lands in (−π/4, π/4); one shift puts a negative angle in [0, π/2).
    if alpha < 0.0 {
        alpha += core::f64::consts::FRAC_PI_2;
    }
    let mu = 1.0 + (4.0 * rho * rho - 4.0) / (c + 1.0 / c).powi(2);
    let gamma = super::det_math::atan_f64(((1.0 - mu) / (1.0 + mu)).max(0.0).sqrt());
    let (sa, ca) = super::det_math::sincos_f64(alpha);
    let (sg, cg) = super::det_math::sincos_f64(gamma);
    let s2 = core::f64::consts::SQRT_2;
    (s2 * ca * cg, s2 * sa * cg, -s2 * sa * sg, s2 * ca * sg)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::engine::bits::{BitReader, BitWriter};
    use crate::engine::ps_data::PsIndexState;

    /// Build a one-envelope FIX ps_data with the given uniform IID /
    /// ICC index (coarse grid, 10 pars each) and resolve it.
    fn ps_with(iid_idx: i32, icc_idx: i32) -> (PsData, PsIndices) {
        let mut w = BitWriter::new();
        w.write_bit(true); // header
        w.write_bit(true); // enable_iid
        w.write_u32(0, 3); // iid_mode 0
        w.write_bit(true); // enable_icc
        w.write_u32(0, 3); // icc_mode 0
        w.write_bit(false); // enable_ext
        w.write_bit(false); // FIX
        w.write_u32(1, 2); // num_env = 1
        w.write_bit(false); // iid freq
        for b in 0..10 {
            // First band carries the index, the rest delta 0.
            let (len, code) = crate::engine::ps_huffman::HUFF_IID_DF[(iid_idx + 14) as usize];
            if b == 0 {
                w.write_u32(code, u32::from(len));
            } else {
                let (l0, c0) = crate::engine::ps_huffman::HUFF_IID_DF[14];
                w.write_u32(c0, u32::from(l0));
            }
        }
        w.write_bit(false); // icc freq
        for b in 0..10 {
            let (len, code) = crate::engine::ps_huffman::HUFF_ICC_DF[(icc_idx + 7) as usize];
            if b == 0 {
                w.write_u32(code, u32::from(len));
            } else {
                let (l0, c0) = crate::engine::ps_huffman::HUFF_ICC_DF[7];
                w.write_u32(c0, u32::from(l0));
            }
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let ps = PsData::parse(&mut r, None).unwrap().unwrap();
        let mut st = PsIndexState::default();
        let idx = ps.resolve(&mut st).unwrap();
        (ps, idx)
    }

    fn ones(nr: usize) -> Vec<Vec<Complex>> {
        (0..NUM_QMF_SLOTS)
            .map(|_| vec![Complex::new(1.0, 0.0); nr])
            .collect()
    }

    fn zeros(nr: usize) -> Vec<Vec<Complex>> {
        (0..NUM_QMF_SLOTS)
            .map(|_| vec![Complex::default(); nr])
            .collect()
    }

    /// ICC = 1 (index 0) makes α = β = 0: the mix is pure IID panning
    /// `l = c2·s`, `r = c1·s`, `d` unused. Pin the exact §8.6.4.6.2.1
    /// scale factors at the last slot (interpolation complete).
    #[test]
    fn pure_iid_panning_matches_scale_factors() {
        let config = HybridConfig::Bands1020;
        let (ps, idx) = ps_with(7, 0); // +25 dB
        let mut st = PsStereo::new(20);
        let s = ones(config.nr_bands());
        let d = zeros(config.nr_bands());
        let (l, r) = st.process(&ps, &idx, config, &s, &d).unwrap();
        let c = 10f64.powf(25.0 / 20.0);
        let c1 = core::f64::consts::SQRT_2 / (1.0 + c * c).sqrt();
        let c2 = c * c1;
        // Hybrid channel 20 (stereo band 16), final slot: h fully
        // interpolated to the envelope value.
        let n = NUM_QMF_SLOTS - 1;
        assert!((l[n][20].re - c2).abs() < 1e-12, "{} vs {c2}", l[n][20].re);
        assert!((r[n][20].re - c1).abs() < 1e-12);
        assert!(l[n][20].im.abs() < 1e-15 && r[n][20].im.abs() < 1e-15);
        // Left is 25 dB louder.
        let ratio = 20.0 * (l[n][20].re / r[n][20].re).log10();
        assert!((ratio - 25.0).abs() < 1e-9);
    }

    /// ICC = −1 (index 7) with IID 0: α = π/2, the channels are the
    /// anti-phase de-correlated pair `l = d`, `r = −d` (§8.6.4.6.2.1).
    #[test]
    fn full_anticorrelation_uses_decorrelated_signal() {
        let config = HybridConfig::Bands1020;
        let (ps, idx) = ps_with(0, 7);
        let mut st = PsStereo::new(20);
        let s = ones(config.nr_bands());
        let d = ones(config.nr_bands());
        let (l, r) = st.process(&ps, &idx, config, &s, &d).unwrap();
        let n = NUM_QMF_SLOTS - 1;
        // h11 = cos(π/2) = 0, h21 = sin(π/2) = 1 → l = d.
        assert!((l[n][20].re - 1.0).abs() < 1e-12);
        // h12 = cos(−π/2) = 0, h22 = sin(−π/2) = −1 → r = −d.
        assert!((r[n][20].re + 1.0).abs() < 1e-12);
    }

    /// The first region interpolates from zero (fresh state) to the
    /// envelope coefficients linearly in (n + 1)/(n_0 + 1) — the
    /// "previous" position is n_{−1} = −1, one slot before the frame.
    #[test]
    fn first_region_interpolates_from_zero() {
        let config = HybridConfig::Bands1020;
        let (ps, idx) = ps_with(0, 0); // IID 0 dB, ICC 1 → h11 = h12 = 1
        let mut st = PsStereo::new(20);
        let s = ones(config.nr_bands());
        let d = zeros(config.nr_bands());
        let (l, _r) = st.process(&ps, &idx, config, &s, &d).unwrap();
        // num_env = 1, FIX → n_0 = 31; H(n) = (n + 1)/32 · h.
        for (n, row) in l.iter().enumerate() {
            let expect = (n + 1) as f64 / 32.0;
            assert!(
                (row[20].re - expect).abs() < 1e-12,
                "slot {n}: {} vs {expect}",
                row[20].re
            );
        }
        // A second identical frame is flat at the full value.
        let (l2, _) = st.process(&ps, &idx, config, &s, &d).unwrap();
        for row in &l2 {
            assert!((row[20].re - 1.0).abs() < 1e-12);
        }
    }

    /// num_env = 0 holds the previous coefficients for the whole
    /// frame.
    #[test]
    fn zero_envelopes_hold_previous_coefficients() {
        let config = HybridConfig::Bands1020;
        let (ps, idx) = ps_with(7, 0);
        let mut st = PsStereo::new(20);
        let s = ones(config.nr_bands());
        let d = zeros(config.nr_bands());
        let _ = st.process(&ps, &idx, config, &s, &d).unwrap();
        // Hold frame: num_env = 0 (frame_class FIX, num_env_idx 0).
        let mut hold = ps.clone();
        hold.num_env = 0;
        let empty = PsIndices::default();
        let (l, r) = st.process(&hold, &empty, config, &s, &d).unwrap();
        let c = 10f64.powf(25.0 / 20.0);
        let c1 = core::f64::consts::SQRT_2 / (1.0 + c * c).sqrt();
        for row in &l {
            assert!((row[20].re - c * c1).abs() < 1e-12);
        }
        for row in &r {
            assert!((row[20].re - c1).abs() < 1e-12);
        }
    }

    /// Rb at c = 1, ρ = 1: α = π/4, μ = 1, γ = 0 → an energy-
    /// preserving 45° rotation of the mono signal (`h11 = h12 = 1`,
    /// `h21 = h22 = 0`).
    #[test]
    fn rb_identity_point() {
        let (h11, h12, h21, h22) = mix_rb(1.0, 1.0);
        assert!((h11 - 1.0).abs() < 1e-12);
        assert!((h12 - 1.0).abs() < 1e-12);
        assert!(h21.abs() < 1e-12);
        assert!(h22.abs() < 1e-12);
    }

    /// Ra preserves total energy: |h11|² + |h12|² + |h21|² + |h22|²
    /// = c1² + c2² = 2 for every cue combination.
    #[test]
    fn ra_energy_invariant() {
        for iid in -7..=7 {
            for &rho in &ICC_RHO {
                let c = IID_GAIN_COARSE[(iid + 7) as usize];
                let (a, b, x, y) = mix_ra(c, rho);
                let e = a * a + b * b + x * x + y * y;
                assert!((e - 2.0).abs() < 1e-12, "iid {iid} rho {rho}: {e}");
            }
        }
    }

    /// Each tabulated gain is the exp10 series (relative error 0) and
    /// within 1e-12 of `10^(dB/20)`.
    #[test]
    fn iid_gain_tables_match_exp10_series() {
        let ln2 = f64::from_bits(0x3FE6_2E42_FEFA_39EF);
        let log2_10 = f64::from_bits(0x400A_934F_0979_A371);
        let series = |db: f64| -> f64 {
            let y = (db / 20.0) * log2_10;
            let n = y.floor() as i32;
            let r = (y - f64::from(n)) * ln2;
            let mut p = 1.0;
            let mut k = 16i32;
            while k >= 1 {
                p = 1.0 + r * p / f64::from(k);
                k -= 1;
            }
            let mut scale = 1.0;
            let mut i = n;
            if i > 0 {
                while i > 0 {
                    scale *= 2.0;
                    i -= 1;
                }
            } else {
                while i < 0 {
                    scale *= 0.5;
                    i += 1;
                }
            }
            scale * p
        };
        let check = |db: &[f64], gain: &[f64]| {
            for (&db, &g) in db.iter().zip(gain) {
                let s = series(db);
                assert_eq!(g.to_bits(), s.to_bits(), "{db} dB series");
                let real = (db / 20.0 * std::f64::consts::LOG2_10).exp2();
                let rel = ((g - real) / real).abs();
                assert!(rel < 1e-12, "{db} dB rel {rel:e}");
            }
        };
        check(&IID_DB_COARSE, &IID_GAIN_COARSE);
        check(&IID_DB_FINE, &IID_GAIN_FINE);
        assert_eq!(IID_GAIN_COARSE[7], 1.0);
        assert_eq!(IID_GAIN_FINE[15], 1.0);
        let sqrt10 = 10f64.sqrt();
        assert!(((IID_GAIN_COARSE[11] - sqrt10) / sqrt10).abs() < 1e-12);
        assert!(((IID_GAIN_FINE[2] - 0.01) / 0.01).abs() < 1e-12);
        assert!(((IID_GAIN_FINE[28] - 100.0) / 100.0).abs() < 1e-12);
    }
}
