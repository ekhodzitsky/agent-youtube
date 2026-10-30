//! IMDCT — ISO/IEC 14496-3 §4.6.11.3.1, plus an N/4 IFFT fast path.
//!
//! Pre/post twiddles live here. The fast path is proven against the naive sum
//! in `imdct_tests.rs`.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Test-only: skip 4/8-wide butterflies (scalar IEEE path).
static FORCE_SCALAR: AtomicBool = AtomicBool::new(false);
/// Test-only: skip AVX so the x86 SSE2 4-wide path is the one that runs.
#[allow(dead_code)] // read only on x86_64
static FORCE_NO_AVX: AtomicBool = AtomicBool::new(false);

#[path = "imdct_simd.rs"]
mod kernels;

#[cfg(target_arch = "x86_64")]
#[path = "imdct_avx512.rs"]
mod avx512k;

#[derive(Clone, Copy)]
#[repr(C)]
struct C {
    re: f32,
    im: f32,
}

impl C {
    fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

/// Bit-reversal permutation for an `n`-point radix-2 FFT (shared with the
/// forward MDCT).
pub(crate) fn bitrev_table(n: usize) -> Vec<u16> {
    let mut t = vec![0u16; n];
    let mut j = 0usize;
    for slot in t.iter_mut() {
        *slot = j as u16;
        let mut bit = n >> 1;
        while bit != 0 && j >= bit {
            j -= bit;
            bit >>= 1;
        }
        j += bit;
    }
    t
}

/// Twiddles `e^{+i·2πk/len}` per stage (shared with the forward MDCT, which
/// negates the imaginary half for the forward FFT). [`super::det_math::sincos`]
/// keeps the bits identical on every host.
pub(crate) fn twiddle_table(n: usize) -> (Vec<f32>, Vec<f32>) {
    super::det_math::twiddle_table(n)
}

/// Unnormalized inverse radix-2 FFT, SoA + precomputed twiddles. With the
/// twiddle imaginary parts negated this is the forward FFT (the MDCT uses
/// it that way). 4/8-wide SIMD (NEON / SSE2 / AVX) is bit-identical to scalar.
pub(crate) fn ifft_soa(
    re: &mut [f32],
    im: &mut [f32],
    bitrev: &[u16],
    tw_re: &[f32],
    tw_im: &[f32],
) {
    let wide = !FORCE_SCALAR.load(Ordering::Relaxed);
    ifft_soa_ex(re, im, bitrev, tw_re, tw_im, wide);
}

/// Holds process-wide FFT mode for encoder scalar/SSE2 identity tests.
#[cfg(test)]
pub(crate) struct FftModeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
static FFT_MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn take_fft_mode() -> FftModeGuard {
    let lock = FFT_MODE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    FftModeGuard { _lock: lock }
}

#[cfg(test)]
impl Drop for FftModeGuard {
    fn drop(&mut self) {
        FORCE_SCALAR.store(false, Ordering::SeqCst);
        FORCE_NO_AVX.store(false, Ordering::SeqCst);
    }
}

/// Force the scalar FFT until the guard drops.
#[cfg(test)]
pub(crate) fn fft_scalar() -> FftModeGuard {
    let g = take_fft_mode();
    FORCE_NO_AVX.store(false, Ordering::SeqCst);
    FORCE_SCALAR.store(true, Ordering::SeqCst);
    g
}

/// Force SSE2 (no AVX) until the guard drops. On aarch64 this is NEON 4-wide.
#[cfg(test)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) fn fft_sse2_only() -> FftModeGuard {
    let g = take_fft_mode();
    FORCE_SCALAR.store(false, Ordering::SeqCst);
    FORCE_NO_AVX.store(true, Ordering::SeqCst);
    g
}

/// Scalar-only FFT (tests and the `-sse2` fallback).
#[cfg(test)]
pub(crate) fn ifft_soa_scalar(
    re: &mut [f32],
    im: &mut [f32],
    bitrev: &[u16],
    tw_re: &[f32],
    tw_im: &[f32],
) {
    ifft_soa_ex(re, im, bitrev, tw_re, tw_im, false);
}

