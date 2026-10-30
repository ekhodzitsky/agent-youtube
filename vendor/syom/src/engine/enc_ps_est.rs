//! Parametric-stereo analysis for the HE v2 encoder (TASK-91): stereo in,
//! one mono downmix plus per-frame IID / ICC indices out. No payload
//! writing here (TASK-92) and no public wiring (TASK-93).
//!
//! Mode: the baseline profile — 20 stereo bands, coarse IID (±7), ICC
//! (0..=7), one envelope per 32-slot frame, no IPD/OPD. Fine IID
//! (±15, Table 8.26) is opt-in via [`PsAnalysis::set_iid_fine`];
//! the product path stays coarse.
//!
//! Analysis runs on the 64-band encoder QMF ([`EncAnalysisQmf`]) of L and
//! R. QMF bands 0–2 are split by the §8.6.4.3 hybrid filters (Type A ×8
//! merged to 6, Type B ×2, ×2), built from `det_math` so the parameters —
//! and the bytes they become — are platform-deterministic. The 13-tap
//! filters are centred, so the estimator lags its input by
//! [`PS_DELAY_SLOTS`] slots; every band is delayed alike.
//!
//! Per stereo band `b` over a frame (plus a quarter of the previous
//! frame's sums, for stability): `e_L`, `e_R`, `Re Σ l·r*`;
//! `IID = 10·log10(e_L / e_R)`, `ICC = Re / √(e_L·e_R)`, each quantized
//! to the nearest grid value (Tables 8.25 / 8.28).
//!
//! Downmix ([`PsDownmix`]): `m = g·(l + r)/2` per 2048-sample block with
//! `g = √((E_L + E_R) / (2·E_M))` limited to `[1, 2]` and to the block's
//! headroom, ramped linearly from the previous block. Correlated content
//! keeps `g = 1`; decorrelated ambience gets its 3 dB back; anti-phase
//! content is the irreversible loss case (`g` saturates at 2 and the
//! estimator reports `ICC = −1`).

use super::det_math;
use super::enc_sbr_qmf::{BANDS, EncAnalysisQmf, EncSlot};
use super::error::Result;
use super::ps_hybrid::HybridConfig;

/// Stereo parameter bands of the baseline mode.
pub(crate) const PS_BANDS: usize = 20;
/// Slots per PS frame (one SBR frame at 1024 framing).
pub(crate) const PS_FRAME_SLOTS: usize = 32;
/// Estimator lag behind its input, in QMF slots (the hybrid half-length).
pub(crate) const PS_DELAY_SLOTS: usize = 6;
/// The decoder's PS frame starts 8 slots before the block boundary the
/// SBR estimator window uses (6 slots of grid lead + `tHFAdj` = 2), so
/// the first analysed frame is this much shorter and frame `k` covers
/// input slots `[32k − 8, 32k + 24)` — the PS frame of access unit `k + 1`.
pub(crate) const PS_FRAME_LEAD: usize = 8;
/// Output-rate samples per downmix block (one HE access unit).
pub(crate) const PS_BLOCK: usize = 2048;

/// Table 8.25 coarse IID grid, dB, index −7..=7.
const IID_DB: [f32; 15] = [
    -25.0, -18.0, -14.0, -10.0, -7.0, -4.0, -2.0, 0.0, 2.0, 4.0, 7.0, 10.0, 14.0, 18.0, 25.0,
];
/// Table 8.26 fine IID grid, dB, index −15..=15 (`iid_mode` 3..=5).
const IID_DB_FINE: [f32; 31] = [
    -50.0, -45.0, -40.0, -35.0, -30.0, -25.0, -22.0, -19.0, -16.0, -13.0, -10.0, -8.0, -6.0, -4.0,
    -2.0, 0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 13.0, 16.0, 19.0, 22.0, 25.0, 30.0, 35.0, 40.0, 45.0,
    50.0,
];
/// Table 8.28 ICC grid, index 0..=7.
const ICC_RHO: [f32; 8] = [1.0, 0.937, 0.84118, 0.60092, 0.36764, 0.0, -0.589, -1.0];
/// Table 8.36 — `g⁰[n]`, QMF band 0, Q = 8.
const G0: [f32; 13] = [
    0.007_460_829_5,
    0.022_704_21,
    0.045_468_66,
    0.072_661_14,
    0.098_851_086,
    0.117_937_11,
    0.125,
    0.117_937_11,
    0.098_851_086,
    0.072_661_14,
    0.045_468_66,
    0.022_704_21,
    0.007_460_829_5,
];
/// Table 8.37 — `g^{1,2}[n]`, QMF bands 1–2, Q = 2.
const G12: [f32; 13] = [
    0.0,
    0.018_994_875,
    0.0,
    -0.072_931_39,
    0.0,
    0.305_966_3,
    0.5,
    0.305_966_3,
    0.0,
    -0.072_931_39,
    0.0,
    0.018_994_875,
    0.0,
];
/// Energy below which a band carries no usable cue (−90 dBFS-ish per slot).
const SILENCE: f64 = 1e-9;
/// Share of the previous frame's sums kept in the estimate.
const CARRY: f64 = 0.25;

