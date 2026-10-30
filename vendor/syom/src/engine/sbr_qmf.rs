//! SBR QMF filterbanks — ISO/IEC 14496-3 §4.6.18.4.
//!
//! The complex-exponential-modulated filterbank pair of the SBR tool:
//!
//! * [`AnalysisQmf`] — §4.6.18.4.1 / Figure 4.42: splits the core
//!   decoder's time-domain output into 32 complex-valued subband
//!   signals (oversampled by two relative to a real QMF bank), one
//!   32-sample slot at a time.
//! * [`SynthesisQmf`] — §4.6.18.4.2 / Figure 4.43: recombines 64
//!   complex subbands into 64 real time-domain samples per slot (the
//!   dual-rate output of the SBR tool).
//! * [`DownsampledSynthesisQmf`] — §4.6.18.4.3 / Figure 4.44: the
//!   32-channel variant that keeps the output at the core rate.
//!
//! The 640-tap prototype window `c[i]` is Table 4.A.89, transcribed
//! from the staged ISO/IEC 14496-3:2009 spec PDF (`docs/audio/aac/`).
//! The table prints `c[639]` with nine decimals (`-0.000552528`); every
//! other entry carries ten. The transcription preserves the printed
//! digits verbatim, including the mirror structure
//! `|c[i]| == |c[640 - i]|` that the tests pin.
//!
//! ## Provenance
//!
//! Every constant and loop bound below comes from the §4.6.18.4 text
//! and the Figure 4.42 / 4.43 / 4.44 flowcharts of the staged spec.
//! No part of this implementation is derived from any external decoder.
//! The analysis/synthesis modulation matrices are evaluated via a
//! DCT-IV factorization (`sbr_qmf_dct.rs`); the dense GEMV is kept as
//! a test reference. Encoder reuse of this path must use `det_math`
//! twiddles and FMA-free butterflies.

use crate::engine::{Error, Result};
#[cfg(test)]
use std::sync::OnceLock;

#[path = "sbr_qmf_dct.rs"]
mod qmf_dct;

#[cfg(test)]
fn analysis_m() -> &'static [Complex] {
    static M: OnceLock<Vec<Complex>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = Vec::with_capacity(32 * 64);
        for k in 0..32 {
            for n in 0..64 {
                let arg = core::f64::consts::PI / 64.0 * (k as f64 + 0.5) * (2.0 * n as f64 - 0.5);
                m.push(Complex::new(2.0 * arg.cos(), 2.0 * arg.sin()));
            }
        }
        m
    })
    .as_slice()
}

#[cfg(test)]
fn synthesis_n_mat() -> &'static [Complex] {
    static N: OnceLock<Vec<Complex>> = OnceLock::new();
    N.get_or_init(|| {
        let mut n_mat = Vec::with_capacity(128 * 64);
        for n in 0..128 {
            for k in 0..64 {
                let arg =
                    core::f64::consts::PI / 128.0 * (k as f64 + 0.5) * (2.0 * n as f64 - 255.0);
                n_mat.push(Complex::new(arg.cos() / 64.0, arg.sin() / 64.0));
            }
        }
        n_mat
    })
    .as_slice()
}

/// A complex number, as used by the SBR subband domain (§4.6.18.2.2:
/// the subband samples are complex-valued).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// `re + i·im`.
    #[inline]
    #[must_use]
    pub fn new(re: f64, im: f64) -> Self {
        Complex { re, im }
    }

    /// The complex conjugate.
    #[inline]
    #[must_use]
    pub fn conj(self) -> Self {
        Complex {
            re: self.re,
            im: -self.im,
        }
    }

    /// Squared magnitude `re² + im²`.
    #[inline]
    #[must_use]
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

