//! Encoder 64-band analysis QMF on full-rate PCM — the decoder's `X`
//! grid (64 subbands × 32 slots per 2048-sample frame; `fTableHigh`
//! borders index this grid). ISO Figure 4.42 windowing generalised to
//! `M = 64` (all 640 prototype taps, `u[n] = Σ_j x[n+128j]·c[n+128j]`),
//! modulation `W[k] = Σ_n u[n]·exp(iπ/128·(k+½)(2n−½))` folded into one
//! 128-point complex FFT with [`super::det_math`] twiddles (no libm,
//! FMA-free butterflies). Kernel gain 1 (the decoder's 32-band bank on
//! the core uses 2× over half the taps), so `|W|²` matches the
//! decoder's `XLow` energy scale — asserted in the tests.

use super::det_math;
use super::error::{Error, Result};
use super::sbr_qmf::QMF_WINDOW;
use std::sync::OnceLock;

/// Subbands per slot.
pub(crate) const BANDS: usize = 64;
/// FFT length (`2·BANDS`).
const N2: usize = 128;
/// Prototype history in samples.
const HIST: usize = 640;

/// One analysis slot: 64 complex subband samples `W[k]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EncSlot {
    pub re: [f32; BANDS],
    pub im: [f32; BANDS],
}

impl EncSlot {
    pub(crate) fn energy(&self, k: usize) -> f32 {
        self.re[k] * self.re[k] + self.im[k] * self.im[k]
    }

    pub(crate) fn band_energy(&self, lo: usize, hi: usize) -> f32 {
        (lo..hi.min(BANDS)).map(|k| self.energy(k)).sum()
    }
}

/// 10^(−20/10). The next QMF band above a speech formant is about −29 dB
/// on the lecture onset; keeping it still opened a scale-factor section.
const FORMANT_RATIO: f32 = 1.0e-2;

/// Core cutoff in Hz for the slots of one core frame: top of the highest
/// analysis band below `k_x` within 20 dB of the peak. A flat spectrum
/// and silence both return the crossover (`k_x · fs / 128`), so noise
/// and harmonic patch sources stay put. The caller passes that frame's
/// slots only — a later frame in the same queue must not set this one.
pub(crate) fn core_cutoff_hz(channels: &[&[EncSlot]], k_x: usize, out_rate: u32) -> u32 {
    let k_x = k_x.min(BANDS);
    let k0_hz = ((u64::from(out_rate) * k_x as u64) / (BANDS as u64 * 2)) as u32;
    if k_x == 0 {
        return k0_hz;
    }
    let mut e = [0.0f32; BANDS];
    for slots in channels {
        for s in *slots {
            for (k, acc) in e.iter_mut().enumerate().take(k_x) {
                *acc += s.energy(k);
            }
        }
    }
    let mut peak = 0.0f32;
    for v in e.iter().take(k_x) {
        if *v > peak {
            peak = *v;
        }
    }
    if peak < 1.0e-18 {
        return k0_hz;
    }
    let thresh = peak * FORMANT_RATIO;
    let mut top = 0usize;
    for (k, v) in e.iter().enumerate().take(k_x) {
        if *v >= thresh {
            top = k;
        }
    }
    let hz = ((u64::from(out_rate) * (top as u64 + 1)) / (BANDS as u64 * 2)) as u32;
    if hz == 0 { k0_hz } else { hz.min(k0_hz) }
}

struct Plan {
    bitrev: [u16; N2],
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
    /// `exp(iπ n / 128)`.
    pre_c: [f32; N2],
    pre_s: [f32; N2],
    /// `exp(−iπ (2k+1) / 512)`.
    post_c: [f32; BANDS],
    post_s: [f32; BANDS],
}

