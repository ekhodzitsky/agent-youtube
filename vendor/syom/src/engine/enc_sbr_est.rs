//! HE v1 SBR parameter estimation (TASK-87): header per rate and
//! bitrate, FIXFIX grid from an HF energy surge, envelope / noise-floor
//! scalefactors on the decoder's dequantisation scale, inverse-filter
//! modes, delta direction with a Huffman bit estimate. In-tree ISO
//! tables only; no peer encoder source.

use super::det_math;
pub(crate) use super::enc_sbr_header::he_header;
use super::enc_sbr_qmf::EncSlot;
use super::error::{Error, Result};
use super::sbr_envelope::{SbrEnvelopeData, SbrNoiseData};
use super::sbr_freq_bands::{HiLoTables, k0, k2, master_table};
use super::sbr_grid::{FrameClass, SbrDtdf, SbrGrid, SbrInvf};
use super::sbr_header::SbrHeader;
use super::sbr_hf_gen::{Patches, build_patches};
use super::sbr_huffman::{SbrHuffCodebook, SbrHuffContext, env_tables, noise_tables};
use super::sbr_reconstruct::ref_band;
use super::sbr_time_grid::TimeGrid;

/// Analysis slots per frame (`numTimeSlots · RATE`).
pub(crate) const SLOTS: usize = 32;
/// Core time slots per frame.
const TIME_SLOTS: i32 = 16;
/// The decoder analyses the LC core at 16-bit scale (`INV_S16` runs
/// after SBR) while the encoder bank runs on [-1, 1]: +30 in `log2`
/// energy, minus the `EOrig = 64·2^(E/a)` offset of 6.
const SCALE_LOG2: f32 = 24.0;
/// Peak-over-running-mean HF slot energy that selects 2 / 4 envelopes.
const SURGE: [f32; 2] = [3.0, 8.0];
/// Per-slot HF energy floor for the surge rule (≈ −90 dBFS): silence
/// never counts as a transient.
const SURGE_FLOOR: f32 = 1e-9;
/// Slots of HF history behind the surge rule.
const SURGE_HIST: usize = 8;
/// Largest noise-floor index (`Q`); `2^(6−Q)` is the noise/patch ratio.
const Q_MAX: i32 = 30;
/// Tonality-gap thresholds for `bs_invf_mode` 1 / 2 / 3.
const INVF_GAP: [f32; 3] = [0.15, 0.35, 0.6];

/// One channel-frame of SBR parameters: absolute scalefactors plus the
/// raw wire deltas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SbrFrameParams {
    pub grid: SbrGrid,
    pub dtdf: SbrDtdf,
    pub invf: SbrInvf,
    /// `E_Q[l][k]`.
    pub env_q: Vec<Vec<i32>>,
    /// `Q[l][k]`.
    pub noise_q: Vec<Vec<i32>>,
    pub envelope: SbrEnvelopeData,
    pub noise: SbrNoiseData,
    /// Empty: `bs_add_harmonic_flag = 0` in v1.
    pub add_harmonic: Vec<bool>,
    /// `sbr_single_channel_element()` payload bits after the header.
    pub est_bits: u32,
}

/// Per-channel estimator (state: HF history, previous scalefactors).
pub(crate) struct SbrEstimator {
    header: SbrHeader,
    bands: HiLoTables,
    patches: Patches,
    time_grid: TimeGrid,
    hf_hist: [f32; SURGE_HIST],
    prev_env: Vec<i32>,
    prev_env_high: bool,
    prev_noise: Vec<i32>,
}

/// `(prediction error, total)` energy of subband `k` over slots `[s0, s1)`:
/// a one-tap complex predictor fits a stationary tone exactly, noise not.
fn tonality(slots: &[EncSlot], k: usize, s0: usize, s1: usize) -> (f32, f32) {
    let mut total = 0.0f32;
    let mut den = 0.0f32;
    let mut cr = 0.0f32;
    let mut ci = 0.0f32;
    for s in (s0 + 1)..s1 {
        let (ar, ai) = (slots[s].re[k], slots[s].im[k]);
        let (br, bi) = (slots[s - 1].re[k], slots[s - 1].im[k]);
        total += ar * ar + ai * ai;
        den += br * br + bi * bi;
        cr += ar * br + ai * bi;
        ci += ai * br - ar * bi;
    }
    if den <= 0.0 {
        return (total, total);
    }
    let fit = (cr * cr + ci * ci) / den;
    ((total - fit).max(0.0), total)
}

fn code_len(book: SbrHuffCodebook, delta: i32) -> Option<u32> {
    let idx = usize::try_from(delta + book.1).ok()?;
    book.0.get(idx).map(|&(len, _)| u32::from(len))
}

fn round_clamp(v: f32, max: i32) -> i32 {
    ((v + 0.5).floor() as i32).clamp(0, max)
}

/// Cost of `cur` coded against `refv` in one direction, or `None` if a
/// delta falls outside the codebook.
fn direction_cost(cur: &[i32], refv: impl Fn(usize) -> i32, book: SbrHuffCodebook) -> Option<u32> {
    cur.iter().enumerate().try_fold(0u32, |acc, (b, &v)| {
        Some(acc + code_len(book, v - refv(b))?)
    })
}