impl core::ops::Add for Complex {
    type Output = Complex;
    #[inline]
    fn add(self, rhs: Complex) -> Complex {
        Complex::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl core::ops::Sub for Complex {
    type Output = Complex;
    #[inline]
    fn sub(self, rhs: Complex) -> Complex {
        Complex::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl core::ops::Mul for Complex {
    type Output = Complex;
    #[inline]
    fn mul(self, rhs: Complex) -> Complex {
        Complex::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

impl core::ops::Mul<f64> for Complex {
    type Output = Complex;
    #[inline]
    fn mul(self, rhs: f64) -> Complex {
        Complex::new(self.re * rhs, self.im * rhs)
    }
}

impl core::ops::AddAssign for Complex {
    #[inline]
    fn add_assign(&mut self, rhs: Complex) {
        self.re += rhs.re;
        self.im += rhs.im;
    }
}

/// Table 4.A.89 — the 640 coefficients `c[i]` of the QMF bank window,
/// shared by the analysis and both synthesis filterbanks.
#[rustfmt::skip]
pub const QMF_WINDOW: [f64; 640] = [
    0.0000000000, -0.0005525286, -0.0005617692, -0.0004947518,
    -0.0004875227, -0.0004893791, -0.0005040714, -0.0005226564,
    -0.0005466565, -0.0005677802, -0.0005870930, -0.0006132747,
    -0.0006312493, -0.0006540333, -0.0006777690, -0.0006941614,
    -0.0007157736, -0.0007255043, -0.0007440941, -0.0007490598,
    -0.0007681371, -0.0007724848, -0.0007834332, -0.0007779869,
    -0.0007803664, -0.0007801449, -0.0007757977, -0.0007630793,
    -0.0007530001, -0.0007319357, -0.0007215391, -0.0006917937,
    -0.0006650415, -0.0006341594, -0.0005946118, -0.0005564576,
    -0.0005145572, -0.0004606325, -0.0004095121, -0.0003501175,
    -0.0002896981, -0.0002098337, -0.0001446380, -0.0000617334,
    0.0000134949, 0.0001094383, 0.0002043017, 0.0002949531,
    0.0004026540, 0.0005107388, 0.0006239376, 0.0007458025,
    0.0008608443, 0.0009885988, 0.0011250155, 0.0012577884,
    0.0013902494, 0.0015443219, 0.0016868083, 0.0018348265,
    0.0019841140, 0.0021461583, 0.0023017254, 0.0024625616,
    0.0026201758, 0.0027870464, 0.0029469447, 0.0031125420,
    0.0032739613, 0.0034418874, 0.0036008268, 0.0037603922,
    0.0039207432, 0.0040819753, 0.0042264269, 0.0043730719,
    0.0045209852, 0.0046606460, 0.0047932560, 0.0049137603,
    0.0050393022, 0.0051407353, 0.0052461166, 0.0053471681,
    0.0054196775, 0.0054876040, 0.0055475714, 0.0055938023,
    0.0056220643, 0.0056455196, 0.0056389199, 0.0056266114,
    0.0055917128, 0.0055404363, 0.0054753783, 0.0053838975,
    0.0052715758, 0.0051382275, 0.0049839687, 0.0048109469,
    0.0046039530, 0.0043801861, 0.0041251642, 0.0038456408,
    0.0035401246, 0.0032091885, 0.0028446757, 0.0024508540,
    0.0020274176, 0.0015784682, 0.0010902329, 0.0005832264,
    0.0000276045, -0.0005464280, -0.0011568135, -0.0018039472,
    -0.0024826723, -0.0031933778, -0.0039401124, -0.0047222596,
    -0.0055337211, -0.0063792293, -0.0072615816, -0.0081798233,
    -0.0091325329, -0.0101150215, -0.0111315548, -0.0121849995,
    0.0132718220, 0.0143904666, 0.0155405553, 0.0167324712,
    0.0179433381, 0.0191872431, 0.0204531793, 0.0217467550,
    0.0230680169, 0.0244160992, 0.0257875847, 0.0271859429,
    0.0286072173, 0.0300502657, 0.0315017608, 0.0329754081,
    0.0344620948, 0.0359697560, 0.0374812850, 0.0390053679,
    0.0405349170, 0.0420649094, 0.0436097542, 0.0451488405,
    0.0466843027, 0.0482165720, 0.0497385755, 0.0512556155,
    0.0527630746, 0.0542452768, 0.0557173648, 0.0571616450,
    0.0585915683, 0.0599837480, 0.0613455171, 0.0626857808,
    0.0639715898, 0.0652247106, 0.0664367512, 0.0676075985,
    0.0687043828, 0.0697630244, 0.0707628710, 0.0717002673,
    0.0725682583, 0.0733620255, 0.0741003642, 0.0747452558,
    0.0753137336, 0.0758008358, 0.0761992479, 0.0764992170,
    0.0767093490, 0.0768173975, 0.0768230011, 0.0767204924,
    0.0765050718, 0.0761748321, 0.0757305756, 0.0751576255,
    0.0744664394, 0.0736406005, 0.0726774642, 0.0715826364,
    0.0703533073, 0.0689664013, 0.0674525021, 0.0657690668,
    0.0639444805, 0.0619602779, 0.0598166570, 0.0575152691,
    0.0550460034, 0.0524093821, 0.0495978676, 0.0466303305,
    0.0434768782, 0.0401458278, 0.0366418116, 0.0329583930,
    0.0290824006, 0.0250307561, 0.0207997072, 0.0163701258,
    0.0117623832, 0.0069636862, 0.0019765601, -0.0032086896,
    -0.0085711749, -0.0141288827, -0.0198834129, -0.0258227288,
    -0.0319531274, -0.0382776572, -0.0447806821, -0.0514804176,
    -0.0583705326, -0.0654409853, -0.0726943300, -0.0801372934,
    -0.0877547536, -0.0955533352, -0.1035329531, -0.1116826931,
    -0.1200077984, -0.1285002850, -0.1371551761, -0.1459766491,
    -0.1549607071, -0.1640958855, -0.1733808172, -0.1828172548,
    -0.1923966745, -0.2021250176, -0.2119735853, -0.2219652696,
    -0.2320690870, -0.2423016884, -0.2526480309, -0.2631053299,
    -0.2736634040, -0.2843214189, -0.2950716717, -0.3059098575,
    -0.3168278913, -0.3278113727, -0.3388722693, -0.3499914122,
    0.3611589903, 0.3723795546, 0.3836350013, 0.3949211761,
    0.4062317676, 0.4175696896, 0.4289119920, 0.4402553754,
    0.4515996535, 0.4629308085, 0.4742453214, 0.4855253091,
    0.4967708254, 0.5079817500, 0.5191234970, 0.5302240895,
    0.5412553448, 0.5522051258, 0.5630789140, 0.5738524131,
    0.5845403235, 0.5951123086, 0.6055783538, 0.6159109932,
    0.6261242695, 0.6361980107, 0.6461269695, 0.6559016302,
    0.6655139880, 0.6749663190, 0.6842353293, 0.6933282376,
    0.7022388719, 0.7109410426, 0.7194462634, 0.7277448900,
    0.7358211758, 0.7436827863, 0.7513137456, 0.7587080760,
    0.7658674865, 0.7727780881, 0.7794287519, 0.7858353120,
    0.7919735841, 0.7978466413, 0.8034485751, 0.8087695004,
    0.8138191270, 0.8185776004, 0.8230419890, 0.8272275347,
    0.8311038457, 0.8346937361, 0.8379717337, 0.8409541392,
    0.8436238281, 0.8459818469, 0.8480315777, 0.8497805198,
    0.8511971524, 0.8523047035, 0.8531020949, 0.8535720573,
    0.8537385600, 0.8535720573, 0.8531020949, 0.8523047035,
    0.8511971524, 0.8497805198, 0.8480315777, 0.8459818469,
    0.8436238281, 0.8409541392, 0.8379717337, 0.8346937361,
    0.8311038457, 0.8272275347, 0.8230419890, 0.8185776004,
    0.8138191270, 0.8087695004, 0.8034485751, 0.7978466413,
    0.7919735841, 0.7858353120, 0.7794287519, 0.7727780881,
    0.7658674865, 0.7587080760, 0.7513137456, 0.7436827863,
    0.7358211758, 0.7277448900, 0.7194462634, 0.7109410426,
    0.7022388719, 0.6933282376, 0.6842353293, 0.6749663190,
    0.6655139880, 0.6559016302, 0.6461269695, 0.6361980107,
    0.6261242695, 0.6159109932, 0.6055783538, 0.5951123086,
    0.5845403235, 0.5738524131, 0.5630789140, 0.5522051258,
    0.5412553448, 0.5302240895, 0.5191234970, 0.5079817500,
    0.4967708254, 0.4855253091, 0.4742453214, 0.4629308085,
    0.4515996535, 0.4402553754, 0.4289119920, 0.4175696896,
    0.4062317676, 0.3949211761, 0.3836350013, 0.3723795546,
    -0.3611589903, -0.3499914122, -0.3388722693, -0.3278113727,
    -0.3168278913, -0.3059098575, -0.2950716717, -0.2843214189,
    -0.2736634040, -0.2631053299, -0.2526480309, -0.2423016884,
    -0.2320690870, -0.2219652696, -0.2119735853, -0.2021250176,
    -0.1923966745, -0.1828172548, -0.1733808172, -0.1640958855,
    -0.1549607071, -0.1459766491, -0.1371551761, -0.1285002850,
    -0.1200077984, -0.1116826931, -0.1035329531, -0.0955533352,
    -0.0877547536, -0.0801372934, -0.0726943300, -0.0654409853,
    -0.0583705326, -0.0514804176, -0.0447806821, -0.0382776572,
    -0.0319531274, -0.0258227288, -0.0198834129, -0.0141288827,
    -0.0085711749, -0.0032086896, 0.0019765601, 0.0069636862,
    0.0117623832, 0.0163701258, 0.0207997072, 0.0250307561,
    0.0290824006, 0.0329583930, 0.0366418116, 0.0401458278,
    0.0434768782, 0.0466303305, 0.0495978676, 0.0524093821,
    0.0550460034, 0.0575152691, 0.0598166570, 0.0619602779,
    0.0639444805, 0.0657690668, 0.0674525021, 0.0689664013,
    0.0703533073, 0.0715826364, 0.0726774642, 0.0736406005,
    0.0744664394, 0.0751576255, 0.0757305756, 0.0761748321,
    0.0765050718, 0.0767204924, 0.0768230011, 0.0768173975,
    0.0767093490, 0.0764992170, 0.0761992479, 0.0758008358,
    0.0753137336, 0.0747452558, 0.0741003642, 0.0733620255,
    0.0725682583, 0.0717002673, 0.0707628710, 0.0697630244,
    0.0687043828, 0.0676075985, 0.0664367512, 0.0652247106,
    0.0639715898, 0.0626857808, 0.0613455171, 0.0599837480,
    0.0585915683, 0.0571616450, 0.0557173648, 0.0542452768,
    0.0527630746, 0.0512556155, 0.0497385755, 0.0482165720,
    0.0466843027, 0.0451488405, 0.0436097542, 0.0420649094,
    0.0405349170, 0.0390053679, 0.0374812850, 0.0359697560,
    0.0344620948, 0.0329754081, 0.0315017608, 0.0300502657,
    0.0286072173, 0.0271859429, 0.0257875847, 0.0244160992,
    0.0230680169, 0.0217467550, 0.0204531793, 0.0191872431,
    0.0179433381, 0.0167324712, 0.0155405553, 0.0143904666,
    -0.0132718220, -0.0121849995, -0.0111315548, -0.0101150215,
    -0.0091325329, -0.0081798233, -0.0072615816, -0.0063792293,
    -0.0055337211, -0.0047222596, -0.0039401124, -0.0031933778,
    -0.0024826723, -0.0018039472, -0.0011568135, -0.0005464280,
    0.0000276045, 0.0005832264, 0.0010902329, 0.0015784682,
    0.0020274176, 0.0024508540, 0.0028446757, 0.0032091885,
    0.0035401246, 0.0038456408, 0.0041251642, 0.0043801861,
    0.0046039530, 0.0048109469, 0.0049839687, 0.0051382275,
    0.0052715758, 0.0053838975, 0.0054753783, 0.0055404363,
    0.0055917128, 0.0056266114, 0.0056389199, 0.0056455196,
    0.0056220643, 0.0055938023, 0.0055475714, 0.0054876040,
    0.0054196775, 0.0053471681, 0.0052461166, 0.0051407353,
    0.0050393022, 0.0049137603, 0.0047932560, 0.0046606460,
    0.0045209852, 0.0043730719, 0.0042264269, 0.0040819753,
    0.0039207432, 0.0037603922, 0.0036008268, 0.0034418874,
    0.0032739613, 0.0031125420, 0.0029469447, 0.0027870464,
    0.0026201758, 0.0024625616, 0.0023017254, 0.0021461583,
    0.0019841140, 0.0018348265, 0.0016868083, 0.0015443219,
    0.0013902494, 0.0012577884, 0.0011250155, 0.0009885988,
    0.0008608443, 0.0007458025, 0.0006239376, 0.0005107388,
    0.0004026540, 0.0002949531, 0.0002043017, 0.0001094383,
    0.0000134949, -0.0000617334, -0.0001446380, -0.0002098337,
    -0.0002896981, -0.0003501175, -0.0004095121, -0.0004606325,
    -0.0005145572, -0.0005564576, -0.0005946118, -0.0006341594,
    -0.0006650415, -0.0006917937, -0.0007215391, -0.0007319357,
    -0.0007530001, -0.0007630793, -0.0007757977, -0.0007801449,
    -0.0007803664, -0.0007779869, -0.0007834332, -0.0007724848,
    -0.0007681371, -0.0007490598, -0.0007440941, -0.0007255043,
    -0.0007157736, -0.0006941614, -0.0006777690, -0.0006540333,
    -0.0006312493, -0.0006132747, -0.0005870930, -0.0005677802,
    -0.0005466565, -0.0005226564, -0.0005040714, -0.0004893791,
    -0.0004875227, -0.0004947518, -0.0005617692, -0.000552528,
];

/// Even coefficients of [`QMF_WINDOW`], five taps of 64. `TAP[j][n]` is
/// `QMF_WINDOW[2 * (n + 64 * j)]`, which is `c[2n]` in the Figure 4.42 sum.
const ANALYSIS_TAP: [[f64; 64]; 5] = analysis_taps();

const fn analysis_taps_f32() -> [[f32; 64]; 5] {
    let mut w = [[0.0f32; 64]; 5];
    let mut j = 0;
    while j < 5 {
        let mut n = 0;
        while n < 64 {
            w[j][n] = ANALYSIS_TAP[j][n] as f32;
            n += 1;
        }
        j += 1;
    }
    w
}

/// Even window taps rounded to f32. The production analysis fold uses these.
const ANALYSIS_TAP_F32: [[f32; 64]; 5] = analysis_taps_f32();

const fn analysis_taps() -> [[f64; 64]; 5] {
    let mut w = [[0.0f64; 64]; 5];
    let mut j = 0;
    while j < 5 {
        let mut n = 0;
        while n < 64 {
            w[j][n] = QMF_WINDOW[2 * (n + 64 * j)];
            n += 1;
        }
        j += 1;
    }
    w
}

/// §4.6.18.4.1 / Figure 4.42 — the 32-band complex analysis QMF bank.
///
/// One instance carries the 320-sample input history `x` of one
/// channel; [`AnalysisQmf::push_slot`] consumes the next 32 time-domain
/// samples and produces the 32 complex subband samples `W[k][l]` of one
/// QMF slot.
#[derive(Debug, Clone)]
pub struct AnalysisQmf {
    /// The Figure 4.42 input history. Logical sample 0 is at [`Self::base`].
    /// f32: the core PCM is f32 and the production DFT consumes f32.
    x: Vec<f32>,
    /// Physical index of logical sample 0. A multiple of 32.
    base: usize,
}

impl Default for AnalysisQmf {
    fn default() -> Self {
        Self::new()
    }
}

impl AnalysisQmf {
    /// A fresh analysis bank with an all-zero history.
    #[must_use]
    pub fn new() -> Self {
        AnalysisQmf {
            x: vec![0.0; 320],
            base: 0,
        }
    }

    /// Run one Figure 4.42 loop: shift in 32 new time samples (oldest
    /// first within `samples`) and return the 32 complex subband
    /// samples `W[k]` for this slot.
    #[inline]
    pub fn push_slot(&mut self, samples: &[f32]) -> Result<[Complex; 32]> {
        if samples.len() != 32 {
            return Err(Error::SbrQmfInvalid);
        }
        // Rotate the history by 32 instead of sliding the 320-sample buffer.
        // Figure 4.42 fills logical `x[31] .. x[0]` from consecutive input
        // samples, so the newest input sample lands at logical index 0.
        self.base = if self.base == 0 { 288 } else { self.base - 32 };
        for (n, s) in samples.iter().enumerate() {
            self.x[self.base + 31 - n] = *s;
        }
        let u = analysis_fold_f32(&self.x, self.base);
        Ok(qmf_dct::analysis_modulate_f32(&u))
    }
}

/// `u[n] = Σ_{j=0..=4} x[n + 64j] · c[2(n + 64j)]` on the ring history.
#[cfg(test)]
fn analysis_fold(x: &[f64], base: usize) -> [f64; 64] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was probed. `x` has 320 samples and `base` is a
        // multiple of 32, so each 8-wide load stays inside the buffer.
        unsafe { return analysis_fold_avx512(x, base) }
    }
    analysis_fold_scalar(x, base)
}

#[cfg(test)]
fn analysis_fold_scalar(x: &[f64], base: usize) -> [f64; 64] {
    let mut u = [0.0f64; 64];
    for (n, un) in u.iter_mut().enumerate() {
        let mut acc = 0.0;
        for (j, tap) in ANALYSIS_TAP.iter().enumerate() {
            let i = base + n + j * 64;
            let i = if i >= 320 { i - 320 } else { i };
            acc += x[i] * tap[n];
        }
        *un = acc;
    }
    u
}

/// # Safety
///
/// AVX-512F is available. `x.len() == 320` and `base` is a multiple of 32
/// in `0..320`.
#[cfg(all(test, target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
unsafe fn analysis_fold_avx512(x: &[f64], base: usize) -> [f64; 64] {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_setzero_pd, _mm512_storeu_pd,
    };
    let mut u = [0.0f64; 64];
    for n in (0..64).step_by(8) {
        let mut acc = _mm512_setzero_pd();
        for (j, tap) in ANALYSIS_TAP.iter().enumerate() {
            let i = base + n + j * 64;
            let i = if i >= 320 { i - 320 } else { i };
            unsafe {
                let s = _mm512_loadu_pd(x.as_ptr().add(i));
                let w = _mm512_loadu_pd(tap.as_ptr().add(n));
                acc = _mm512_add_pd(acc, _mm512_mul_pd(s, w));
            }
        }
        unsafe {
            _mm512_storeu_pd(u.as_mut_ptr().add(n), acc);
        }
    }
    u
}

/// `u[n] = Σ_{j=0..=4} x[n + 64j] · c[2(n + 64j)]` on the f32 ring.
#[inline]
fn analysis_fold_f32(x: &[f32], base: usize) -> [f32; 64] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was probed. `x` has 320 samples and `base` is a
        // multiple of 32, so each 16-wide load stays inside the buffer.
        unsafe { return analysis_fold_f32_avx512(x, base) }
    }
    analysis_fold_f32_scalar(x, base)
}

fn analysis_fold_f32_scalar(x: &[f32], base: usize) -> [f32; 64] {
    let mut u = [0.0f32; 64];
    for (n, un) in u.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for (j, tap) in ANALYSIS_TAP_F32.iter().enumerate() {
            let i = base + n + j * 64;
            let i = if i >= 320 { i - 320 } else { i };
            acc += x[i] * tap[n];
        }
        *un = acc;
    }
    u
}