/// One frame's quantized parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PsFrameParams {
    /// IID index per band: coarse −7..=7, or fine −15..=15.
    pub iid: [i8; PS_BANDS],
    /// ICC index per band, 0..=7 (0 = fully correlated).
    pub icc: [u8; PS_BANDS],
}

#[derive(Clone, Copy, Default)]
struct Sums {
    l: [f64; PS_BANDS],
    r: [f64; PS_BANDS],
    re: [f64; PS_BANDS],
}

/// IID / ICC estimator over QMF slot pairs.
pub(crate) struct PsEstimator {
    /// Last 13 slots of L and R, oldest first.
    ring: Vec<(EncSlot, EncSlot)>,
    /// `exp(jπk/8)`, k = 0..16.
    tw: [(f32, f32); 16],
    cur: Sums,
    prev: Sums,
    slots: usize,
    primed: usize,
    /// Table 8.26 instead of Table 8.25.
    iid_fine: bool,
}

impl PsEstimator {
    pub(crate) fn new() -> Self {
        let mut tw = [(0.0f32, 0.0f32); 16];
        for (k, t) in tw.iter_mut().enumerate() {
            let (s, c) = det_math::sincos(k as f32 * std::f32::consts::PI / 8.0);
            *t = (c, s);
        }
        Self {
            ring: Vec::with_capacity(13),
            tw,
            cur: Sums::default(),
            prev: Sums::default(),
            slots: PS_FRAME_LEAD,
            primed: 0,
            iid_fine: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.ring.clear();
        self.cur = Sums::default();
        self.prev = Sums::default();
        self.slots = PS_FRAME_LEAD;
        self.primed = 0;
    }

    /// Push one L/R slot pair. Every 32 analysed slots yield the frame's
    /// parameters; the first frame completes [`PS_DELAY_SLOTS`] slots late.
    pub(crate) fn push(&mut self, l: EncSlot, r: EncSlot) -> Option<PsFrameParams> {
        if self.ring.len() == 13 {
            self.ring.remove(0);
        }
        self.ring.push((l, r));
        // Zero history before the stream: the centre is 6 slots back.
        self.primed += 1;
        if self.primed <= PS_DELAY_SLOTS {
            return None;
        }
        self.accumulate();
        self.slots += 1;
        if self.slots < PS_FRAME_SLOTS {
            return None;
        }
        self.slots = 0;
        let out = self.quantize();
        self.prev = self.cur;
        self.cur = Sums::default();
        Some(out)
    }

    /// Band sums of the last completed frame (L, R) — diagnostics.
    #[cfg(test)]
    pub(crate) fn last_energies(&self) -> ([f64; PS_BANDS], [f64; PS_BANDS]) {
        (self.prev.l, self.prev.r)
    }

    /// Input slot `newest − m` (zeros before the stream start).
    fn tap(&self, m: usize, band: usize) -> ((f32, f32), (f32, f32)) {
        let n = self.ring.len();
        if m >= n {
            return ((0.0, 0.0), (0.0, 0.0));
        }
        let (l, r) = &self.ring[n - 1 - m];
        ((l.re[band], l.im[band]), (r.re[band], r.im[band]))
    }

    /// Hybrid channels of the slot 6 back, summed into the band sums.
    fn accumulate(&mut self) {
        let map = super::ps_map::parameter_map(HybridConfig::Bands1020);
        let add = |sums: &mut Sums, b: usize, l: (f32, f32), r: (f32, f32)| {
            let (lr, li, rr, ri) = (
                f64::from(l.0),
                f64::from(l.1),
                f64::from(r.0),
                f64::from(r.1),
            );
            sums.l[b] += lr * lr + li * li;
            sums.r[b] += rr * rr + ri * ri;
            sums.re[b] += lr * rr + li * ri;
        };
        let mut cur = self.cur;
        // QMF band 0: Type A, eight sub-filters, merged to six channels.
        let mut q = [((0.0f32, 0.0f32), (0.0f32, 0.0f32)); 8];
        for (qi, slot) in q.iter_mut().enumerate() {
            let (mut l, mut r) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
            for (m, &g) in G0.iter().enumerate() {
                // exp(jπ(2q+1)(m−6)/8); (m−6) may be negative.
                let k = ((2 * qi as i32 + 1) * (m as i32 - 6)).rem_euclid(16) as usize;
                let (c, s) = self.tw[k];
                let (xl, xr) = self.tap(m, 0);
                l.0 += g * (xl.0 * c - xl.1 * s);
                l.1 += g * (xl.0 * s + xl.1 * c);
                r.0 += g * (xr.0 * c - xr.1 * s);
                r.1 += g * (xr.0 * s + xr.1 * c);
            }
            *slot = (l, r);
        }
        let sum = |a: ((f32, f32), (f32, f32)), b: ((f32, f32), (f32, f32))| {
            (
                ((a.0).0 + (b.0).0, (a.0).1 + (b.0).1),
                ((a.1).0 + (b.1).0, (a.1).1 + (b.1).1),
            )
        };
        let merged = [q[6], q[7], q[0], q[1], sum(q[2], q[5]), sum(q[3], q[4])];
        for (k, &(l, r)) in merged.iter().enumerate() {
            add(&mut cur, usize::from(map[k]), l, r);
        }
        // QMF bands 1, 2: Type B (real), q0 = Σ g·x, q1 = Σ g·(−1)^(m−6)·x.
        for (p, band) in [1usize, 2].into_iter().enumerate() {
            let (mut a, mut b) = (
                ((0.0f32, 0.0f32), (0.0f32, 0.0f32)),
                ((0.0f32, 0.0f32), (0.0f32, 0.0f32)),
            );
            for (m, &g) in G12.iter().enumerate() {
                let (xl, xr) = self.tap(m, band);
                let sgn = if m % 2 == 0 { g } else { -g };
                (a.0).0 += g * xl.0;
                (a.0).1 += g * xl.1;
                (a.1).0 += g * xr.0;
                (a.1).1 += g * xr.1;
                (b.0).0 += sgn * xl.0;
                (b.0).1 += sgn * xl.1;
                (b.1).0 += sgn * xr.0;
                (b.1).1 += sgn * xr.1;
            }
            // Band 1 lands swapped (spectrally inverted), band 2 in order.
            let (first, second) = if p == 0 { (b, a) } else { (a, b) };
            add(&mut cur, usize::from(map[6 + 2 * p]), first.0, first.1);
            add(&mut cur, usize::from(map[7 + 2 * p]), second.0, second.1);
        }
        // QMF bands 3..63 pass through, delayed like the split bands.
        for band in 3..BANDS {
            let (l, r) = self.tap(PS_DELAY_SLOTS, band);
            add(&mut cur, usize::from(map[10 + band - 3]), l, r);
        }
        self.cur = cur;
    }

    fn quantize(&self) -> PsFrameParams {
        let mut out = PsFrameParams::default();
        for b in 0..PS_BANDS {
            let el = self.cur.l[b] + CARRY * self.prev.l[b];
            let er = self.cur.r[b] + CARRY * self.prev.r[b];
            let re = self.cur.re[b] + CARRY * self.prev.re[b];
            if el + er < SILENCE {
                continue; // IID 0 dB, ICC 1
            }
            let ratio = ((el + SILENCE * 1e-3) / (er + SILENCE * 1e-3)) as f32;
            let db = 3.010_3 * det_math::log2(ratio.clamp(1e-6, 1e6));
            let (grid, zero) = if self.iid_fine {
                (&IID_DB_FINE[..], 15i8)
            } else {
                (&IID_DB[..], 7i8)
            };
            out.iid[b] = nearest(grid, db) as i8 - zero;
            // One side silent: coherence is undefined, keep ICC = 1.
            if el.min(er) > 1e-5 * el.max(er) {
                let rho = (re / (el * er).sqrt()) as f32;
                out.icc[b] = nearest(&ICC_RHO, rho.clamp(-1.0, 1.0)) as u8;
            }
        }
        out
    }
}

/// Index of the grid value closest to `v` (first wins ties).
fn nearest(grid: &[f32], v: f32) -> usize {
    let mut best = 0usize;
    for (i, &g) in grid.iter().enumerate() {
        if (g - v).abs() < (grid[best] - v).abs() {
            best = i;
        }
    }
    best
}

/// Energy-compensated mono downmix in 2048-sample blocks.
pub(crate) struct PsDownmix {
    gain: f32,
}

impl PsDownmix {
    pub(crate) fn new() -> Self {
        Self { gain: 1.0 }
    }