/// Cheaper valid delta direction for one envelope / floor: `(time,
/// raw deltas, bits)`; frequency is always valid (bands were clamped).
fn choose_direction(
    cur: &[i32],
    refv: Option<&dyn Fn(usize) -> i32>,
    start_bits: u32,
    books: (SbrHuffCodebook, SbrHuffCodebook),
) -> Result<(bool, Vec<i32>, u32)> {
    let f_cost =
        start_bits + direction_cost(&cur[1..], |b| cur[b], books.1).ok_or(Error::SbrGridInvalid)?;
    let t_cost = refv.and_then(|r| direction_cost(cur, r, books.0));
    match (refv, t_cost) {
        (Some(r), Some(t)) if t < f_cost => {
            Ok((true, (0..cur.len()).map(|b| cur[b] - r(b)).collect(), t))
        }
        _ => {
            let row = (0..cur.len())
                .map(|b| if b == 0 { cur[0] } else { cur[b] - cur[b - 1] })
                .collect();
            Ok((false, row, f_cost))
        }
    }
}

impl SbrEstimator {
    pub(crate) fn new(fs_sbr: u32, kbps_per_channel: u32) -> Result<Self> {
        let header = he_header(fs_sbr, kbps_per_channel)?;
        let k0v = k0(fs_sbr, header.start_freq)?;
        let k2v = k2(fs_sbr, header.stop_freq, k0v)?;
        let f_master = master_table(k0v, k2v, header.freq_scale, header.alter_scale)?;
        let bands = HiLoTables::derive(&f_master, header.xover_band, header.noise_bands)?;
        let patches = build_patches(&f_master, k0v, bands.k_x, bands.m, fs_sbr)?;
        Ok(Self {
            header,
            bands,
            patches,
            time_grid: TimeGrid {
                t_e: Vec::new(),
                t_q: Vec::new(),
                l_a: -1,
            },
            hf_hist: [0.0; SURGE_HIST],
            prev_env: Vec::new(),
            prev_env_high: false,
            prev_noise: Vec::new(),
        })
    }

    pub(crate) fn header(&self) -> &SbrHeader {
        &self.header
    }

    pub(crate) fn bands(&self) -> &HiLoTables {
        &self.bands
    }

    /// Forget signal history (tables stay).
    pub(crate) fn reset(&mut self) {
        self.hf_hist = [0.0; SURGE_HIST];
        self.prev_env.clear();
        self.prev_noise.clear();
    }

    /// Source subband the decoder patches into HF subband `k`.
    fn patch_source(&self, k: usize) -> usize {
        let mut lo = self.bands.k_x as usize;
        for (p, &n) in self.patches.num.iter().enumerate() {
            if k < lo + n {
                return self.patches.start[p] + (k - lo);
            }
            lo += n;
        }
        k
    }

    /// FIXFIX envelope count from the HF energy surge over the running
    /// mean (the LC attack detector cannot see above the core rate).
    fn choose_num_env(&mut self, slots: &[EncSlot]) -> usize {
        let (kx, m) = (self.bands.k_x as usize, self.bands.m as usize);
        let mut hist = self.hf_hist;
        let mut pos = 0usize;
        let mut surge = 0.0f32;
        for s in slots {
            let e = s.band_energy(kx, kx + m);
            let mean = hist.iter().sum::<f32>() / SURGE_HIST as f32 + SURGE_FLOOR;
            surge = surge.max(e / mean);
            hist[pos] = e;
            pos = (pos + 1) % SURGE_HIST;
        }
        self.hf_hist = hist;
        1 << SURGE.iter().filter(|&&t| surge >= t).count()
    }