/// # Safety
///
/// AVX-512F is available. `x.len() == 320` and `base` is a multiple of 32
/// in `0..320`.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f")]
unsafe fn analysis_fold_f32_avx512(x: &[f32], base: usize) -> [f32; 64] {
    use std::arch::x86_64::{
        _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_setzero_ps, _mm512_storeu_ps,
    };
    let mut u = [0.0f32; 64];
    for n in (0..64).step_by(16) {
        let mut acc = _mm512_setzero_ps();
        for (j, tap) in ANALYSIS_TAP_F32.iter().enumerate() {
            let i = base + n + j * 64;
            let i = if i >= 320 { i - 320 } else { i };
            unsafe {
                let s = _mm512_loadu_ps(x.as_ptr().add(i));
                let w = _mm512_loadu_ps(tap.as_ptr().add(n));
                acc = _mm512_add_ps(acc, _mm512_mul_ps(s, w));
            }
        }
        unsafe {
            _mm512_storeu_ps(u.as_mut_ptr().add(n), acc);
        }
    }
    u
}

/// §4.6.18.4.2 / Figure 4.43 — the 64-band real-output synthesis QMF
/// bank (dual-rate SBR output).
#[derive(Debug, Clone)]
pub struct SynthesisQmf {
    /// The Figure 4.43 synthesis history `v`, 1280 samples.
    /// Stored as f32: the production DCT-IV is already f32, and the
    /// window multiplies in that width.
    v: Vec<f32>,
    /// Physical index of logical sample 0. A multiple of 128.
    base: usize,
}