fn ifft_soa_ex(
    re: &mut [f32],
    im: &mut [f32],
    bitrev: &[u16],
    tw_re: &[f32],
    tw_im: &[f32],
    wide: bool,
) {
    let n = re.len();
    kernels::bitrev_swap(re, im, bitrev);
    #[cfg(target_arch = "x86_64")]
    if wide
        && std::is_x86_feature_detected!("avx512f")
        && !FORCE_NO_AVX.load(Ordering::Relaxed)
        && im.len() == n
        && n.is_power_of_two()
        && tw_re.len() >= n.saturating_sub(1)
        && tw_im.len() >= n.saturating_sub(1)
    {
        // SAFETY: AVX-512F is probed, n is a power of two, and the twiddle
        // table covers every stage. Lanes use the scalar mul/add/sub order.
        unsafe { avx512k::ifft_stages_avx512(re, im, tw_re, tw_im) };
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if wide
        && kernels::avx_ok()
        && !FORCE_NO_AVX.load(Ordering::Relaxed)
        && im.len() == n
        && n.is_power_of_two()
        && tw_re.len() >= n.saturating_sub(1)
        && tw_im.len() >= n.saturating_sub(1)
    {
        // SAFETY: AVX is probed, n is a power of two, and the twiddle
        // table covers every stage (bitrev_table / twiddle_table).
        unsafe { kernels::ifft_stages_avx(re, im, tw_re, tw_im) };
        return;
    }
    #[cfg(target_arch = "x86_64")]
    let wide = wide && kernels::sse2_ok();
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let wide = {
        let _ = wide;
        false
    };
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        for i in (0..n).step_by(len) {
            let mut k = 0usize;
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            if wide {
                while k + 4 <= half {
                    // SAFETY: 4-wide path is cfg'd to NEON (baseline aarch64)
                    // or probed SSE2; k+3 < half keeps loads in-range.
                    unsafe {
                        kernels::butterfly4(re, im, i, k, half, off, tw_re, tw_im);
                    }
                    k += 4;
                }
            }
            while k < half {
                let wr = tw_re[off + k];
                let wi = tw_im[off + k];
                let j = i + k + half;
                let tr = wr * re[j] - wi * im[j];
                let ti = wr * im[j] + wi * re[j];
                let ur = re[i + k];
                let ui = im[i + k];
                re[j] = ur - tr;
                im[j] = ui - ti;
                re[i + k] = ur + tr;
                im[i + k] = ui + ti;
                k += 1;
            }
        }
        off += half;
        len *= 2;
    }
}

/// Naive §4.6.11.3.1 sum.
///
/// `x[n] = (2/N) · Σ_k spec[k] · cos((2π/N)·(n + n0)·(k + 1/2))`
/// with `n0 = (N/2 + 1)/2`, `N = n_transform`.
#[cfg(test)]
#[must_use]
pub fn imdct_naive(spec: &[f64], n_transform: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; n_transform];
    imdct_naive_into(spec, &mut out);
    out
}

fn imdct_naive_into(spec: &[f64], out: &mut [f64]) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let n0 = (n / 2 + 1) as f64 / 2.0;
    let scale = 2.0 / n as f64;
    let step = 2.0 * std::f64::consts::PI / n as f64;
    for (n_i, slot) in out.iter_mut().enumerate() {
        let np = n_i as f64 + n0;
        let mut acc = 0.0f64;
        for (k, &c) in spec.iter().enumerate() {
            acc += c * (step * np * (k as f64 + 0.5)).cos();
        }
        *slot = scale * acc;
    }
}

fn naive_f32(spec: &[f32], out: &mut [f32]) {
    let spec64: Vec<f64> = spec.iter().copied().map(f64::from).collect();
    let mut tmp = vec![0.0f64; out.len()];
    imdct_naive_into(&spec64, &mut tmp);
    for (d, s) in out.iter_mut().zip(tmp) {
        *d = s as f32;
    }
}