    /// Estimate one frame from its 32 analysis slots (the caller aligns
    /// them to the decoder's envelope grid).
    pub(crate) fn estimate(&mut self, slots: &[EncSlot]) -> Result<SbrFrameParams> {
        if slots.len() != SLOTS {
            return Err(Error::SbrGridInvalid);
        }
        let num_env = self.choose_num_env(slots);
        let high_res = num_env == 1;
        let grid = SbrGrid {
            frame_class: FrameClass::FixFix,
            num_env,
            num_noise: if num_env > 1 { 2 } else { 1 },
            freq_res: vec![high_res; num_env],
            var_bord_0: 0,
            var_bord_1: 0,
            rel_bord_0: Vec::new(),
            rel_bord_1: Vec::new(),
            pointer: 0,
            amp_res_override: high_res,
        };
        self.time_grid.derive_into(&grid, TIME_SLOTS)?;
        let amp_res = self.header.amp_res && !grid.amp_res_override;
        let (a, e_max, lav_f) = if amp_res {
            (1.0, 63, 31)
        } else {
            (2.0, 127, 60)
        };
        let f = if high_res {
            &self.bands.f_table_high
        } else {
            &self.bands.f_table_low
        };

        // Envelopes: mean |X|² per band and envelope, quantised, then
        // held within the frequency codebook's reach of the band below.
        let mut env_q = Vec::with_capacity(num_env);
        for l in 0..num_env {
            let (s0, s1) = (
                2 * self.time_grid.t_e[l] as usize,
                2 * self.time_grid.t_e[l + 1] as usize,
            );
            let mut row = Vec::with_capacity(f.len() - 1);
            for w in f.windows(2) {
                let (lo, hi) = (w[0] as usize, w[1] as usize);
                let sum: f32 = slots[s0..s1].iter().map(|s| s.band_energy(lo, hi)).sum();
                let mean = sum / ((s1 - s0) * (hi - lo)) as f32;
                let mut q = round_clamp(a * (det_math::log2(mean.max(1e-30)) + SCALE_LOG2), e_max);
                if let Some(&prev) = row.last() {
                    q = q.clamp(prev - lav_f, prev + lav_f);
                }
                row.push(q);
            }
            env_q.push(row);
        }

        // Noise floors and inverse filtering from the tonality gap
        // between the original HF and the LF the decoder will patch in.
        let nq = self.bands.n_q();
        let mut noise_q = Vec::with_capacity(grid.num_noise);
        let mut invf = vec![0u8; nq];
        for l in 0..grid.num_noise {
            let (s0, s1) = (
                2 * self.time_grid.t_q[l] as usize,
                2 * self.time_grid.t_q[l + 1] as usize,
            );
            let mut row = Vec::with_capacity(nq);
            for (nb, w) in self.bands.f_table_noise.windows(2).enumerate() {
                let (mut err_hf, mut tot_hf, mut err_lf, mut tot_lf) =
                    (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for k in w[0] as usize..w[1] as usize {
                    let (e, t) = tonality(slots, k, s0, s1);
                    err_hf += e;
                    tot_hf += t;
                    let (e, t) = tonality(slots, self.patch_source(k), s0, s1);
                    err_lf += e;
                    tot_lf += t;
                }
                let nf_hf = if tot_hf > 0.0 { err_hf / tot_hf } else { 0.0 };
                let nf_lf = if tot_lf > 0.0 { err_lf / tot_lf } else { 0.0 };
                let gap = (nf_hf - nf_lf).max(0.0);
                let ratio = gap / (1.0 - nf_hf).max(1e-3);
                let q = if ratio <= 0.0 {
                    Q_MAX
                } else {
                    round_clamp(6.0 - det_math::log2(ratio), Q_MAX)
                };
                row.push(q);
                if l == 0 {
                    invf[nb] = INVF_GAP.iter().filter(|&&t| gap > t).count() as u8;
                }
            }
            noise_q.push(row);
        }

        // Delta direction per envelope / floor: cheapest valid codebook.
        let ctx = SbrHuffContext {
            coupling: false,
            ch: false,
            amp_res,
        };
        let mut bits = 1 + 5 + (num_env + grid.num_noise) as u32 + 2 * nq as u32 + 1 + 1;
        let mut df_env = Vec::with_capacity(num_env);
        let mut env_rows = Vec::with_capacity(num_env);
        for l in 0..num_env {
            let (refv, ref_high): (&[i32], bool) = if l > 0 {
                (&env_q[l - 1], high_res)
            } else {
                (&self.prev_env, self.prev_env_high)
            };
            let map = |b: usize| ref_band(&self.bands, refv, high_res, ref_high, b);
            let r: Option<&dyn Fn(usize) -> i32> = if refv.is_empty() { None } else { Some(&map) };
            let start_bits = if amp_res { 6 } else { 7 };
            let (t, row, c) = choose_direction(&env_q[l], r, start_bits, env_tables(ctx))?;
            bits += c;
            df_env.push(t);
            env_rows.push(row);
        }
        let mut df_noise = Vec::with_capacity(grid.num_noise);
        let mut noise_rows = Vec::with_capacity(grid.num_noise);
        for l in 0..grid.num_noise {
            let refv: &[i32] = if l > 0 {
                &noise_q[l - 1]
            } else {
                &self.prev_noise
            };
            let map = |b: usize| refv[b];
            let r: Option<&dyn Fn(usize) -> i32> = if refv.len() == nq { Some(&map) } else { None };
            let (t, row, c) = choose_direction(&noise_q[l], r, 5, noise_tables(ctx))?;
            bits += c;
            df_noise.push(t);
            noise_rows.push(row);
        }

        self.prev_env.clear();
        self.prev_env.extend_from_slice(&env_q[num_env - 1]);
        self.prev_env_high = high_res;
        self.prev_noise.clear();
        self.prev_noise
            .extend_from_slice(&noise_q[grid.num_noise - 1]);

        Ok(SbrFrameParams {
            grid,
            dtdf: SbrDtdf { df_env, df_noise },
            invf: SbrInvf { invf_mode: invf },
            env_q,
            noise_q,
            envelope: SbrEnvelopeData { data: env_rows },
            noise: SbrNoiseData { data: noise_rows },
            add_harmonic: Vec::new(),
            est_bits: bits,
        })
    }
}

#[cfg(test)]
#[path = "enc_sbr_est_recon_tests.rs"]
mod enc_sbr_est_recon_tests;
#[cfg(test)]
#[path = "enc_sbr_est_tests.rs"]
mod enc_sbr_est_tests;