impl Default for SynthesisQmf {
    fn default() -> Self {
        Self::new()
    }
}

impl SynthesisQmf {
    /// A fresh synthesis bank with an all-zero history.
    #[must_use]
    pub fn new() -> Self {
        SynthesisQmf {
            v: vec![0.0; 1280],
            base: 0,
        }
    }

    /// Run one Figure 4.43 loop: consume the 64 complex subband samples
    /// `X[k]` of one slot and return the 64 real output samples.
    #[inline]
    pub fn push_slot(&mut self, bands: &[Complex]) -> Result<[f32; 64]> {
        if bands.len() != 64 {
            return Err(Error::SbrQmfInvalid);
        }
        // Rotate the history by 128 instead of sliding the 1280-sample buffer.
        self.base = if self.base == 0 {
            1152
        } else {
            self.base - 128
        };
        let start = self.base;
        // The 128 new samples land in the ring. The window reads them there.
        qmf_dct::synthesis_modulate_f32_into(bands, &mut self.v[start..start + 128]);
        Ok(synth_window_f32(&self.v, self.base))
    }
}

const fn qmf_window_f32() -> [f32; 640] {
    let mut w = [0.0f32; 640];
    let mut i = 0;
    while i < 640 {
        w[i] = QMF_WINDOW[i] as f32;
        i += 1;
    }
    w
}