    pub(crate) fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// Downmix one block (`l`, `r`, `out` of equal length ≤ [`PS_BLOCK`]).
    pub(crate) fn block(&mut self, l: &[f32], r: &[f32], out: &mut [f32]) {
        let (mut e_lr, mut e_m, mut peak) = (0.0f64, 0.0f64, 0.0f32);
        for ((&a, &b), o) in l.iter().zip(r.iter()).zip(out.iter_mut()) {
            let m = 0.5 * (a + b);
            *o = m;
            e_lr += f64::from(a) * f64::from(a) + f64::from(b) * f64::from(b);
            e_m += f64::from(m) * f64::from(m);
            peak = peak.max(m.abs());
        }
        let want = if e_m > 1e-12 {
            ((e_lr / (2.0 * e_m)).sqrt() as f32).clamp(1.0, 2.0)
        } else {
            1.0
        };
        let headroom = if peak > 0.0 { 0.999 / peak } else { 2.0 };
        let target = want.min(headroom).max(1.0f32.min(headroom));
        let from = self.gain.min(headroom);
        let n = out.len().max(1) as f32;
        for (i, o) in out.iter_mut().enumerate() {
            *o *= from + (target - from) * (i as f32 + 1.0) / n;
        }
        self.gain = target;
    }
}

/// Stereo front end: QMF analysis of both planes, parameters per frame,
/// downmix per block. Chunking is the caller's (one block per call).
pub(crate) struct PsAnalysis {
    qmf: [EncAnalysisQmf; 2],
    est: PsEstimator,
    mix: PsDownmix,
}

impl PsAnalysis {
    pub(crate) fn new() -> Self {
        Self {
            qmf: [EncAnalysisQmf::new(), EncAnalysisQmf::new()],
            est: PsEstimator::new(),
            mix: PsDownmix::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.qmf.iter_mut().for_each(EncAnalysisQmf::reset);
        self.est.reset();
        self.mix.reset();
    }

    /// Quantize IID on Table 8.26 (`iid_mode` 4). Default is coarse.
    pub(crate) fn set_iid_fine(&mut self, on: bool) {
        self.est.iid_fine = on;
    }

    /// One 2048-sample block: the mono downmix into `mono`, and the
    /// parameters of the frame that completed inside it (one per block
    /// after the first, which is [`PS_DELAY_SLOTS`] slots short).
    pub(crate) fn block(
        &mut self,
        l: &[f32; PS_BLOCK],
        r: &[f32; PS_BLOCK],
        mono: &mut [f32; PS_BLOCK],
    ) -> Result<Option<PsFrameParams>> {
        self.mix.block(l, r, mono);
        let mut done = None;
        for s in 0..PS_FRAME_SLOTS {
            let ls = self.qmf[0].push_slot(&l[s * BANDS..(s + 1) * BANDS])?;
            let rs = self.qmf[1].push_slot(&r[s * BANDS..(s + 1) * BANDS])?;
            if let Some(p) = self.est.push(ls, rs) {
                done = Some(p);
            }
        }
        Ok(done)
    }
}

#[cfg(test)]
#[path = "enc_ps_est_tests.rs"]
mod enc_ps_est_tests;