fn plan() -> &'static Plan {
    static P: OnceLock<Plan> = OnceLock::new();
    P.get_or_init(|| {
        let mut bitrev = [0u16; N2];
        let mut j = 0usize;
        for slot in &mut bitrev {
            *slot = j as u16;
            let mut bit = N2 / 2;
            while bit != 0 && j >= bit {
                j -= bit;
                bit >>= 1;
            }
            j += bit;
        }
        let (tw_re, tw_im) = det_math::twiddle_table(N2);
        let mut pre_c = [0.0f32; N2];
        let mut pre_s = [0.0f32; N2];
        for n in 0..N2 {
            let (s, c) = det_math::sincos(core::f32::consts::PI * n as f32 / N2 as f32);
            pre_c[n] = c;
            pre_s[n] = s;
        }
        let mut post_c = [0.0f32; BANDS];
        let mut post_s = [0.0f32; BANDS];
        for k in 0..BANDS {
            let (s, c) =
                det_math::sincos(core::f32::consts::PI * (2 * k + 1) as f32 / (8 * BANDS) as f32);
            post_c[k] = c;
            post_s[k] = -s;
        }
        Plan {
            bitrev,
            tw_re,
            tw_im,
            pre_c,
            pre_s,
            post_c,
            post_s,
        }
    })
}

/// In-place radix-2 FFT with `e^{+i2πkn/N}` twiddles (product written
/// as separate mul/add so no FMA contraction can change rounding).
fn fft128(re: &mut [f32; N2], im: &mut [f32; N2], p: &Plan) {
    for (i, &rev) in p.bitrev.iter().enumerate() {
        let j = rev as usize;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= N2 {
        let half = len / 2;
        for i in (0..N2).step_by(len) {
            for k in 0..half {
                let wr = p.tw_re[off + k];
                let wi = p.tw_im[off + k];
                let j = i + k + half;
                let wrj = wr * re[j];
                let wij = wi * im[j];
                let tr = wrj - wij;
                let wir = wi * re[j];
                let wjr = wr * im[j];
                let ti = wir + wjr;
                let ur = re[i + k];
                let ui = im[i + k];
                re[j] = ur - tr;
                im[j] = ui - ti;
                re[i + k] = ur + tr;
                im[i + k] = ui + ti;
            }
        }
        off += half;
        len *= 2;
    }
}

fn modulate(u: &[f32; N2]) -> EncSlot {
    let p = plan();
    let mut re = [0.0f32; N2];
    let mut im = [0.0f32; N2];
    for n in 0..N2 {
        re[n] = u[n] * p.pre_c[n];
        im[n] = u[n] * p.pre_s[n];
    }
    fft128(&mut re, &mut im, p);
    let mut slot = EncSlot {
        re: [0.0; BANDS],
        im: [0.0; BANDS],
    };
    for k in 0..BANDS {
        let a = re[k] * p.post_c[k];
        let b = im[k] * p.post_s[k];
        slot.re[k] = a - b;
        let c = re[k] * p.post_s[k];
        let d = im[k] * p.post_c[k];
        slot.im[k] = c + d;
    }
    slot
}

/// 64-band analysis bank (encoder path, f32 + det_math).
pub(crate) struct EncAnalysisQmf {
    x: [f32; HIST],
}

impl EncAnalysisQmf {
    pub(crate) fn new() -> Self {
        Self { x: [0.0; HIST] }
    }

    pub(crate) fn reset(&mut self) {
        self.x = [0.0; HIST];
    }

    /// Push 64 output-rate samples; the newest lands at `x[0]`.
    pub(crate) fn push_slot(&mut self, samples: &[f32]) -> Result<EncSlot> {
        if samples.len() != BANDS {
            return Err(Error::SbrQmfInvalid);
        }
        self.x.copy_within(0..HIST - BANDS, BANDS);
        for (n, s) in samples.iter().enumerate() {
            self.x[BANDS - 1 - n] = *s;
        }
        let mut u = [0.0f32; N2];
        for (n, un) in u.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for j in 0..5 {
                let idx = n + j * N2;
                acc += self.x[idx] * (QMF_WINDOW[idx] as f32);
            }
            *un = acc;
        }
        Ok(modulate(&u))
    }
}

impl Default for EncAnalysisQmf {
    fn default() -> Self {
        Self::new()
    }
}