/// [`QMF_WINDOW`] rounded to f32. The production synthesis window uses this.
const QMF_WINDOW_F32: [f32; 640] = qmf_window_f32();

/// Ten-tap window on the f32 ring. `base` is a multiple of 128 and every
/// 16-wide load stays inside the 1280-sample buffer.
#[inline]
fn synth_window_f32(v: &[f32], base: usize) -> [f32; 64] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was probed. `v` has 1280 samples and `base` is a
        // multiple of 128, so each 16-wide load computed below stays in range.
        unsafe { return synth_window_f32_avx512(v, base) }
    }
    synth_window_f32_scalar(v, base)
}

fn synth_window_f32_scalar(v: &[f32], base: usize) -> [f32; 64] {
    let mut out = [0.0f32; 64];
    for (k, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for n in 0..5 {
            let i1 = base + 256 * n + k;
            let i1 = if i1 >= 1280 { i1 - 1280 } else { i1 };
            let i2 = base + 256 * n + 192 + k;
            let i2 = if i2 >= 1280 { i2 - 1280 } else { i2 };
            acc += v[i1] * QMF_WINDOW_F32[128 * n + k];
            acc += v[i2] * QMF_WINDOW_F32[128 * n + 64 + k];
        }
        *o = acc;
    }
    out
}

/// # Safety
///
/// AVX-512F is available. `v.len() == 1280` and `base` is a multiple of 128
/// in `0..1280`.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f")]
unsafe fn synth_window_f32_avx512(v: &[f32], base: usize) -> [f32; 64] {
    use std::arch::x86_64::{
        _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_setzero_ps, _mm512_storeu_ps,
    };
    let mut acc = [_mm512_setzero_ps(); 4];
    for n in 0..5 {
        for (chunk, a) in acc.iter_mut().enumerate() {
            let k = chunk * 16;
            let i1 = base + 256 * n + k;
            let i1 = if i1 >= 1280 { i1 - 1280 } else { i1 };
            let i2 = base + 256 * n + 192 + k;
            let i2 = if i2 >= 1280 { i2 - 1280 } else { i2 };
            unsafe {
                let s1 = _mm512_loadu_ps(v.as_ptr().add(i1));
                let w1 = _mm512_loadu_ps(QMF_WINDOW_F32.as_ptr().add(128 * n + k));
                let s2 = _mm512_loadu_ps(v.as_ptr().add(i2));
                let w2 = _mm512_loadu_ps(QMF_WINDOW_F32.as_ptr().add(128 * n + 64 + k));
                *a = _mm512_add_ps(*a, _mm512_mul_ps(s1, w1));
                *a = _mm512_add_ps(*a, _mm512_mul_ps(s2, w2));
            }
        }
    }
    let mut out = [0.0f32; 64];
    for (chunk, a) in acc.iter().enumerate() {
        unsafe {
            _mm512_storeu_ps(out.as_mut_ptr().add(chunk * 16), *a);
        }
    }
    out
}

/// Ten-tap window on the f64 ring history. Kept so the bit-identical
/// check still has a scalar reference. Logical index `i` is physical
/// `(base + i) mod 1280`. `base` is a multiple of 128 and every load of
/// eight samples stays inside the buffer.
#[cfg(test)]
fn synth_window(v: &[f64], base: usize) -> [f64; 64] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was probed. `v` has 1280 samples and `base` is a
        // multiple of 128, so each 8-wide load computed below stays in range.
        unsafe { return synth_window_avx512(v, base) }
    }
    synth_window_scalar(v, base)
}

#[cfg(test)]
fn synth_window_scalar(v: &[f64], base: usize) -> [f64; 64] {
    let mut out = [0.0f64; 64];
    for (k, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0;
        for n in 0..5 {
            let i1 = base + 256 * n + k;
            let i1 = if i1 >= 1280 { i1 - 1280 } else { i1 };
            let i2 = base + 256 * n + 192 + k;
            let i2 = if i2 >= 1280 { i2 - 1280 } else { i2 };
            acc += v[i1] * QMF_WINDOW[128 * n + k];
            acc += v[i2] * QMF_WINDOW[128 * n + 64 + k];
        }
        *o = acc;
    }
    out
}

/// # Safety
///
/// AVX-512F is available. `v.len() == 1280` and `base` is a multiple of 128
/// in `0..1280`.
#[cfg(all(test, target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
unsafe fn synth_window_avx512(v: &[f64], base: usize) -> [f64; 64] {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_setzero_pd, _mm512_storeu_pd,
    };
    let mut acc = [_mm512_setzero_pd(); 8];
    for n in 0..5 {
        for (chunk, a) in acc.iter_mut().enumerate() {
            let k = chunk * 8;
            let i1 = base + 256 * n + k;
            let i1 = if i1 >= 1280 { i1 - 1280 } else { i1 };
            let i2 = base + 256 * n + 192 + k;
            let i2 = if i2 >= 1280 { i2 - 1280 } else { i2 };
            unsafe {
                let s1 = _mm512_loadu_pd(v.as_ptr().add(i1));
                let w1 = _mm512_loadu_pd(QMF_WINDOW.as_ptr().add(128 * n + k));
                let s2 = _mm512_loadu_pd(v.as_ptr().add(i2));
                let w2 = _mm512_loadu_pd(QMF_WINDOW.as_ptr().add(128 * n + 64 + k));
                *a = _mm512_add_pd(*a, _mm512_mul_pd(s1, w1));
                *a = _mm512_add_pd(*a, _mm512_mul_pd(s2, w2));
            }
        }
    }
    let mut out = [0.0f64; 64];
    for (chunk, a) in acc.iter().enumerate() {
        unsafe {
            _mm512_storeu_pd(out.as_mut_ptr().add(chunk * 8), *a);
        }
    }
    out
}

/// §4.6.18.4.3 / Figure 4.44 — the 32-channel downsampled synthesis QMF
/// bank (output at the core rate).
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct DownsampledSynthesisQmf {
    /// The Figure 4.44 synthesis history `v`.
    v: Vec<f64>,
    /// Precomputed `exp(i·π/64·(k + 0.5)·(2n − 127.5)) / 64`, row-major
    /// `[n][k]`.
    n_mat: Vec<Complex>,
}

#[cfg(test)]
impl Default for DownsampledSynthesisQmf {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl DownsampledSynthesisQmf {
    /// A fresh downsampled synthesis bank with an all-zero history.
    #[must_use]
    pub fn new() -> Self {
        let mut n_mat = Vec::with_capacity(64 * 32);
        for n in 0..64 {
            for k in 0..32 {
                let arg =
                    core::f64::consts::PI / 64.0 * (k as f64 + 0.5) * (2.0 * n as f64 - 127.5);
                n_mat.push(Complex::new(arg.cos() / 64.0, arg.sin() / 64.0));
            }
        }
        DownsampledSynthesisQmf {
            v: vec![0.0; 640],
            n_mat,
        }
    }