/// Fast IMDCT into `out` (f32 time domain).
pub fn imdct_into_f32(spec: &[f32], out: &mut [f32]) {
    let n = out.len();
    if spec.len() != n / 2 {
        naive_f32(spec, out);
        return;
    }
    match n {
        256 => PLAN_256.apply_into(spec, out),
        1024 => PLAN_1024.apply_into(spec, out),
        2048 => PLAN_2048.apply_into(spec, out),
        _ => naive_f32(spec, out),
    }
}

/// Fast IMDCT into f64 `out` (tests / legacy).
#[cfg(test)]
pub fn imdct_into(spec: &[f64], out: &mut [f64]) {
    use std::cell::RefCell;
    thread_local! {
        static SPEC: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
        static TMP: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
    }
    SPEC.with(|sc| {
        TMP.with(|tc| {
            let mut s = sc.borrow_mut();
            let mut t = tc.borrow_mut();
            s.clear();
            s.extend(spec.iter().map(|&x| x as f32));
            t.resize(out.len(), 0.0);
            imdct_into_f32(&s, &mut t);
            for (d, &v) in out.iter_mut().zip(t.iter()) {
                *d = f64::from(v);
            }
        });
    });
}

/// Allocate and run [`imdct_into`].
#[cfg(test)]
#[must_use]
pub fn imdct(spec: &[f64], n_transform: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; n_transform];
    imdct_into(spec, &mut out);
    out
}

struct Plan {
    n: usize,
    pre: Vec<C>,
    post: Vec<C>,
    bitrev: Vec<u16>,
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
}

static PLAN_256: LazyLock<Plan> = LazyLock::new(|| Plan::new(256));
static PLAN_1024: LazyLock<Plan> = LazyLock::new(|| Plan::new(1024));
static PLAN_2048: LazyLock<Plan> = LazyLock::new(|| Plan::new(2048));

impl Plan {
    fn new(n: usize) -> Self {
        let n4 = n / 4;
        let two_pi_n = 2.0 * std::f32::consts::PI / n as f32;
        let mut pre = Vec::with_capacity(n4);
        let mut post = Vec::with_capacity(n4);
        for k in 0..n4 {
            let a = two_pi_n * (k as f32 + 0.125);
            let (s, c) = super::det_math::sincos(a);
            let cpx = C::new(c, s);
            pre.push(cpx);
            post.push(cpx);
        }
        let (tw_re, tw_im) = twiddle_table(n4);
        Self {
            n,
            pre,
            post,
            bitrev: bitrev_table(n4),
            tw_re,
            tw_im,
        }
    }

    fn apply_into(&self, spec: &[f32], out: &mut [f32]) {
        match self.n {
            2048 => {
                let mut re = [0.0f32; 512];
                let mut im = [0.0f32; 512];
                kernels::apply_soa(self, spec, out, &mut re, &mut im);
            }
            1024 => {
                let mut re = [0.0f32; 256];
                let mut im = [0.0f32; 256];
                kernels::apply_soa(self, spec, out, &mut re, &mut im);
            }
            256 => {
                let mut re = [0.0f32; 64];
                let mut im = [0.0f32; 64];
                kernels::apply_soa(self, spec, out, &mut re, &mut im);
            }
            _ => naive_f32(spec, out),
        }
    }
}

/// Pre, twiddle, and post tables of the 2048-point plan (host bit probe).
#[cfg(test)]
type PlanTables = (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>);

#[cfg(test)]
pub(crate) fn probe_plan_2048() -> PlanTables {
    let p = &*PLAN_2048;
    let split = |cs: &[C]| {
        let mut re = Vec::with_capacity(cs.len());
        let mut im = Vec::with_capacity(cs.len());
        for c in cs {
            re.push(c.re);
            im.push(c.im);
        }
        (re, im)
    };
    let (pre_re, pre_im) = split(&p.pre);
    let (post_re, post_im) = split(&p.post);
    (
        pre_re,
        pre_im,
        p.tw_re.clone(),
        p.tw_im.clone(),
        post_re,
        post_im,
    )
}

#[cfg(test)]
#[path = "imdct_tests.rs"]
mod imdct_tests;