    /// Run one Figure 4.44 loop: consume the 32 complex subband samples
    /// `X[k]` of one slot and return the 32 real output samples.
    pub fn push_slot(&mut self, bands: &[Complex]) -> Result<[f64; 32]> {
        if bands.len() != 32 {
            return Err(Error::SbrQmfInvalid);
        }
        // Shift v by 64 (discard the oldest 64 samples).
        self.v.copy_within(0..576, 64);
        // v[n] = Σ_k Real(X[k]/64 · exp(i·π/64·(k + 0.5)(2n − 127.5))).
        for n in 0..64 {
            let row = &self.n_mat[n * 32..(n + 1) * 32];
            let mut acc = 0.0;
            for (k, cell) in row.iter().enumerate() {
                let x = bands[k];
                acc += x.re * cell.re - x.im * cell.im;
            }
            self.v[n] = acc;
        }
        // g extraction, every-other-coefficient windowing, ten-tap sum.
        let mut out = [0.0f64; 32];
        for (k, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for n in 0..5 {
                // g[64n + k] = v[128n + k]; w[n] = g[n]·c[2n].
                acc += self.v[128 * n + k] * QMF_WINDOW[2 * (64 * n + k)];
                // g[64n + 32 + k] = v[128n + 96 + k].
                acc += self.v[128 * n + 96 + k] * QMF_WINDOW[2 * (64 * n + 32 + k)];
            }
            *o = acc;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Table 4.A.89 spot values, straight from the printed table.
    #[test]
    fn window_spot_values() {
        assert_eq!(QMF_WINDOW[0], 0.0);
        assert_eq!(QMF_WINDOW[1], -0.0005525286);
        assert_eq!(QMF_WINDOW[128], 0.0132718220);
        assert_eq!(QMF_WINDOW[320], 0.8537385600);
        assert_eq!(QMF_WINDOW[512], -0.0132718220);
        // The table prints c[639] with nine decimals.
        assert_eq!(QMF_WINDOW[639], -0.000552528);
    }

    /// The printed table mirrors around index 320:
    /// `|c[i]| == |c[640 - i]|` for every interior index (the last
    /// entry only to the table's own nine printed decimals).
    #[test]
    fn window_mirror_structure() {
        for i in 1..320usize {
            let a = QMF_WINDOW[i].abs();
            let b = QMF_WINDOW[640 - i].abs();
            assert!((a - b).abs() < 1e-9, "mirror mismatch at {i}: {a} vs {b}");
        }
    }

    /// The ring window matches the scalar sum on every base, and the
    /// 8-wide loads never cross the end of the buffer.
    #[test]
    fn synth_window_ring_matches_scalar_bits() {
        let mut state = 0x1234_5678u32;
        let mut v = vec![0.0f64; 1280];
        for slot in &mut v {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *slot = f64::from(state) / f64::from(u32::MAX) - 0.5;
        }
        for base in (0..1280).step_by(128) {
            for n in 0..5 {
                for k in (0..64).step_by(8) {
                    for off in [0usize, 192] {
                        let i = base + 256 * n + off + k;
                        let i = if i >= 1280 { i - 1280 } else { i };
                        assert!(i + 7 < 1280, "base={base} n={n} k={k} off={off} i={i}");
                    }
                }
            }
            let scalar = super::synth_window_scalar(&v, base);
            let fast = super::synth_window(&v, base);
            for (k, (&a, &b)) in scalar.iter().zip(fast.iter()).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "base={base} k={k}");
            }
        }
    }

    /// The f32 ring window matches its scalar sum, and the 16-wide loads
    /// stay inside the buffer on every base.
    #[test]
    fn synth_window_f32_ring_matches_scalar_bits() {
        for base in (0..1280).step_by(128) {
            for n in 0..5 {
                for k in (0..64).step_by(16) {
                    for off in [0usize, 192] {
                        let i = base + 256 * n + off + k;
                        let i = if i >= 1280 { i - 1280 } else { i };
                        assert!(i + 15 < 1280, "base={base} n={n} k={k} off={off} i={i}");
                    }
                }
            }
        }
        let mut state = 0x1234_5678u32;
        let mut v = vec![0.0f32; 1280];
        for slot in &mut v {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *slot = (state as f32) / (u32::MAX as f32) - 0.5;
        }
        for base in (0..1280).step_by(128) {
            let scalar = super::synth_window_f32_scalar(&v, base);
            let fast = super::synth_window_f32(&v, base);
            for (k, (&a, &b)) in scalar.iter().zip(fast.iter()).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "base={base} k={k}");
            }
        }
    }

    /// The f32 synthesis slot stays near the widened f64 production slot.
    /// A bad interleave is O(1); f32 combine rounding is far smaller.
    #[test]
    fn synthesis_f32_stays_near_widened_f64() {
        let u = named_u("random");
        let mut bands = [Complex::default(); 64];
        for k in 0..64 {
            bands[k] = Complex::new(u[k % 32], u[(k * 3) % 64] * 0.25);
        }
        let slow = qmf_dct::synthesis_modulate(&bands);
        let mut fast = [0.0f32; 128];
        qmf_dct::synthesis_modulate_f32_into(&bands, &mut fast);
        let mut max = 0.0f64;
        for n in 0..128 {
            max = max.max((f64::from(fast[n]) - slow[n]).abs());
        }
        assert!(max < 1e-3, "f32 synth abs {max}");
    }

    /// The analysis ring matches a sliding 320-sample buffer, and the
    /// 8-wide loads never cross the end of the buffer.
    #[test]
    fn analysis_ring_matches_linear_shift_bits() {
        for base in (0..320).step_by(32) {
            for n in (0..64).step_by(8) {
                for j in 0..5 {
                    let i = base + n + j * 64;
                    let i = if i >= 320 { i - 320 } else { i };
                    assert!(i + 7 < 320, "base={base} n={n} j={j} i={i}");
                }
            }
        }
        let mut state = 0x1234_5678u32;
        let mut v = vec![0.0f64; 320];
        for slot in &mut v {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *slot = f64::from(state) / f64::from(u32::MAX) - 0.5;
        }
        for base in (0..320).step_by(32) {
            let scalar = super::analysis_fold_scalar(&v, base);
            let fast = super::analysis_fold(&v, base);
            for (n, (&a, &b)) in scalar.iter().zip(fast.iter()).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "base={base} n={n}");
            }
        }
        for base in (0..320).step_by(32) {
            for n in (0..64).step_by(16) {
                for j in 0..5 {
                    let i = base + n + j * 64;
                    let i = if i >= 320 { i - 320 } else { i };
                    assert!(i + 15 < 320, "f32 base={base} n={n} j={j} i={i}");
                }
            }
        }
        let mut ring = AnalysisQmf::new();
        let mut linear = vec![0.0f32; 320];
        for _ in 0..24 {
            let mut samples = [0.0f32; 32];
            for s in &mut samples {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *s = (state as f32) / (u32::MAX as f32) - 0.5;
            }
            linear.copy_within(0..288, 32);
            for (n, s) in samples.iter().enumerate() {
                linear[31 - n] = *s;
            }
            let mut u = [0.0f32; 64];
            for (n, un) in u.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for j in 0..5 {
                    let idx = n + j * 64;
                    acc += linear[idx] * (QMF_WINDOW[2 * idx] as f32);
                }
                *un = acc;
            }
            let w_lin = qmf_dct::analysis_modulate_f32(&u);
            let w_ring = ring.push_slot(&samples).unwrap();
            for k in 0..32 {
                assert_eq!(w_lin[k].re.to_bits(), w_ring[k].re.to_bits(), "re {k}");
                assert_eq!(w_lin[k].im.to_bits(), w_ring[k].im.to_bits(), "im {k}");
            }
        }
    }

    /// Silence in → silence out, and slot-length validation.
    #[test]
    fn analysis_silence_and_shape() {
        let mut a = AnalysisQmf::new();
        assert!(matches!(a.push_slot(&[0.0; 16]), Err(Error::SbrQmfInvalid)));
        for _ in 0..4 {
            let w = a.push_slot(&[0.0; 32]).unwrap();
            assert!(w.iter().all(|c| c.re == 0.0 && c.im == 0.0));
        }
        let mut s = SynthesisQmf::new();
        assert!(matches!(
            s.push_slot(&[Complex::default(); 32]),
            Err(Error::SbrQmfInvalid)
        ));
        let out = s.push_slot(&[Complex::default(); 64]).unwrap();
        assert!(out.iter().all(|&x| x == 0.0));
    }

    /// A pure low-frequency sine through analysis → 64-band synthesis
    /// (upper 32 bands zero) reconstructs the 2×-upsampled sine to
    /// within the filterbank's near-perfect-reconstruction bound.
    #[test]
    fn analysis_synthesis_upsamples_a_sine() {
        let mut a = AnalysisQmf::new();
        let mut s = SynthesisQmf::new();
        let freq = 0.03; // cycles per input sample, well inside band 1
        let slots = 96;
        let mut output = Vec::new();
        for slot in 0..slots {
            let mut input = [0.0f32; 32];
            for (n, v) in input.iter_mut().enumerate() {
                let t = (slot * 32 + n) as f64;
                *v = (2.0 * core::f64::consts::PI * freq * t).sin() as f32;
            }
            let w = a.push_slot(&input).unwrap();
            let mut x = [Complex::default(); 64];
            x[..32].copy_from_slice(&w);
            output.extend_from_slice(&s.push_slot(&x).unwrap());
        }
        // Search the analysis+synthesis delay (in output samples) by
        // matching against the ideal upsampled sine, then measure the
        // steady-state error.
        let ideal =
            |t: f64, delay: f64| (2.0 * core::f64::consts::PI * freq * (t - delay) / 2.0).sin();
        let mut best = (f64::INFINITY, 0usize);
        for delay in 0..1200usize {
            let mut err = 0.0;
            let mut sig = 0.0;
            for (t, &out) in output.iter().enumerate().skip(1400) {
                let s = f64::from(out);
                let e = s - ideal(t as f64, delay as f64);
                err += e * e;
                sig += s * s;
            }
            let ratio = err / sig.max(1e-30);
            if ratio < best.0 {
                best = (ratio, delay);
            }
        }
        assert!(
            best.0 < 1e-4,
            "reconstruction error ratio {} at delay {}",
            best.0,
            best.1
        );
    }

    /// The downsampled synthesis bank reconstructs the input at the
    /// core rate (identity up to the filterbank delay).
    #[test]
    fn analysis_downsampled_synthesis_is_identity() {
        let mut a = AnalysisQmf::new();
        let mut s = DownsampledSynthesisQmf::new();
        let freq = 0.04;
        let slots = 96;
        let mut input_all = Vec::new();
        let mut output = Vec::new();
        for slot in 0..slots {
            let mut input = [0.0f32; 32];
            for (n, v) in input.iter_mut().enumerate() {
                let t = (slot * 32 + n) as f64;
                *v = ((2.0 * core::f64::consts::PI * freq * t).sin()
                    + 0.5 * (2.0 * core::f64::consts::PI * 2.3 * freq * t).cos())
                    as f32;
            }
            input_all.extend(input.iter().map(|&s| f64::from(s)));
            let w = a.push_slot(&input).unwrap();
            output.extend_from_slice(&s.push_slot(&w).unwrap());
        }
        let mut best = (f64::INFINITY, 0usize);
        for delay in 0..640usize {
            let mut err = 0.0;
            let mut sig = 0.0;
            for t in 800..output.len() {
                if t < delay {
                    continue;
                }
                let e = output[t] - input_all[t - delay];
                err += e * e;
                sig += output[t] * output[t];
            }
            let ratio = err / sig.max(1e-30);
            if ratio < best.0 {
                best = (ratio, delay);
            }
        }
        assert!(
            best.0 < 1e-4,
            "identity error ratio {} at delay {}",
            best.0,
            best.1
        );
    }

    /// The analysis bank is linear up to f32 DFT rounding:
    /// analysis(a + b) ≈ analysis(a) + analysis(b) slot by slot.
    /// The f64 path is checked against the GEMV at 1e-20.
    #[test]
    fn analysis_is_linear() {
        let mut qa = AnalysisQmf::new();
        let mut qb = AnalysisQmf::new();
        let mut qs = AnalysisQmf::new();
        for slot in 0..8 {
            let mut a = [0.0f32; 32];
            let mut b = [0.0f32; 32];
            let mut sum = [0.0f32; 32];
            for n in 0..32 {
                let t = (slot * 32 + n) as f64;
                a[n] = (0.11 * t).sin() as f32;
                b[n] = (0.031 * t + 1.0).cos() as f32;
                sum[n] = a[n] + b[n];
            }
            let wa = qa.push_slot(&a).unwrap();
            let wb = qb.push_slot(&b).unwrap();
            let ws = qs.push_slot(&sum).unwrap();
            for k in 0..32 {
                let d = ws[k] - (wa[k] + wb[k]);
                assert!(
                    d.norm_sqr() < 1e-8,
                    "slot={slot} k={k} err={}",
                    d.norm_sqr().sqrt()
                );
            }
        }
    }

    fn named_u(kind: &str) -> [f64; 64] {
        let mut u = [0.0f64; 64];
        match kind {
            "silence" => {}
            "impulse" => u[0] = 1.0,
            "tonal" => {
                for (n, slot) in u.iter_mut().enumerate() {
                    *slot = (0.17 * n as f64).sin();
                }
            }
            "random" => {
                let mut s = 0x9e3779b9u32;
                for slot in &mut u {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    *slot = (s as f64 / f64::from(u32::MAX)) * 2.0 - 1.0;
                }
            }
            _ => unreachable!(),
        }
        u
    }

    /// DCT-IV analysis matches the ISO GEMV on silence, impulse, tonal and
    /// random stacked windows.
    #[test]
    fn analysis_dct_matches_scalar_reference() {
        let m = analysis_m();
        for kind in ["silence", "impulse", "tonal", "random"] {
            let u = named_u(kind);
            let fast = qmf_dct::analysis_modulate_f64(&u);
            let slow = qmf_dct::analysis_modulate_ref(&u, m);
            for k in 0..32 {
                let d = fast[k] - slow[k];
                assert!(
                    d.norm_sqr() < 1e-20,
                    "{kind} k={k} err={}",
                    d.norm_sqr().sqrt()
                );
            }
        }
    }

    /// DCT-IV synthesis matches the ISO GEMV on the same named spectra,
    /// including a long 256-slot state walk.
    #[test]
    fn synthesis_dct_matches_scalar_reference() {
        let n_mat = synthesis_n_mat();
        for kind in ["silence", "impulse", "tonal", "random"] {
            let u = named_u(kind);
            let mut bands = [Complex::default(); 64];
            for k in 0..32 {
                bands[k] = Complex::new(u[k], u[32 + k]);
            }
            let fast = qmf_dct::synthesis_modulate_f64(&bands);
            let slow = qmf_dct::synthesis_modulate_ref(&bands, n_mat);
            for n in 0..128 {
                let e = (fast[n] - slow[n]).abs();
                assert!(e < 1e-12, "{kind} n={n} err={e}");
            }
        }
        let m = analysis_m();
        let mut hist = vec![0.0f64; 320];
        for slot in 0..256 {
            let mut input = [0.0f64; 32];
            for (n, v) in input.iter_mut().enumerate() {
                let t = (slot * 32 + n) as f64;
                *v = (0.03 * t).sin() + 0.2 * (0.11 * t).cos();
            }
            hist.copy_within(0..288, 32);
            for (n, s) in input.iter().enumerate() {
                hist[31 - n] = *s;
            }
            let mut u = [0.0f64; 64];
            for (n, un) in u.iter_mut().enumerate() {
                let mut acc = 0.0;
                for j in 0..5 {
                    let idx = n + j * 64;
                    acc += hist[idx] * QMF_WINDOW[2 * idx];
                }
                *un = acc;
            }
            let fast = qmf_dct::analysis_modulate_f64(&u);
            let slow = qmf_dct::analysis_modulate_ref(&u, m);
            for k in 0..32 {
                assert!((fast[k] - slow[k]).norm_sqr() < 1e-20, "slot={slot} k={k}");
            }
            let mut bands = [Complex::default(); 64];
            bands[..32].copy_from_slice(&fast);
            let vf = qmf_dct::synthesis_modulate_f64(&bands);
            let vs = qmf_dct::synthesis_modulate_ref(&bands, n_mat);
            for n in 0..128 {
                assert!((vf[n] - vs[n]).abs() < 1e-12, "slot={slot} n={n}");
            }
        }
    }

    /// The production DFT (f32 on AVX-512) stays near the f64 path.
    /// A wrong factorization is O(1); f32 rounding on these vectors is far smaller.
    #[test]
    fn production_qmf_stays_near_f64() {
        let mut max_an = 0.0f64;
        let mut max_sy = 0.0f64;
        for kind in ["silence", "impulse", "tonal", "random"] {
            let u = named_u(kind);
            let fast = qmf_dct::analysis_modulate(&u);
            let slow = qmf_dct::analysis_modulate_f64(&u);
            for k in 0..32 {
                max_an = max_an.max((fast[k] - slow[k]).norm_sqr().sqrt());
            }
            let mut bands = [Complex::default(); 64];
            for k in 0..32 {
                bands[k] = Complex::new(u[k], u[32 + k]);
            }
            let vf = qmf_dct::synthesis_modulate(&bands);
            let vs = qmf_dct::synthesis_modulate_f64(&bands);
            for n in 0..128 {
                max_sy = max_sy.max((vf[n] - vs[n]).abs());
            }
        }
        assert!(max_an < 1e-3, "analysis abs {max_an} (f32 DFT vs f64)");
        assert!(max_sy < 1e-3, "synthesis abs {max_sy} (f32 DFT vs f64)");
    }

    /// Isolated QMF kernel wall: GEMV reference vs DCT-IV, 4096 slots.
    /// Run: `cargo test --release --lib qmf_cpu_probe -- --ignored --nocapture`
    #[test]
    #[ignore = "manual TASK-81 QMF CPU probe"]
    fn qmf_cpu_probe() {
        let m = analysis_m();
        let n_mat = synthesis_n_mat();
        let mut u = named_u("random");
        let mut bands = [Complex::default(); 64];
        for k in 0..32 {
            bands[k] = Complex::new(u[k], u[32 + k]);
        }
        for k in 32..64 {
            bands[k] = Complex::new(u[k % 32] * 0.1, u[(k + 3) % 32] * 0.1);
        }
        const SLOTS: usize = 4096;
        for _ in 0..8 {
            std::hint::black_box(qmf_dct::analysis_modulate(&u));
            std::hint::black_box(qmf_dct::synthesis_modulate(&bands));
        }
        let t0 = std::time::Instant::now();
        for _ in 0..SLOTS {
            std::hint::black_box(qmf_dct::analysis_modulate_ref(&u, m));
            std::hint::black_box(qmf_dct::synthesis_modulate_ref(&bands, n_mat));
            u[0] += 1e-12;
        }
        let gemv_ns = t0.elapsed().as_nanos();
        let t1 = std::time::Instant::now();
        for _ in 0..SLOTS {
            std::hint::black_box(qmf_dct::analysis_modulate(&u));
            std::hint::black_box(qmf_dct::synthesis_modulate(&bands));
            u[0] += 1e-12;
        }
        let dct_ns = t1.elapsed().as_nanos();
        println!(
            "qmf_cpu_probe slots={SLOTS} gemv_ns={gemv_ns} dct_ns={dct_ns} ratio={}",
            dct_ns as f64 / gemv_ns as f64
        );
        assert!(
            dct_ns < gemv_ns,
            "DCT-IV QMF should not be slower than GEMV"
        );
    }
}
