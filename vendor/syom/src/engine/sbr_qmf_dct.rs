//! Fast DFT factorization of the ISO SBR QMF modulation kernels.
//!
//! Figure 4.42 is one length-64 forward DFT. Figure 4.43 is a length-64
//! DCT-IV of `Re X` and a DST-IV of `Im X`, each one length-32 forward
//! DFT. Tables are interned from `det_math` (no host libm) and butterflies
//! stay FMA-free. The AVX-512 length-32 and length-64 DFTs are the same
//! radix-2 steps as the scalar transform, kept in registers, including
//! the bit reversal. On AVX-512 the QMF pre-twiddle, DFT, and post-twiddle
//! for those two lengths run in f32. The f64 kernels remain for the
//! bit-exact checks.

use super::Complex;
use std::sync::OnceLock;

// `_mm512_xor_ps` / `_mm512_xor_pd` are marked AVX-512DQ in this toolchain.
// From an AVX-512F function that is an out-of-line call plus `vzeroupper`.
// `vpxorq` is AVX-512F and has the same bits.
#[cfg(target_arch = "x86_64")]
macro_rules! xor512_ps {
    ($a:expr, $b:expr) => {{
        std::arch::x86_64::_mm512_castsi512_ps(std::arch::x86_64::_mm512_xor_si512(
            std::arch::x86_64::_mm512_castps_si512($a),
            std::arch::x86_64::_mm512_castps_si512($b),
        ))
    }};
}
#[cfg(target_arch = "x86_64")]
macro_rules! xor512_pd {
    ($a:expr, $b:expr) => {{
        std::arch::x86_64::_mm512_castsi512_pd(std::arch::x86_64::_mm512_xor_si512(
            std::arch::x86_64::_mm512_castpd_si512($a),
            std::arch::x86_64::_mm512_castpd_si512($b),
        ))
    }};
}

struct FftPlan {
    bitrev: Vec<u16>,
    tw_re: Vec<f64>,
    tw_im: Vec<f64>,
}

fn bitrev_table(n: usize) -> Vec<u16> {
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

fn fft_plan(n: usize) -> FftPlan {
    fft_plan_signed(n, -1.0)
}

/// `sign` is −1 for a forward DFT and +1 for an unnormalized inverse.
fn fft_plan_signed(n: usize, sign: f64) -> FftPlan {
    let mut tw_re = Vec::new();
    let mut tw_im = Vec::new();
    let mut len = 2usize;
    while len <= n {
        let half = len / 2;
        let ang = sign * 2.0 * std::f64::consts::PI / len as f64;
        for k in 0..half {
            let a = ang * k as f64;
            let (s, c) = super::super::det_math::sincos_f64(a);
            tw_re.push(c);
            tw_im.push(s);
        }
        len *= 2;
    }
    FftPlan {
        bitrev: bitrev_table(n),
        tw_re,
        tw_im,
    }
}

fn fft_in_place(re: &mut [f64], im: &mut [f64], plan: &FftPlan) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f")
        && ((re.len() == 32 && std::ptr::eq(plan, fft32()))
            || (re.len() == 64 && std::ptr::eq(plan, fft64())))
    {
        // SAFETY: AVX-512F was just probed. `plan` is the forward length
        // matched above, and `re`/`im` have that length.
        unsafe { fft_dit_avx512(re, im, plan) };
        return;
    }
    for (i, &rev) in plan.bitrev.iter().enumerate() {
        let j = rev as usize;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx512f") {
            // SAFETY: AVX-512F was just probed. `re`/`im` share `plan`'s length.
            unsafe { fft_stages_avx512(re, im, plan) };
            return;
        }
        if std::is_x86_feature_detected!("avx") {
            // SAFETY: AVX was just probed. `re`/`im` share `plan`'s length.
            unsafe { fft_stages_avx(re, im, plan) };
            return;
        }
    }
    fft_stages_scalar(re, im, plan);
}

fn fft_stages_scalar(re: &mut [f64], im: &mut [f64], plan: &FftPlan) {
    let n = re.len();
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        for i in (0..n).step_by(len) {
            for k in 0..half {
                butterfly_scalar(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
            }
        }
        off += half;
        len *= 2;
    }
}

/// One radix-2 butterfly. Mul then add/sub, never FMA.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn butterfly_scalar(
    re: &mut [f64],
    im: &mut [f64],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f64],
    tw_im: &[f64],
) {
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
}

/// Four independent f64 butterflies. Lanes match [`butterfly_scalar`].
///
/// # Safety
///
/// AVX is available. `k + 3 < half`, and `i + k + 3` and `i + k + half + 3`
/// are inside `re` and `im`. `tw_re`/`tw_im` cover `off + k + 3`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[allow(clippy::too_many_arguments)]
unsafe fn butterfly4_f64(
    re: &mut [f64],
    im: &mut [f64],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f64],
    tw_im: &[f64],
) {
    use std::arch::x86_64::{
        _mm256_add_pd, _mm256_loadu_pd, _mm256_mul_pd, _mm256_storeu_pd, _mm256_sub_pd,
    };
    let j = i + k + half;
    unsafe {
        let wr = _mm256_loadu_pd(tw_re.as_ptr().add(off + k));
        let wi = _mm256_loadu_pd(tw_im.as_ptr().add(off + k));
        let rj = _mm256_loadu_pd(re.as_ptr().add(j));
        let ij = _mm256_loadu_pd(im.as_ptr().add(j));
        let tr = _mm256_sub_pd(_mm256_mul_pd(wr, rj), _mm256_mul_pd(wi, ij));
        let ti = _mm256_add_pd(_mm256_mul_pd(wr, ij), _mm256_mul_pd(wi, rj));
        let ur = _mm256_loadu_pd(re.as_ptr().add(i + k));
        let ui = _mm256_loadu_pd(im.as_ptr().add(i + k));
        _mm256_storeu_pd(re.as_mut_ptr().add(j), _mm256_sub_pd(ur, tr));
        _mm256_storeu_pd(im.as_mut_ptr().add(j), _mm256_sub_pd(ui, ti));
        _mm256_storeu_pd(re.as_mut_ptr().add(i + k), _mm256_add_pd(ur, tr));
        _mm256_storeu_pd(im.as_mut_ptr().add(i + k), _mm256_add_pd(ui, ti));
    }
}

/// # Safety
///
/// AVX is available. `re.len() == im.len()` equals the plan length, a power of two.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn fft_stages_avx(re: &mut [f64], im: &mut [f64], plan: &FftPlan) {
    let n = re.len();
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        let mut i = 0usize;
        while i < n {
            if half < 4 {
                for k in 0..half {
                    butterfly_scalar(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
                }
            } else {
                let mut k = 0usize;
                while k < half {
                    // SAFETY: half is a power of two and at least 4, so k
                    // steps of 4 stay inside the stage and the twiddle row.
                    unsafe {
                        butterfly4_f64(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
                    }
                    k += 4;
                }
            }
            i += len;
        }
        off += half;
        len *= 2;
    }
}

/// Eight independent f64 butterflies. Lanes match [`butterfly_scalar`].
///
/// # Safety
///
/// AVX-512F is available. `k + 7 < half`, and `i + k + 7` and
/// `i + k + half + 7` are inside `re` and `im`. `tw_re`/`tw_im` cover
/// `off + k + 7`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[allow(clippy::too_many_arguments)]
unsafe fn butterfly8_f64(
    re: &mut [f64],
    im: &mut [f64],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f64],
    tw_im: &[f64],
) {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_storeu_pd, _mm512_sub_pd,
    };
    let j = i + k + half;
    unsafe {
        let wr = _mm512_loadu_pd(tw_re.as_ptr().add(off + k));
        let wi = _mm512_loadu_pd(tw_im.as_ptr().add(off + k));
        let rj = _mm512_loadu_pd(re.as_ptr().add(j));
        let ij = _mm512_loadu_pd(im.as_ptr().add(j));
        let tr = _mm512_sub_pd(_mm512_mul_pd(wr, rj), _mm512_mul_pd(wi, ij));
        let ti = _mm512_add_pd(_mm512_mul_pd(wr, ij), _mm512_mul_pd(wi, rj));
        let ur = _mm512_loadu_pd(re.as_ptr().add(i + k));
        let ui = _mm512_loadu_pd(im.as_ptr().add(i + k));
        _mm512_storeu_pd(re.as_mut_ptr().add(j), _mm512_sub_pd(ur, tr));
        _mm512_storeu_pd(im.as_mut_ptr().add(j), _mm512_sub_pd(ui, ti));
        _mm512_storeu_pd(re.as_mut_ptr().add(i + k), _mm512_add_pd(ur, tr));
        _mm512_storeu_pd(im.as_mut_ptr().add(i + k), _mm512_add_pd(ui, ti));
    }
}

/// # Safety
///
/// AVX-512F and AVX are available. `re.len() == im.len()` equals the plan length, a power of two.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
unsafe fn fft_stages_avx512(re: &mut [f64], im: &mut [f64], plan: &FftPlan) {
    let n = re.len();
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        let mut i = 0usize;
        while i < n {
            if half >= 8 {
                let mut k = 0usize;
                while k < half {
                    // SAFETY: half is a power of two and at least 8, so k
                    // steps of 8 stay inside the stage and the twiddle row.
                    unsafe {
                        butterfly8_f64(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
                    }
                    k += 8;
                }
            } else if half >= 4 {
                let mut k = 0usize;
                while k < half {
                    // SAFETY: half is 4, so one AVX butterfly covers the row.
                    unsafe {
                        butterfly4_f64(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
                    }
                    k += 4;
                }
            } else {
                for k in 0..half {
                    butterfly_scalar(re, im, i, k, half, off, &plan.tw_re, &plan.tw_im);
                }
            }
            i += len;
        }
        off += half;
        len *= 2;
    }
}

/// In-register decimation-in-time FFT for length 32 or 64.
///
/// Bit reversal matches [`bitrev_table`]. The butterflies are expanded in
/// this function: every zmm is caller-saved, and `inline(always)` cannot
/// be combined with `target_feature`, so a helper call spills the working
/// set. A length-64 transform runs each half through the length-32 stages
/// in eight registers, then one cross stage. Twiddle offsets follow
/// [`fft_plan_signed`]. Mul then add/sub, never FMA. The length-2 twiddle
/// is multiplied even though it is `(1, 0)`, so signed zeros match.
///
/// # Safety
///
/// AVX-512F and AVX are available. `re.len() == im.len()` is 32 or 64 and
/// equals the plan length. `plan` was built by [`fft_plan`] (forward sign).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
unsafe fn fft_dit_avx512(re: &mut [f64], im: &mut [f64], plan: &FftPlan) {
    macro_rules! vadd {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_add_pd($a, $b)
        };
    }
    macro_rules! vsub {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_sub_pd($a, $b)
        };
    }
    macro_rules! vmul {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_mul_pd($a, $b)
        };
    }
    macro_rules! yadd {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_add_pd($a, $b)
        };
    }
    macro_rules! ysub {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_sub_pd($a, $b)
        };
    }
    macro_rules! ymul {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_mul_pd($a, $b)
        };
    }
    // Length-2 butterflies inside one register. Even lanes are the low leg.
    // The twiddle is valid on even lanes only; odd lanes take the swapped product.
    macro_rules! stage2 {
        ($re:ident, $im:ident, $wr:expr, $wi:expr, $swap:expr) => {
            let jr = std::arch::x86_64::_mm512_permutexvar_pd($swap, $re);
            let ji = std::arch::x86_64::_mm512_permutexvar_pd($swap, $im);
            let tr = vsub!(vmul!($wr, jr), vmul!($wi, ji));
            let ti = vadd!(vmul!($wr, ji), vmul!($wi, jr));
            let tr_s = std::arch::x86_64::_mm512_permutexvar_pd($swap, tr);
            let ti_s = std::arch::x86_64::_mm512_permutexvar_pd($swap, ti);
            $re = std::arch::x86_64::_mm512_mask_blend_pd(
                0b1010_1010,
                vadd!($re, tr),
                vsub!(jr, tr_s),
            );
            $im = std::arch::x86_64::_mm512_mask_blend_pd(
                0b1010_1010,
                vadd!($im, ti),
                vsub!(ji, ti_s),
            );
        };
    }
    // Length-4 butterflies inside one register. `wr`/`wi` lanes are w0, w1 repeated.
    macro_rules! stage4 {
        ($re:ident, $im:ident, $wr:expr, $wi:expr, $idx_u:expr, $idx_j:expr, $back:expr) => {
            let ur = std::arch::x86_64::_mm512_permutexvar_pd($idx_u, $re);
            let jr = std::arch::x86_64::_mm512_permutexvar_pd($idx_j, $re);
            let ui = std::arch::x86_64::_mm512_permutexvar_pd($idx_u, $im);
            let ji = std::arch::x86_64::_mm512_permutexvar_pd($idx_j, $im);
            let tr = vsub!(vmul!($wr, jr), vmul!($wi, ji));
            let ti = vadd!(vmul!($wr, ji), vmul!($wi, jr));
            $re = std::arch::x86_64::_mm512_permutex2var_pd(vadd!(ur, tr), $back, vsub!(ur, tr));
            $im = std::arch::x86_64::_mm512_permutex2var_pd(vadd!(ui, ti), $back, vsub!(ui, ti));
        };
    }
    // Length-8 butterflies: the low four lanes against the high four.
    macro_rules! stage8 {
        ($re:ident, $im:ident, $wr:expr, $wi:expr) => {
            let ur = std::arch::x86_64::_mm512_extractf64x4_pd($re, 0);
            let jr = std::arch::x86_64::_mm512_extractf64x4_pd($re, 1);
            let ui = std::arch::x86_64::_mm512_extractf64x4_pd($im, 0);
            let ji = std::arch::x86_64::_mm512_extractf64x4_pd($im, 1);
            let tr = ysub!(ymul!($wr, jr), ymul!($wi, ji));
            let ti = yadd!(ymul!($wr, ji), ymul!($wi, jr));
            $re = std::arch::x86_64::_mm512_insertf64x4(
                std::arch::x86_64::_mm512_castpd256_pd512(yadd!(ur, tr)),
                ysub!(ur, tr),
                1,
            );
            $im = std::arch::x86_64::_mm512_insertf64x4(
                std::arch::x86_64::_mm512_castpd256_pd512(yadd!(ui, ti)),
                ysub!(ui, ti),
                1,
            );
        };
    }
    macro_rules! bfly {
        ($ur:ident, $ui:ident, $jr:ident, $ji:ident, $wr:expr, $wi:expr) => {
            let tr = vsub!(vmul!($wr, $jr), vmul!($wi, $ji));
            let ti = vadd!(vmul!($wr, $ji), vmul!($wi, $jr));
            let nur = vadd!($ur, tr);
            let nui = vadd!($ui, ti);
            let njr = vsub!($ur, tr);
            let nji = vsub!($ui, ti);
            $ur = nur;
            $ui = nui;
            $jr = njr;
            $ji = nji;
        };
    }
    macro_rules! load4 {
        ($re:ident, $im:ident, $base:expr) => {{
            // SAFETY: `$base`..`$base + 32` lies inside this 32- or 64-length slice.
            unsafe {
                let pr = $re.as_ptr().add($base);
                let pi = $im.as_ptr().add($base);
                (
                    std::arch::x86_64::_mm512_loadu_pd(pr),
                    std::arch::x86_64::_mm512_loadu_pd(pi),
                    std::arch::x86_64::_mm512_loadu_pd(pr.add(8)),
                    std::arch::x86_64::_mm512_loadu_pd(pi.add(8)),
                    std::arch::x86_64::_mm512_loadu_pd(pr.add(16)),
                    std::arch::x86_64::_mm512_loadu_pd(pi.add(16)),
                    std::arch::x86_64::_mm512_loadu_pd(pr.add(24)),
                    std::arch::x86_64::_mm512_loadu_pd(pi.add(24)),
                )
            }
        }};
    }
    macro_rules! store4 {
        (
            $re:ident,
            $im:ident,
            $base:expr,
            $r0:ident,
            $i0:ident,
            $r1:ident,
            $i1:ident,
            $r2:ident,
            $i2:ident,
            $r3:ident,
            $i3:ident
        ) => {
            // SAFETY: same range as the load of these four registers.
            unsafe {
                let pr = $re.as_mut_ptr().add($base);
                let pi = $im.as_mut_ptr().add($base);
                std::arch::x86_64::_mm512_storeu_pd(pr, $r0);
                std::arch::x86_64::_mm512_storeu_pd(pi, $i0);
                std::arch::x86_64::_mm512_storeu_pd(pr.add(8), $r1);
                std::arch::x86_64::_mm512_storeu_pd(pi.add(8), $i1);
                std::arch::x86_64::_mm512_storeu_pd(pr.add(16), $r2);
                std::arch::x86_64::_mm512_storeu_pd(pi.add(16), $i2);
                std::arch::x86_64::_mm512_storeu_pd(pr.add(24), $r3);
                std::arch::x86_64::_mm512_storeu_pd(pi.add(24), $i3);
            }
        };
    }
    // Stages of length 2, 4, 8, 16 and 32 on one half (four complex registers).
    macro_rules! fft32_regs {
        (
            $plan:ident,
            $r0:ident,
            $i0:ident,
            $r1:ident,
            $i1:ident,
            $r2:ident,
            $i2:ident,
            $r3:ident,
            $i3:ident
        ) => {{
            let wr2 = std::arch::x86_64::_mm512_set1_pd($plan.tw_re[0]);
            let wi2 = std::arch::x86_64::_mm512_set1_pd($plan.tw_im[0]);
            let swap = std::arch::x86_64::_mm512_setr_epi64(1, 0, 3, 2, 5, 4, 7, 6);
            stage2!($r0, $i0, wr2, wi2, swap);
            stage2!($r1, $i1, wr2, wi2, swap);
            stage2!($r2, $i2, wr2, wi2, swap);
            stage2!($r3, $i3, wr2, wi2, swap);
            let wr4 = std::arch::x86_64::_mm512_setr_pd(
                $plan.tw_re[1],
                $plan.tw_re[2],
                $plan.tw_re[1],
                $plan.tw_re[2],
                $plan.tw_re[1],
                $plan.tw_re[2],
                $plan.tw_re[1],
                $plan.tw_re[2],
            );
            let wi4 = std::arch::x86_64::_mm512_setr_pd(
                $plan.tw_im[1],
                $plan.tw_im[2],
                $plan.tw_im[1],
                $plan.tw_im[2],
                $plan.tw_im[1],
                $plan.tw_im[2],
                $plan.tw_im[1],
                $plan.tw_im[2],
            );
            let idx_u = std::arch::x86_64::_mm512_setr_epi64(0, 1, 4, 5, 0, 1, 4, 5);
            let idx_j = std::arch::x86_64::_mm512_setr_epi64(2, 3, 6, 7, 2, 3, 6, 7);
            let back = std::arch::x86_64::_mm512_setr_epi64(0, 1, 8, 9, 2, 3, 10, 11);
            stage4!($r0, $i0, wr4, wi4, idx_u, idx_j, back);
            stage4!($r1, $i1, wr4, wi4, idx_u, idx_j, back);
            stage4!($r2, $i2, wr4, wi4, idx_u, idx_j, back);
            stage4!($r3, $i3, wr4, wi4, idx_u, idx_j, back);
            // SAFETY: a forward plan of length 32 or 64 has four length-8 twiddles at offset 3.
            let wr8 = unsafe { std::arch::x86_64::_mm256_loadu_pd($plan.tw_re.as_ptr().add(3)) };
            let wi8 = unsafe { std::arch::x86_64::_mm256_loadu_pd($plan.tw_im.as_ptr().add(3)) };
            stage8!($r0, $i0, wr8, wi8);
            stage8!($r1, $i1, wr8, wi8);
            stage8!($r2, $i2, wr8, wi8);
            stage8!($r3, $i3, wr8, wi8);
            // SAFETY: length-16 twiddles occupy offsets 7..14, length-32 occupy 15..30.
            let w16 = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_re.as_ptr().add(7)) };
            let v16 = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_im.as_ptr().add(7)) };
            bfly!($r0, $i0, $r1, $i1, w16, v16);
            bfly!($r2, $i2, $r3, $i3, w16, v16);
            let w32a = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_re.as_ptr().add(15)) };
            let v32a = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_im.as_ptr().add(15)) };
            let w32b = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_re.as_ptr().add(23)) };
            let v32b = unsafe { std::arch::x86_64::_mm512_loadu_pd($plan.tw_im.as_ptr().add(23)) };
            bfly!($r0, $i0, $r2, $i2, w32a, v32a);
            bfly!($r1, $i1, $r3, $i3, w32b, v32b);
        }};
    }
    // Last stage of a length-64 FFT: one register from memory against one live register.
    macro_rules! bfly_hi {
        ($re:ident, $im:ident, $plan:ident, $lo:expr, $jr:ident, $ji:ident, $tw:expr) => {
            // SAFETY: `$lo` is 0, 8, 16 or 24, `$tw` is inside the length-64 twiddle row,
            // and both slices hold 64 lanes.
            unsafe {
                let ur = std::arch::x86_64::_mm512_loadu_pd($re.as_ptr().add($lo));
                let ui = std::arch::x86_64::_mm512_loadu_pd($im.as_ptr().add($lo));
                let wr = std::arch::x86_64::_mm512_loadu_pd($plan.tw_re.as_ptr().add($tw));
                let wi = std::arch::x86_64::_mm512_loadu_pd($plan.tw_im.as_ptr().add($tw));
                let tr = vsub!(vmul!(wr, $jr), vmul!(wi, $ji));
                let ti = vadd!(vmul!(wr, $ji), vmul!(wi, $jr));
                std::arch::x86_64::_mm512_storeu_pd($re.as_mut_ptr().add($lo), vadd!(ur, tr));
                std::arch::x86_64::_mm512_storeu_pd($im.as_mut_ptr().add($lo), vadd!(ui, ti));
                std::arch::x86_64::_mm512_storeu_pd($re.as_mut_ptr().add($lo + 32), vsub!(ur, tr));
                std::arch::x86_64::_mm512_storeu_pd($im.as_mut_ptr().add($lo + 32), vsub!(ui, ti));
            }
        };
    }

    let n = re.len();
    // SAFETY: `plan.bitrev` is a permutation of `0..n`, and both slices have
    // that length. Swaps stay in range; the checks would be on the hot path.
    unsafe {
        let rev = plan.bitrev.as_ptr();
        let rp = re.as_mut_ptr();
        let ip = im.as_mut_ptr();
        for i in 0..n {
            let j = usize::from(*rev.add(i));
            if i < j {
                std::ptr::swap(rp.add(i), rp.add(j));
                std::ptr::swap(ip.add(i), ip.add(j));
            }
        }
    }
    if n == 32 {
        let (mut r0, mut i0, mut r1, mut i1, mut r2, mut i2, mut r3, mut i3) = load4!(re, im, 0);
        fft32_regs!(plan, r0, i0, r1, i1, r2, i2, r3, i3);
        store4!(re, im, 0, r0, i0, r1, i1, r2, i2, r3, i3);
        return;
    }
    let (mut r0, mut i0, mut r1, mut i1, mut r2, mut i2, mut r3, mut i3) = load4!(re, im, 0);
    fft32_regs!(plan, r0, i0, r1, i1, r2, i2, r3, i3);
    store4!(re, im, 0, r0, i0, r1, i1, r2, i2, r3, i3);
    (r0, i0, r1, i1, r2, i2, r3, i3) = load4!(re, im, 32);
    fft32_regs!(plan, r0, i0, r1, i1, r2, i2, r3, i3);
    bfly_hi!(re, im, plan, 0, r0, i0, 31);
    bfly_hi!(re, im, plan, 8, r1, i1, 39);
    bfly_hi!(re, im, plan, 16, r2, i2, 47);
    bfly_hi!(re, im, plan, 24, r3, i3, 55);
}

#[cfg(target_arch = "x86_64")]
struct FftPlanF32 {
    /// Used by the scalar f32 reference. The AVX-512 kernel hardcodes the same permutation.
    #[cfg_attr(not(test), allow(dead_code))]
    bitrev: Vec<u16>,
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
}

/// Forward plan with twiddles rounded once from the f64 `det_math` tables.
#[cfg(target_arch = "x86_64")]
fn fft_plan_f32(n: usize) -> FftPlanF32 {
    let plan = fft_plan(n);
    FftPlanF32 {
        bitrev: plan.bitrev,
        tw_re: plan.tw_re.iter().map(|v| *v as f32).collect(),
        tw_im: plan.tw_im.iter().map(|v| *v as f32).collect(),
    }
}

#[cfg(target_arch = "x86_64")]
fn fft32_f32() -> &'static FftPlanF32 {
    static P: OnceLock<FftPlanF32> = OnceLock::new();
    P.get_or_init(|| fft_plan_f32(32))
}

#[cfg(target_arch = "x86_64")]
fn fft64_f32() -> &'static FftPlanF32 {
    static P: OnceLock<FftPlanF32> = OnceLock::new();
    P.get_or_init(|| fft_plan_f32(64))
}

/// Round `src` to f32. Length is a positive multiple of 8.
///
/// # Safety
///
/// AVX-512F is available. `dst.len() == src.len()`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn cvt_f64_to_f32(src: &[f64], dst: &mut [f32]) {
    use std::arch::x86_64::{_mm256_storeu_ps, _mm512_cvtpd_ps, _mm512_loadu_pd};
    let mut i = 0usize;
    while i < src.len() {
        // SAFETY: `i` steps by 8 and both slices share that length.
        unsafe {
            let pd = _mm512_loadu_pd(src.as_ptr().add(i));
            _mm256_storeu_ps(dst.as_mut_ptr().add(i), _mm512_cvtpd_ps(pd));
        }
        i += 8;
    }
}

/// Widen `src` back to f64. Length is a positive multiple of 8.
///
/// # Safety
///
/// AVX-512F is available. `dst.len() == src.len()`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn cvt_f32_to_f64(src: &[f32], dst: &mut [f64]) {
    use std::arch::x86_64::{_mm256_loadu_ps, _mm512_cvtps_pd, _mm512_storeu_pd};
    let mut i = 0usize;
    while i < src.len() {
        // SAFETY: `i` steps by 8 and both slices share that length.
        unsafe {
            let ps = _mm256_loadu_ps(src.as_ptr().add(i));
            _mm512_storeu_pd(dst.as_mut_ptr().add(i), _mm512_cvtps_pd(ps));
        }
        i += 8;
    }
}

/// In-register decimation-in-time FFT for length 32 or 64, in f32.
///
/// Bit reversal matches [`bitrev_table`] (`out[i] = in[bitrev(i)]`) and is
/// a permute, not a scalar swap loop. A 16-lane register holds one block
/// of 16 complexes' real or imaginary part. Stages of length 2, 4, 8 and
/// 16 stay inside that register; length 32 pairs adjacent registers;
/// length 64 pairs the two halves. Twiddle offsets follow
/// [`fft_plan_signed`], rounded to f32. Mul then add/sub, never FMA. The
/// length-2 twiddle is multiplied even though it is `(1, 0)`.
///
/// # Safety
///
/// AVX-512F and AVX are available. `re.len() == im.len()` is 32 or 64 and
/// equals the plan length. `plan` was built by [`fft_plan_f32`].
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
#[inline]
unsafe fn fft_dit_f32_avx512(re: &mut [f32], im: &mut [f32], plan: &FftPlanF32) {
    macro_rules! vadd {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_add_ps($a, $b)
        };
    }
    macro_rules! vsub {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_sub_ps($a, $b)
        };
    }
    macro_rules! vmul {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm512_mul_ps($a, $b)
        };
    }
    macro_rules! yadd {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_add_ps($a, $b)
        };
    }
    macro_rules! ysub {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_sub_ps($a, $b)
        };
    }
    macro_rules! ymul {
        ($a:expr, $b:expr) => {
            std::arch::x86_64::_mm256_mul_ps($a, $b)
        };
    }
    // Odd lanes (mask bit 1) take the high leg. 16 lanes, so the mask is 16 bits.
    macro_rules! stage2 {
        ($re:ident, $im:ident, $wr:expr, $wi:expr, $swap:expr) => {
            let jr = std::arch::x86_64::_mm512_permutexvar_ps($swap, $re);
            let ji = std::arch::x86_64::_mm512_permutexvar_ps($swap, $im);
            let tr = vsub!(vmul!($wr, jr), vmul!($wi, ji));
            let ti = vadd!(vmul!($wr, ji), vmul!($wi, jr));
            let tr_s = std::arch::x86_64::_mm512_permutexvar_ps($swap, tr);
            let ti_s = std::arch::x86_64::_mm512_permutexvar_ps($swap, ti);
            $re = std::arch::x86_64::_mm512_mask_blend_ps(
                0b1010_1010_1010_1010,
                vadd!($re, tr),
                vsub!(jr, tr_s),
            );
            $im = std::arch::x86_64::_mm512_mask_blend_ps(
                0b1010_1010_1010_1010,
                vadd!($im, ti),
                vsub!(ji, ti_s),
            );
        };
    }
    macro_rules! stage_perm {
        ($re:ident, $im:ident, $wr:expr, $wi:expr, $idx_u:expr, $idx_j:expr, $back:expr) => {
            let ur = std::arch::x86_64::_mm512_permutexvar_ps($idx_u, $re);
            let jr = std::arch::x86_64::_mm512_permutexvar_ps($idx_j, $re);
            let ui = std::arch::x86_64::_mm512_permutexvar_ps($idx_u, $im);
            let ji = std::arch::x86_64::_mm512_permutexvar_ps($idx_j, $im);
            let tr = vsub!(vmul!($wr, jr), vmul!($wi, ji));
            let ti = vadd!(vmul!($wr, ji), vmul!($wi, jr));
            $re = std::arch::x86_64::_mm512_permutex2var_ps(vadd!(ur, tr), $back, vsub!(ur, tr));
            $im = std::arch::x86_64::_mm512_permutex2var_ps(vadd!(ui, ti), $back, vsub!(ui, ti));
        };
    }
    // Low eight lanes against the high eight. extractf32x8 is AVX-512DQ;
    // the 256-bit extract/insert used here is AVX-512F, via a bit cast.
    macro_rules! stage16 {
        ($re:ident, $im:ident, $wr:expr, $wi:expr) => {
            let ur =
                std::arch::x86_64::_mm256_castpd_ps(std::arch::x86_64::_mm512_extractf64x4_pd(
                    std::arch::x86_64::_mm512_castps_pd($re),
                    0,
                ));
            let jr =
                std::arch::x86_64::_mm256_castpd_ps(std::arch::x86_64::_mm512_extractf64x4_pd(
                    std::arch::x86_64::_mm512_castps_pd($re),
                    1,
                ));
            let ui =
                std::arch::x86_64::_mm256_castpd_ps(std::arch::x86_64::_mm512_extractf64x4_pd(
                    std::arch::x86_64::_mm512_castps_pd($im),
                    0,
                ));
            let ji =
                std::arch::x86_64::_mm256_castpd_ps(std::arch::x86_64::_mm512_extractf64x4_pd(
                    std::arch::x86_64::_mm512_castps_pd($im),
                    1,
                ));
            let tr = ysub!(ymul!($wr, jr), ymul!($wi, ji));
            let ti = yadd!(ymul!($wr, ji), ymul!($wi, jr));
            $re = std::arch::x86_64::_mm512_castpd_ps(std::arch::x86_64::_mm512_insertf64x4(
                std::arch::x86_64::_mm512_castps_pd(std::arch::x86_64::_mm512_castps256_ps512(
                    yadd!(ur, tr),
                )),
                std::arch::x86_64::_mm256_castps_pd(ysub!(ur, tr)),
                1,
            ));
            $im = std::arch::x86_64::_mm512_castpd_ps(std::arch::x86_64::_mm512_insertf64x4(
                std::arch::x86_64::_mm512_castps_pd(std::arch::x86_64::_mm512_castps256_ps512(
                    yadd!(ui, ti),
                )),
                std::arch::x86_64::_mm256_castps_pd(ysub!(ui, ti)),
                1,
            ));
        };
    }
    macro_rules! bfly {
        ($ur:ident, $ui:ident, $jr:ident, $ji:ident, $wr:expr, $wi:expr) => {
            let tr = vsub!(vmul!($wr, $jr), vmul!($wi, $ji));
            let ti = vadd!(vmul!($wr, $ji), vmul!($wi, $jr));
            let nur = vadd!($ur, tr);
            let nui = vadd!($ui, ti);
            let njr = vsub!($ur, tr);
            let nji = vsub!($ui, ti);
            $ur = nur;
            $ui = nui;
            $jr = njr;
            $ji = nji;
        };
    }
    macro_rules! store2 {
        ($re:ident, $im:ident, $base:expr, $r0:ident, $i0:ident, $r1:ident, $i1:ident) => {
            // SAFETY: same range as the load of these two registers.
            unsafe {
                let pr = $re.as_mut_ptr().add($base);
                let pi = $im.as_mut_ptr().add($base);
                std::arch::x86_64::_mm512_storeu_ps(pr, $r0);
                std::arch::x86_64::_mm512_storeu_ps(pi, $i0);
                std::arch::x86_64::_mm512_storeu_ps(pr.add(16), $r1);
                std::arch::x86_64::_mm512_storeu_ps(pi.add(16), $i1);
            }
        };
    }
    // Length-32 DFT on four registers. Constants are local to this block so
    // they do not stay live across the two halves of a length-64 transform.
    macro_rules! fft32_ps {
        ($plan:ident, $r0:ident, $i0:ident, $r1:ident, $i1:ident) => {{
            let wr2 = std::arch::x86_64::_mm512_set1_ps($plan.tw_re[0]);
            let wi2 = std::arch::x86_64::_mm512_set1_ps($plan.tw_im[0]);
            let swap = std::arch::x86_64::_mm512_setr_epi32(
                1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14,
            );
            stage2!($r0, $i0, wr2, wi2, swap);
            stage2!($r1, $i1, wr2, wi2, swap);
            let wr4 = std::arch::x86_64::_mm512_broadcast_f32x4(std::arch::x86_64::_mm_setr_ps(
                $plan.tw_re[1],
                $plan.tw_re[2],
                $plan.tw_re[1],
                $plan.tw_re[2],
            ));
            let wi4 = std::arch::x86_64::_mm512_broadcast_f32x4(std::arch::x86_64::_mm_setr_ps(
                $plan.tw_im[1],
                $plan.tw_im[2],
                $plan.tw_im[1],
                $plan.tw_im[2],
            ));
            let u4 = std::arch::x86_64::_mm512_setr_epi32(
                0, 1, 4, 5, 8, 9, 12, 13, 0, 1, 4, 5, 8, 9, 12, 13,
            );
            let j4 = std::arch::x86_64::_mm512_setr_epi32(
                2, 3, 6, 7, 10, 11, 14, 15, 2, 3, 6, 7, 10, 11, 14, 15,
            );
            let b4 = std::arch::x86_64::_mm512_setr_epi32(
                0, 1, 16, 17, 2, 3, 18, 19, 4, 5, 20, 21, 6, 7, 22, 23,
            );
            stage_perm!($r0, $i0, wr4, wi4, u4, j4, b4);
            stage_perm!($r1, $i1, wr4, wi4, u4, j4, b4);
            // SAFETY: length-8 twiddles are at offset 3 (four lanes) and
            // length-16 twiddles at offset 7 (eight lanes).
            let (wr8, wi8, wr16, wi16) = unsafe {
                (
                    std::arch::x86_64::_mm512_broadcast_f32x4(std::arch::x86_64::_mm_loadu_ps(
                        $plan.tw_re.as_ptr().add(3),
                    )),
                    std::arch::x86_64::_mm512_broadcast_f32x4(std::arch::x86_64::_mm_loadu_ps(
                        $plan.tw_im.as_ptr().add(3),
                    )),
                    std::arch::x86_64::_mm256_loadu_ps($plan.tw_re.as_ptr().add(7)),
                    std::arch::x86_64::_mm256_loadu_ps($plan.tw_im.as_ptr().add(7)),
                )
            };
            let u8 = std::arch::x86_64::_mm512_setr_epi32(
                0, 1, 2, 3, 8, 9, 10, 11, 0, 1, 2, 3, 8, 9, 10, 11,
            );
            let j8 = std::arch::x86_64::_mm512_setr_epi32(
                4, 5, 6, 7, 12, 13, 14, 15, 4, 5, 6, 7, 12, 13, 14, 15,
            );
            let b8 = std::arch::x86_64::_mm512_setr_epi32(
                0, 1, 2, 3, 16, 17, 18, 19, 4, 5, 6, 7, 20, 21, 22, 23,
            );
            stage_perm!($r0, $i0, wr8, wi8, u8, j8, b8);
            stage_perm!($r1, $i1, wr8, wi8, u8, j8, b8);
            stage16!($r0, $i0, wr16, wi16);
            stage16!($r1, $i1, wr16, wi16);
            // SAFETY: length-32 twiddles occupy offsets 15..30.
            let (w32, v32) = unsafe {
                (
                    std::arch::x86_64::_mm512_loadu_ps($plan.tw_re.as_ptr().add(15)),
                    std::arch::x86_64::_mm512_loadu_ps($plan.tw_im.as_ptr().add(15)),
                )
            };
            bfly!($r0, $i0, $r1, $i1, w32, v32);
        }};
    }
    // Last stage of a length-64 FFT: one stored block against one live register.
    macro_rules! bfly_lo {
        ($re:ident, $im:ident, $plan:ident, $base:expr, $jr:ident, $ji:ident, $tw:expr) => {
            // SAFETY: `$base` is 0 or 16, `$tw` is inside the length-64 twiddle
            // row, and both slices hold 64 lanes.
            unsafe {
                let ur = std::arch::x86_64::_mm512_loadu_ps($re.as_ptr().add($base));
                let ui = std::arch::x86_64::_mm512_loadu_ps($im.as_ptr().add($base));
                let wr = std::arch::x86_64::_mm512_loadu_ps($plan.tw_re.as_ptr().add($tw));
                let wi = std::arch::x86_64::_mm512_loadu_ps($plan.tw_im.as_ptr().add($tw));
                let tr = vsub!(vmul!(wr, $jr), vmul!(wi, $ji));
                let ti = vadd!(vmul!(wr, $ji), vmul!(wi, $jr));
                std::arch::x86_64::_mm512_storeu_ps($re.as_mut_ptr().add($base), vadd!(ur, tr));
                std::arch::x86_64::_mm512_storeu_ps($im.as_mut_ptr().add($base), vadd!(ui, ti));
                std::arch::x86_64::_mm512_storeu_ps(
                    $re.as_mut_ptr().add($base + 32),
                    vsub!(ur, tr),
                );
                std::arch::x86_64::_mm512_storeu_ps(
                    $im.as_mut_ptr().add($base + 32),
                    vsub!(ui, ti),
                );
            }
        };
    }

    // `out[i] = in[bitrev(i)]`. For length 32 the index itself is the
    // two-register permute (bit 4 selects the high half). For length 64,
    // odd lanes come from the upper 32 inputs and even lanes from the lower.
    macro_rules! bitrev32 {
        ($re:ident, $im:ident) => {{
            // SAFETY: both slices hold 32 lanes.
            unsafe {
                let ra = std::arch::x86_64::_mm512_loadu_ps($re.as_ptr());
                let rb = std::arch::x86_64::_mm512_loadu_ps($re.as_ptr().add(16));
                let ia = std::arch::x86_64::_mm512_loadu_ps($im.as_ptr());
                let ib = std::arch::x86_64::_mm512_loadu_ps($im.as_ptr().add(16));
                let ilo = std::arch::x86_64::_mm512_setr_epi32(
                    0, 16, 8, 24, 4, 20, 12, 28, 2, 18, 10, 26, 6, 22, 14, 30,
                );
                let ihi = std::arch::x86_64::_mm512_setr_epi32(
                    1, 17, 9, 25, 5, 21, 13, 29, 3, 19, 11, 27, 7, 23, 15, 31,
                );
                (
                    std::arch::x86_64::_mm512_permutex2var_ps(ra, ilo, rb),
                    std::arch::x86_64::_mm512_permutex2var_ps(ia, ilo, ib),
                    std::arch::x86_64::_mm512_permutex2var_ps(ra, ihi, rb),
                    std::arch::x86_64::_mm512_permutex2var_ps(ia, ihi, ib),
                )
            }
        }};
    }
    macro_rules! qtr {
        ($ra:expr, $rb:expr, $rc:expr, $rd:expr, $lo:expr, $hi:expr) => {
            std::arch::x86_64::_mm512_mask_blend_ps(
                0b1010_1010_1010_1010,
                std::arch::x86_64::_mm512_permutex2var_ps($ra, $lo, $rb),
                std::arch::x86_64::_mm512_permutex2var_ps($rc, $hi, $rd),
            )
        };
    }
    let n = re.len();
    if n == 32 {
        let (mut r0, mut i0, mut r1, mut i1) = bitrev32!(re, im);
        fft32_ps!(plan, r0, i0, r1, i1);
        store2!(re, im, 0, r0, i0, r1, i1);
        return;
    }
    // SAFETY: both slices hold 64 lanes. Quarter indexes are the length-64
    // bit reversal; masked-off lanes are zero.
    let (mut r0, mut i0, mut r1, mut i1, mut h0, mut g0, mut h1, mut g1) = unsafe {
        let rp = re.as_ptr();
        let ip = im.as_ptr();
        let ra = std::arch::x86_64::_mm512_loadu_ps(rp);
        let rb = std::arch::x86_64::_mm512_loadu_ps(rp.add(16));
        let rc = std::arch::x86_64::_mm512_loadu_ps(rp.add(32));
        let rd = std::arch::x86_64::_mm512_loadu_ps(rp.add(48));
        let ia = std::arch::x86_64::_mm512_loadu_ps(ip);
        let ib = std::arch::x86_64::_mm512_loadu_ps(ip.add(16));
        let ic = std::arch::x86_64::_mm512_loadu_ps(ip.add(32));
        let id = std::arch::x86_64::_mm512_loadu_ps(ip.add(48));
        let l0 = std::arch::x86_64::_mm512_setr_epi32(
            0, 0, 16, 0, 8, 0, 24, 0, 4, 0, 20, 0, 12, 0, 28, 0,
        );
        let h0i = std::arch::x86_64::_mm512_setr_epi32(
            0, 0, 0, 16, 0, 8, 0, 24, 0, 4, 0, 20, 0, 12, 0, 28,
        );
        let l1 = std::arch::x86_64::_mm512_setr_epi32(
            2, 0, 18, 0, 10, 0, 26, 0, 6, 0, 22, 0, 14, 0, 30, 0,
        );
        let h1i = std::arch::x86_64::_mm512_setr_epi32(
            0, 2, 0, 18, 0, 10, 0, 26, 0, 6, 0, 22, 0, 14, 0, 30,
        );
        let l2 = std::arch::x86_64::_mm512_setr_epi32(
            1, 0, 17, 0, 9, 0, 25, 0, 5, 0, 21, 0, 13, 0, 29, 0,
        );
        let h2i = std::arch::x86_64::_mm512_setr_epi32(
            0, 1, 0, 17, 0, 9, 0, 25, 0, 5, 0, 21, 0, 13, 0, 29,
        );
        let l3 = std::arch::x86_64::_mm512_setr_epi32(
            3, 0, 19, 0, 11, 0, 27, 0, 7, 0, 23, 0, 15, 0, 31, 0,
        );
        let h3i = std::arch::x86_64::_mm512_setr_epi32(
            0, 3, 0, 19, 0, 11, 0, 27, 0, 7, 0, 23, 0, 15, 0, 31,
        );
        (
            qtr!(ra, rb, rc, rd, l0, h0i),
            qtr!(ia, ib, ic, id, l0, h0i),
            qtr!(ra, rb, rc, rd, l1, h1i),
            qtr!(ia, ib, ic, id, l1, h1i),
            qtr!(ra, rb, rc, rd, l2, h2i),
            qtr!(ia, ib, ic, id, l2, h2i),
            qtr!(ra, rb, rc, rd, l3, h3i),
            qtr!(ia, ib, ic, id, l3, h3i),
        )
    };
    fft32_ps!(plan, r0, i0, r1, i1);
    store2!(re, im, 0, r0, i0, r1, i1);
    fft32_ps!(plan, h0, g0, h1, g1);
    bfly_lo!(re, im, plan, 0, h0, g0, 31);
    bfly_lo!(re, im, plan, 16, h1, g1, 47);
}

fn fft64() -> &'static FftPlan {
    static P: OnceLock<FftPlan> = OnceLock::new();
    P.get_or_init(|| fft_plan(64))
}

struct Tw64 {
    c: [f64; 64],
    s: [f64; 64],
}

struct Tw32 {
    c: [f64; 32],
    s: [f64; 32],
}

/// `exp(−i π n / 64)` for the length-64 analysis DFT.
fn analysis_pre64() -> &'static Tw64 {
    static T: OnceLock<Tw64> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tw64 {
            c: [0.0; 64],
            s: [0.0; 64],
        };
        for n in 0..64 {
            let (s, c) = super::super::det_math::sincos_f64(std::f64::consts::PI * n as f64 / 64.0);
            t.c[n] = c;
            t.s[n] = s;
        }
        t
    })
}

/// `exp(−i π (k+½) / 128)`, `k = 0..32`.
fn analysis_post32() -> &'static Tw32 {
    static T: OnceLock<Tw32> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tw32 {
            c: [0.0; 32],
            s: [0.0; 32],
        };
        for k in 0..32 {
            let phi = std::f64::consts::PI * (k as f64 + 0.5) / 128.0;
            let (s, c) = super::super::det_math::sincos_f64(phi);
            t.c[k] = c;
            t.s[k] = s;
        }
        t
    })
}

#[cfg(target_arch = "x86_64")]
struct Tw32F {
    c: [f32; 32],
    s: [f32; 32],
}

#[cfg(target_arch = "x86_64")]
struct Tw64F {
    c: [f32; 64],
    s: [f32; 64],
}

/// f32 copy of [`analysis_pre64`].
#[cfg(target_arch = "x86_64")]
fn analysis_pre64_f32() -> &'static Tw64F {
    static T: OnceLock<Tw64F> = OnceLock::new();
    T.get_or_init(|| {
        let src = analysis_pre64();
        let mut t = Tw64F {
            c: [0.0; 64],
            s: [0.0; 64],
        };
        for n in 0..64 {
            t.c[n] = src.c[n] as f32;
            t.s[n] = src.s[n] as f32;
        }
        t
    })
}

/// f32 copy of [`analysis_post32`].
#[cfg(target_arch = "x86_64")]
fn analysis_post32_f32() -> &'static Tw32F {
    static T: OnceLock<Tw32F> = OnceLock::new();
    T.get_or_init(|| {
        let src = analysis_post32();
        let mut t = Tw32F {
            c: [0.0; 32],
            s: [0.0; 32],
        };
        for k in 0..32 {
            t.c[k] = src.c[k] as f32;
            t.s[k] = src.s[k] as f32;
        }
        t
    })
}

/// f32 copy of [`dct4_tw32`].
#[cfg(target_arch = "x86_64")]
fn dct4_tw32_f32() -> &'static Tw32F {
    static T: OnceLock<Tw32F> = OnceLock::new();
    T.get_or_init(|| {
        let src = dct4_tw32();
        let mut t = Tw32F {
            c: [0.0; 32],
            s: [0.0; 32],
        };
        for p in 0..32 {
            t.c[p] = src.c[p] as f32;
            t.s[p] = src.s[p] as f32;
        }
        t
    })
}

/// Figure 4.42 modulation: `W[k] = Σ_n u[n] · 2·exp(i·π/64·(k+½)·(2n−½))`.
///
/// One length-64 DFT. The pre-twiddle is `exp(−i π n / 64)` and the
/// post-twiddle is `exp(−i π (k+½) / 128)` on the conjugated bin.
#[cfg(test)]
pub(super) fn analysis_modulate(u: &[f64; 64]) -> [Complex; 32] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was just probed. Pre-twiddle, DFT, and
        // post-twiddle of this length run in f32.
        unsafe { return analysis_modulate_f32fft(u) }
    }
    analysis_modulate_scalar(u)
}

/// f64 analysis DFT. The 1e-20 GEMV check uses this, not the f32 path.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn analysis_modulate_f64(u: &[f64; 64]) -> [Complex; 32] {
    analysis_modulate_scalar(u)
}

fn analysis_modulate_scalar(u: &[f64; 64]) -> [Complex; 32] {
    let pre = analysis_pre64();
    let mut re = [0.0f64; 64];
    let mut im = [0.0f64; 64];
    for n in 0..64 {
        re[n] = u[n] * pre.c[n];
        im[n] = -u[n] * pre.s[n];
    }
    fft_in_place(&mut re, &mut im, fft64());
    let post = analysis_post32();
    let mut w = [Complex::default(); 32];
    for k in 0..32 {
        let cr = re[k];
        let ci = -im[k];
        w[k] = Complex::new(
            2.0 * (post.c[k] * cr + post.s[k] * ci),
            2.0 * (post.c[k] * ci - post.s[k] * cr),
        );
    }
    w
}

/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[cfg_attr(not(test), allow(dead_code))]
unsafe fn analysis_modulate_avx512(u: &[f64; 64]) -> [Complex; 32] {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_castsi512_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_set1_epi64,
        _mm512_set1_pd, _mm512_storeu_pd, _mm512_sub_pd,
    };
    let pre = analysis_pre64();
    let mut re = [0.0f64; 64];
    let mut im = [0.0f64; 64];
    let sign = _mm512_castsi512_pd(_mm512_set1_epi64(i64::MIN));
    for i in 0..8 {
        unsafe {
            let uv = _mm512_loadu_pd(u.as_ptr().add(i * 8));
            let c = _mm512_loadu_pd(pre.c.as_ptr().add(i * 8));
            let s = _mm512_loadu_pd(pre.s.as_ptr().add(i * 8));
            let nu = xor512_pd!(uv, sign);
            _mm512_storeu_pd(re.as_mut_ptr().add(i * 8), _mm512_mul_pd(uv, c));
            _mm512_storeu_pd(im.as_mut_ptr().add(i * 8), _mm512_mul_pd(nu, s));
        }
    }
    fft_in_place(&mut re, &mut im, fft64());
    let post = analysis_post32();
    let two = _mm512_set1_pd(2.0);
    let mut wr = [0.0f64; 32];
    let mut wi = [0.0f64; 32];
    for i in 0..4 {
        unsafe {
            let cr = _mm512_loadu_pd(re.as_ptr().add(i * 8));
            let ci = xor512_pd!(_mm512_loadu_pd(im.as_ptr().add(i * 8)), sign);
            let cp = _mm512_loadu_pd(post.c.as_ptr().add(i * 8));
            let sp = _mm512_loadu_pd(post.s.as_ptr().add(i * 8));
            let rr = _mm512_add_pd(_mm512_mul_pd(cp, cr), _mm512_mul_pd(sp, ci));
            let ii = _mm512_sub_pd(_mm512_mul_pd(cp, ci), _mm512_mul_pd(sp, cr));
            _mm512_storeu_pd(wr.as_mut_ptr().add(i * 8), _mm512_mul_pd(two, rr));
            _mm512_storeu_pd(wi.as_mut_ptr().add(i * 8), _mm512_mul_pd(two, ii));
        }
    }
    let mut w = [Complex::default(); 32];
    for k in 0..32 {
        w[k] = Complex::new(wr[k], wi[k]);
    }
    w
}

/// Figure 4.42 from an f32 folded window. Skips the f64→f32 narrowing.
pub(super) fn analysis_modulate_f32(u: &[f32; 64]) -> [Complex; 32] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was just probed.
        unsafe { return analysis_modulate_f32_in(u) }
    }
    let mut wide = [0.0f64; 64];
    for (d, &s) in wide.iter_mut().zip(u.iter()) {
        *d = f64::from(s);
    }
    analysis_modulate_scalar(&wide)
}

/// Figure 4.42 with f32 pre-twiddle, length-64 DFT, and post-twiddle.
///
/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(all(test, target_arch = "x86_64"))]
#[target_feature(enable = "avx512f,avx")]
unsafe fn analysis_modulate_f32fft(u: &[f64; 64]) -> [Complex; 32] {
    use std::arch::x86_64::{
        _mm256_loadu_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps, _mm256_xor_ps,
        _mm512_cvtpd_ps, _mm512_loadu_pd,
    };
    let pre = analysis_pre64_f32();
    let mut re = [0.0f32; 64];
    let mut im = [0.0f32; 64];
    let sign = _mm256_set1_ps(-0.0f32);
    for i in 0..8 {
        // SAFETY: eight f64 inputs and eight f32 twiddles per step.
        unsafe {
            let u32 = _mm512_cvtpd_ps(_mm512_loadu_pd(u.as_ptr().add(i * 8)));
            let c = _mm256_loadu_ps(pre.c.as_ptr().add(i * 8));
            let s = _mm256_loadu_ps(pre.s.as_ptr().add(i * 8));
            _mm256_storeu_ps(re.as_mut_ptr().add(i * 8), _mm256_mul_ps(u32, c));
            _mm256_storeu_ps(
                im.as_mut_ptr().add(i * 8),
                _mm256_mul_ps(_mm256_xor_ps(u32, sign), s),
            );
        }
    }
    // SAFETY: the pre-twiddled buffers have the plan length.
    unsafe { analysis_modulate_f32_body(&mut re, &mut im) }
}

/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f,avx")]
unsafe fn analysis_modulate_f32_in(u: &[f32; 64]) -> [Complex; 32] {
    use std::arch::x86_64::{
        _mm256_loadu_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps, _mm256_xor_ps,
    };
    let pre = analysis_pre64_f32();
    let mut re = [0.0f32; 64];
    let mut im = [0.0f32; 64];
    let sign = _mm256_set1_ps(-0.0f32);
    for i in 0..8 {
        unsafe {
            let u32 = _mm256_loadu_ps(u.as_ptr().add(i * 8));
            let c = _mm256_loadu_ps(pre.c.as_ptr().add(i * 8));
            let s = _mm256_loadu_ps(pre.s.as_ptr().add(i * 8));
            _mm256_storeu_ps(re.as_mut_ptr().add(i * 8), _mm256_mul_ps(u32, c));
            _mm256_storeu_ps(
                im.as_mut_ptr().add(i * 8),
                _mm256_mul_ps(_mm256_xor_ps(u32, sign), s),
            );
        }
    }
    unsafe { analysis_modulate_f32_body(&mut re, &mut im) }
}

/// DFT and post-twiddle shared by the f64-narrowed and f32 analysis paths.
///
/// # Safety
///
/// AVX-512F and AVX are available. `re` and `im` are length 64.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f,avx")]
unsafe fn analysis_modulate_f32_body(re: &mut [f32; 64], im: &mut [f32; 64]) -> [Complex; 32] {
    use std::arch::x86_64::{
        _mm256_add_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_sub_ps,
        _mm256_xor_ps, _mm512_cvtps_pd, _mm512_permutex2var_pd, _mm512_setr_epi64,
        _mm512_storeu_pd, _mm512_unpackhi_pd, _mm512_unpacklo_pd,
    };
    // SAFETY: both buffers have the plan length.
    unsafe { fft_dit_f32_avx512(re, im, fft64_f32()) };
    let post = analysis_post32_f32();
    let two = _mm256_set1_ps(2.0);
    let sign = _mm256_set1_ps(-0.0);
    let idx0 = _mm512_setr_epi64(0, 1, 8, 9, 2, 3, 10, 11);
    let idx1 = _mm512_setr_epi64(4, 5, 12, 13, 6, 7, 14, 15);
    let mut w = [Complex::default(); 32];
    for i in 0..4 {
        // SAFETY: eight f32 bins, then eight complexes of interleaved f64.
        unsafe {
            let cr = _mm256_loadu_ps(re.as_ptr().add(i * 8));
            let ci = _mm256_xor_ps(_mm256_loadu_ps(im.as_ptr().add(i * 8)), sign);
            let cp = _mm256_loadu_ps(post.c.as_ptr().add(i * 8));
            let sp = _mm256_loadu_ps(post.s.as_ptr().add(i * 8));
            let rr = _mm256_mul_ps(
                two,
                _mm256_add_ps(_mm256_mul_ps(cp, cr), _mm256_mul_ps(sp, ci)),
            );
            let ii = _mm256_mul_ps(
                two,
                _mm256_sub_ps(_mm256_mul_ps(cp, ci), _mm256_mul_ps(sp, cr)),
            );
            let rd = _mm512_cvtps_pd(rr);
            let id = _mm512_cvtps_pd(ii);
            let lo = _mm512_unpacklo_pd(rd, id);
            let hi = _mm512_unpackhi_pd(rd, id);
            let dst = w.as_mut_ptr().add(i * 8) as *mut f64;
            _mm512_storeu_pd(dst, _mm512_permutex2var_pd(lo, idx0, hi));
            _mm512_storeu_pd(dst.add(8), _mm512_permutex2var_pd(lo, idx1, hi));
        }
    }
    w
}

fn fft32() -> &'static FftPlan {
    static P: OnceLock<FftPlan> = OnceLock::new();
    P.get_or_init(|| fft_plan(32))
}

/// `cos(π(p+⅛)/64)`, `sin(π(p+⅛)/64)` for a length-64 DCT-IV.
fn dct4_tw32() -> &'static Tw32 {
    static T: OnceLock<Tw32> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tw32 {
            c: [0.0; 32],
            s: [0.0; 32],
        };
        for p in 0..32 {
            let ang = std::f64::consts::PI * (p as f64 + 0.125) / 64.0;
            let (s, c) = super::super::det_math::sincos_f64(ang);
            t.c[p] = c;
            t.s[p] = s;
        }
        t
    })
}

/// Unnormalized DCT-IV of length 64 via one forward DFT of length 32.
fn dct4_64(x: &[f64; 64], out: &mut [f64; 64]) {
    dct4_dispatch(x, out, false);
}

/// Unnormalized DST-IV: sign-flip the odd samples, DCT-IV, reverse.
fn dst4_64(x: &[f64; 64], out: &mut [f64; 64]) {
    dct4_dispatch(x, out, true);
}

fn dct4_dispatch(x: &[f64; 64], out: &mut [f64; 64], dst: bool) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was just probed. Pre-twiddle, DFT, and
        // post-twiddle of this length run in f32.
        unsafe { dct4_f32fft(x, out, dst) };
        return;
    }
    dct4_scalar(x, out, dst);
}

fn dct4_scalar(x: &[f64; 64], out: &mut [f64; 64], dst: bool) {
    let tw = dct4_tw32();
    let mut re = [0.0f64; 32];
    let mut im = [0.0f64; 32];
    for p in 0..32 {
        let r0 = x[2 * p];
        let odd = x[63 - 2 * p];
        let i0 = if dst { -odd } else { odd };
        re[p] = -i0 * tw.s[p] - r0 * tw.c[p];
        im[p] = -i0 * tw.c[p] + r0 * tw.s[p];
    }
    fft_in_place(&mut re, &mut im, fft32());
    for p in 0..32 {
        let r0 = re[p];
        let i0 = im[p];
        let a = -r0 * tw.c[p] - i0 * tw.s[p];
        let b = -r0 * tw.s[p] + i0 * tw.c[p];
        if dst {
            out[63 - 2 * p] = a;
            out[2 * p] = b;
        } else {
            out[2 * p] = a;
            out[63 - 2 * p] = b;
        }
    }
}

/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
#[cfg_attr(not(test), allow(dead_code))]
unsafe fn dct4_avx512(x: &[f64; 64], out: &mut [f64; 64], dst: bool) {
    use std::arch::x86_64::{
        _mm256_storeu_pd, _mm512_add_pd, _mm512_castpd512_pd256, _mm512_castsi512_pd,
        _mm512_loadu_pd, _mm512_maskz_compress_pd, _mm512_mul_pd, _mm512_set1_epi64,
        _mm512_storeu_pd, _mm512_sub_pd,
    };
    let tw = dct4_tw32();
    let mut even = [0.0f64; 32];
    let mut odd = [0.0f64; 32];
    let mut e_at = 0usize;
    let mut o_at = 0usize;
    for chunk in 0..8 {
        unsafe {
            let v = _mm512_loadu_pd(x.as_ptr().add(chunk * 8));
            let ev = _mm512_maskz_compress_pd(0b0101_0101, v);
            let od = _mm512_maskz_compress_pd(0b1010_1010, v);
            _mm256_storeu_pd(even.as_mut_ptr().add(e_at), _mm512_castpd512_pd256(ev));
            _mm256_storeu_pd(odd.as_mut_ptr().add(o_at), _mm512_castpd512_pd256(od));
        }
        e_at += 4;
        o_at += 4;
    }
    let mut odd_rev = [0.0f64; 32];
    // SAFETY: both buffers are 32 doubles.
    unsafe { reverse32(&odd, &mut odd_rev) };
    let sign = _mm512_castsi512_pd(_mm512_set1_epi64(i64::MIN));
    let mut re = [0.0f64; 32];
    let mut im = [0.0f64; 32];
    for i in 0..4 {
        unsafe {
            let r0 = _mm512_loadu_pd(even.as_ptr().add(i * 8));
            let mut t = _mm512_loadu_pd(odd_rev.as_ptr().add(i * 8));
            if !dst {
                t = xor512_pd!(t, sign);
            }
            let c = _mm512_loadu_pd(tw.c.as_ptr().add(i * 8));
            let s = _mm512_loadu_pd(tw.s.as_ptr().add(i * 8));
            _mm512_storeu_pd(
                re.as_mut_ptr().add(i * 8),
                _mm512_sub_pd(_mm512_mul_pd(t, s), _mm512_mul_pd(r0, c)),
            );
            _mm512_storeu_pd(
                im.as_mut_ptr().add(i * 8),
                _mm512_add_pd(_mm512_mul_pd(t, c), _mm512_mul_pd(r0, s)),
            );
        }
    }
    fft_in_place(&mut re, &mut im, fft32());
    let mut form_a = [0.0f64; 32];
    let mut form_b = [0.0f64; 32];
    for i in 0..4 {
        unsafe {
            let r0 = _mm512_loadu_pd(re.as_ptr().add(i * 8));
            let i0 = _mm512_loadu_pd(im.as_ptr().add(i * 8));
            let nr = xor512_pd!(r0, sign);
            let c = _mm512_loadu_pd(tw.c.as_ptr().add(i * 8));
            let s = _mm512_loadu_pd(tw.s.as_ptr().add(i * 8));
            _mm512_storeu_pd(
                form_a.as_mut_ptr().add(i * 8),
                _mm512_sub_pd(_mm512_mul_pd(nr, c), _mm512_mul_pd(i0, s)),
            );
            _mm512_storeu_pd(
                form_b.as_mut_ptr().add(i * 8),
                _mm512_add_pd(_mm512_mul_pd(nr, s), _mm512_mul_pd(i0, c)),
            );
        }
    }
    if dst {
        // SAFETY: the three buffers have the lengths the helper requires.
        unsafe { interleave_dct(&form_b, &form_a, out) };
    } else {
        // SAFETY: the three buffers have the lengths the helper requires.
        unsafe { interleave_dct(&form_a, &form_b, out) };
    }
}

/// Length-64 DCT-IV / DST-IV. Even/odd split, twiddles, DFT, and the
/// post-twiddle are f32; the interleave widens back to f64.
///
/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
unsafe fn dct4_f32fft(x: &[f64; 64], out: &mut [f64; 64], dst: bool) {
    use std::arch::x86_64::{
        _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_permutex2var_ps,
        _mm512_permutexvar_ps, _mm512_set1_ps, _mm512_setr_epi32, _mm512_storeu_ps, _mm512_sub_ps,
    };
    let tw = dct4_tw32_f32();
    let mut xf = [0.0f32; 64];
    // SAFETY: both slices have length 64.
    unsafe { cvt_f64_to_f32(x, &mut xf) };
    let e_idx = _mm512_setr_epi32(0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30);
    let o_idx = _mm512_setr_epi32(1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31);
    let rev = _mm512_setr_epi32(15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0);
    // SAFETY: `xf` holds 64 lanes. `odd_rev[p] = x[63 - 2p]` after the reverse.
    let (even0, t0, even1, t1) = unsafe {
        let p = xf.as_ptr();
        let x0 = _mm512_loadu_ps(p);
        let x1 = _mm512_loadu_ps(p.add(16));
        let x2 = _mm512_loadu_ps(p.add(32));
        let x3 = _mm512_loadu_ps(p.add(48));
        let sign = _mm512_set1_ps(-0.0f32);
        let odd_rev0 = _mm512_permutexvar_ps(rev, _mm512_permutex2var_ps(x2, o_idx, x3));
        let odd_rev1 = _mm512_permutexvar_ps(rev, _mm512_permutex2var_ps(x0, o_idx, x1));
        let t0 = if dst {
            odd_rev0
        } else {
            xor512_ps!(odd_rev0, sign)
        };
        let t1 = if dst {
            odd_rev1
        } else {
            xor512_ps!(odd_rev1, sign)
        };
        (
            _mm512_permutex2var_ps(x0, e_idx, x1),
            t0,
            _mm512_permutex2var_ps(x2, e_idx, x3),
            t1,
        )
    };
    let mut re = [0.0f32; 32];
    let mut im = [0.0f32; 32];
    for (i, (r0, t)) in [(even0, t0), (even1, t1)].into_iter().enumerate() {
        // SAFETY: sixteen twiddles at this offset.
        unsafe {
            let c = _mm512_loadu_ps(tw.c.as_ptr().add(i * 16));
            let s = _mm512_loadu_ps(tw.s.as_ptr().add(i * 16));
            _mm512_storeu_ps(
                re.as_mut_ptr().add(i * 16),
                _mm512_sub_ps(_mm512_mul_ps(t, s), _mm512_mul_ps(r0, c)),
            );
            _mm512_storeu_ps(
                im.as_mut_ptr().add(i * 16),
                _mm512_add_ps(_mm512_mul_ps(t, c), _mm512_mul_ps(r0, s)),
            );
        }
    }
    // SAFETY: both buffers have the length-32 plan.
    unsafe { fft_dit_f32_avx512(&mut re, &mut im, fft32_f32()) };
    let sign = _mm512_set1_ps(-0.0f32);
    let mut af = [0.0f32; 32];
    let mut bf = [0.0f32; 32];
    for i in 0..2 {
        // SAFETY: sixteen lanes of the DFT output and of the twiddle table.
        unsafe {
            let r0 = _mm512_loadu_ps(re.as_ptr().add(i * 16));
            let i0 = _mm512_loadu_ps(im.as_ptr().add(i * 16));
            let nr = xor512_ps!(r0, sign);
            let c = _mm512_loadu_ps(tw.c.as_ptr().add(i * 16));
            let s = _mm512_loadu_ps(tw.s.as_ptr().add(i * 16));
            _mm512_storeu_ps(
                af.as_mut_ptr().add(i * 16),
                _mm512_sub_ps(_mm512_mul_ps(nr, c), _mm512_mul_ps(i0, s)),
            );
            _mm512_storeu_ps(
                bf.as_mut_ptr().add(i * 16),
                _mm512_add_ps(_mm512_mul_ps(nr, s), _mm512_mul_ps(i0, c)),
            );
        }
    }
    let mut form_a = [0.0f64; 32];
    let mut form_b = [0.0f64; 32];
    // SAFETY: the f32 and f64 buffers share their lengths.
    unsafe {
        cvt_f32_to_f64(&af, &mut form_a);
        cvt_f32_to_f64(&bf, &mut form_b);
        if dst {
            interleave_dct(&form_b, &form_a, out);
        } else {
            interleave_dct(&form_a, &form_b, out);
        }
    }
}

/// Reverse 32 doubles. `dst[p] = src[31 - p]`.
///
/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn reverse32(src: &[f64; 32], dst: &mut [f64; 32]) {
    use std::arch::x86_64::{
        _mm512_loadu_pd, _mm512_permutexvar_pd, _mm512_setr_epi64, _mm512_storeu_pd,
    };
    let idx = _mm512_setr_epi64(7, 6, 5, 4, 3, 2, 1, 0);
    for i in 0..4 {
        unsafe {
            let v = _mm512_loadu_pd(src.as_ptr().add(24 - i * 8));
            _mm512_storeu_pd(dst.as_mut_ptr().add(i * 8), _mm512_permutexvar_pd(idx, v));
        }
    }
}

/// `even[p]` lands at `out[2p]` and `odd_rev[p]` at `out[63 - 2p]`.
///
/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
unsafe fn interleave_dct(even: &[f64; 32], odd_rev: &[f64; 32], out: &mut [f64; 64]) {
    use std::arch::x86_64::{
        _mm512_loadu_pd, _mm512_permutex2var_pd, _mm512_setr_epi64, _mm512_storeu_pd,
        _mm512_unpackhi_pd, _mm512_unpacklo_pd,
    };
    let mut odd = [0.0f64; 32];
    // SAFETY: both buffers are 32 doubles, and the permute indices select
    // lanes of the two unpacked registers.
    let (lo_idx, hi_idx) = unsafe {
        reverse32(odd_rev, &mut odd);
        (
            _mm512_setr_epi64(0, 1, 8, 9, 2, 3, 10, 11),
            _mm512_setr_epi64(4, 5, 12, 13, 6, 7, 14, 15),
        )
    };
    for i in 0..4 {
        unsafe {
            let e = _mm512_loadu_pd(even.as_ptr().add(i * 8));
            let o = _mm512_loadu_pd(odd.as_ptr().add(i * 8));
            let lo = _mm512_unpacklo_pd(e, o);
            let hi = _mm512_unpackhi_pd(e, o);
            _mm512_storeu_pd(
                out.as_mut_ptr().add(i * 16),
                _mm512_permutex2var_pd(lo, lo_idx, hi),
            );
            _mm512_storeu_pd(
                out.as_mut_ptr().add(i * 16 + 8),
                _mm512_permutex2var_pd(lo, hi_idx, hi),
            );
        }
    }
}

/// Figure 4.43 modulation: `v[n] = Σ_k Re(X[k]/64 · exp(i·π/128·(k+½)·(2n−255)))`.
///
/// `v` is a length-64 DCT-IV of `Re X` and a DST-IV of `Im X`. Each is one
/// length-32 DFT. Writes 128 samples; a shorter slice is left untouched.
pub(super) fn synthesis_modulate_into(bands: &[Complex], v: &mut [f64]) {
    if v.len() < 128 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was just probed, and `v` holds 128 lanes.
        unsafe { synthesis_modulate_avx512(bands, v) }
        return;
    }
    let got = synthesis_modulate_scalar(bands);
    v[..128].copy_from_slice(&got);
}

/// Figure 4.43 modulation into a fresh array. Tests use this; the synthesis
/// bank writes straight into its ring.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn synthesis_modulate(bands: &[Complex]) -> [f64; 128] {
    let mut v = [0.0f64; 128];
    synthesis_modulate_into(bands, &mut v);
    v
}

fn synthesis_modulate_scalar(bands: &[Complex]) -> [f64; 128] {
    let mut xr = [0.0f64; 64];
    let mut xi = [0.0f64; 64];
    for (k, x) in bands.iter().enumerate().take(64) {
        xr[k] = x.re;
        xi[k] = x.im;
    }
    let mut cr = [0.0f64; 64];
    let mut si = [0.0f64; 64];
    dct4_64(&xr, &mut cr);
    dst4_64(&xi, &mut si);
    let mut v = [0.0f64; 128];
    for n in 0..64 {
        v[n] = (-cr[n] + si[n]) * (1.0 / 64.0);
        v[64 + n] = (cr[63 - n] + si[63 - n]) * (1.0 / 64.0);
    }
    v
}

/// Both DCT-IVs and the `/64` combine. The combine matches the scalar
/// expression bit for bit: negate is a sign-bit xor, then add, then scale.
///
/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx")]
unsafe fn synthesis_modulate_avx512(bands: &[Complex], v: &mut [f64]) {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_castsi512_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_permutexvar_pd,
        _mm512_set1_epi64, _mm512_set1_pd, _mm512_setr_epi64, _mm512_storeu_pd,
    };
    let mut xr = [0.0f64; 64];
    let mut xi = [0.0f64; 64];
    for (k, x) in bands.iter().enumerate().take(64) {
        xr[k] = x.re;
        xi[k] = x.im;
    }
    let mut cr = [0.0f64; 64];
    let mut si = [0.0f64; 64];
    // SAFETY: the DCT-IV helper has the same feature set.
    unsafe {
        dct4_f32fft(&xr, &mut cr, false);
        dct4_f32fft(&xi, &mut si, true);
    }
    let scale = _mm512_set1_pd(1.0 / 64.0);
    let sign = _mm512_castsi512_pd(_mm512_set1_epi64(i64::MIN));
    let rev = _mm512_setr_epi64(7, 6, 5, 4, 3, 2, 1, 0);
    for i in 0..8 {
        // SAFETY: eight lanes inside each 64-length buffer, stored in `v`.
        unsafe {
            let crv = _mm512_loadu_pd(cr.as_ptr().add(i * 8));
            let siv = _mm512_loadu_pd(si.as_ptr().add(i * 8));
            let sum = _mm512_add_pd(xor512_pd!(crv, sign), siv);
            _mm512_storeu_pd(v.as_mut_ptr().add(i * 8), _mm512_mul_pd(sum, scale));
        }
    }
    for i in 0..8 {
        // `v[64 + n] = (cr[63 - n] + si[63 - n]) / 64`. Reverse each
        // octet in a register instead of copying both transforms.
        let src = 56 - i * 8;
        unsafe {
            let crv = _mm512_permutexvar_pd(rev, _mm512_loadu_pd(cr.as_ptr().add(src)));
            let siv = _mm512_permutexvar_pd(rev, _mm512_loadu_pd(si.as_ptr().add(src)));
            let sum = _mm512_add_pd(crv, siv);
            _mm512_storeu_pd(v.as_mut_ptr().add(64 + i * 8), _mm512_mul_pd(sum, scale));
        }
    }
}

/// Figure 4.43 modulation straight into an f32 history slot.
///
/// The DCT-IV stays in f32 (same butterflies as [`dct4_f32fft`]) and the
/// `/64` combine does not widen. A short `bands` or `v` is left untouched.
#[inline]
pub(super) fn synthesis_modulate_f32_into(bands: &[Complex], v: &mut [f32]) {
    if bands.len() < 64 || v.len() < 128 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was just probed, and both buffers hold a full slot.
        unsafe { synthesis_modulate_f32_avx512(bands, v) }
        return;
    }
    let wide = synthesis_modulate_scalar(bands);
    for (dst, src) in v[..128].iter_mut().zip(wide.iter()) {
        *dst = *src as f32;
    }
}

/// # Safety
///
/// AVX-512F and AVX are available. `bands` has 64 complexes and `v` has
/// 128 lanes.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f,avx")]
unsafe fn synthesis_modulate_f32_avx512(bands: &[Complex], v: &mut [f32]) {
    use std::arch::x86_64::{
        _mm256_storeu_ps, _mm512_add_ps, _mm512_cvtpd_ps, _mm512_loadu_pd, _mm512_loadu_ps,
        _mm512_mul_ps, _mm512_permutex2var_pd, _mm512_permutexvar_ps, _mm512_set1_ps,
        _mm512_setr_epi32, _mm512_setr_epi64, _mm512_storeu_ps,
    };
    let mut xr = [0.0f32; 64];
    let mut xi = [0.0f32; 64];
    // Complex is two f64s, so eight bands are sixteen doubles.
    let idx_re = _mm512_setr_epi64(0, 2, 4, 6, 8, 10, 12, 14);
    let idx_im = _mm512_setr_epi64(1, 3, 5, 7, 9, 11, 13, 15);
    let p = bands.as_ptr().cast::<f64>();
    for i in 0..8 {
        unsafe {
            let a = _mm512_loadu_pd(p.add(i * 16));
            let b = _mm512_loadu_pd(p.add(i * 16 + 8));
            _mm256_storeu_ps(
                xr.as_mut_ptr().add(i * 8),
                _mm512_cvtpd_ps(_mm512_permutex2var_pd(a, idx_re, b)),
            );
            _mm256_storeu_ps(
                xi.as_mut_ptr().add(i * 8),
                _mm512_cvtpd_ps(_mm512_permutex2var_pd(a, idx_im, b)),
            );
        }
    }
    let mut cr = [0.0f32; 64];
    let mut si = [0.0f32; 64];
    // SAFETY: both DCT-IV buffers are 64 lanes, and this function carries
    // the same target features.
    unsafe {
        dct4_f32_io(&xr, &mut cr, false);
        dct4_f32_io(&xi, &mut si, true);
    }
    let scale = _mm512_set1_ps(1.0 / 64.0);
    let sign = _mm512_set1_ps(-0.0);
    for i in 0..4 {
        unsafe {
            let crv = _mm512_loadu_ps(cr.as_ptr().add(i * 16));
            let siv = _mm512_loadu_ps(si.as_ptr().add(i * 16));
            let sum = _mm512_add_ps(xor512_ps!(crv, sign), siv);
            _mm512_storeu_ps(v.as_mut_ptr().add(i * 16), _mm512_mul_ps(sum, scale));
        }
    }
    let rev = _mm512_setr_epi32(15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0);
    for i in 0..4 {
        let src = 48 - i * 16;
        unsafe {
            let crv = _mm512_permutexvar_ps(rev, _mm512_loadu_ps(cr.as_ptr().add(src)));
            let siv = _mm512_permutexvar_ps(rev, _mm512_loadu_ps(si.as_ptr().add(src)));
            let sum = _mm512_add_ps(crv, siv);
            _mm512_storeu_ps(v.as_mut_ptr().add(64 + i * 16), _mm512_mul_ps(sum, scale));
        }
    }
}

/// Length-64 DCT-IV / DST-IV that starts and ends in f32.
///
/// # Safety
///
/// AVX-512F and AVX are available.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f,avx")]
unsafe fn dct4_f32_io(x: &[f32; 64], out: &mut [f32; 64], dst: bool) {
    use std::arch::x86_64::{
        _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_permutex2var_ps,
        _mm512_permutexvar_ps, _mm512_set1_ps, _mm512_setr_epi32, _mm512_storeu_ps, _mm512_sub_ps,
    };
    let tw = dct4_tw32_f32();
    let e_idx = _mm512_setr_epi32(0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30);
    let o_idx = _mm512_setr_epi32(1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31);
    let rev = _mm512_setr_epi32(15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0);
    let (even0, t0, even1, t1) = unsafe {
        let p = x.as_ptr();
        let x0 = _mm512_loadu_ps(p);
        let x1 = _mm512_loadu_ps(p.add(16));
        let x2 = _mm512_loadu_ps(p.add(32));
        let x3 = _mm512_loadu_ps(p.add(48));
        let sign = _mm512_set1_ps(-0.0f32);
        let odd_rev0 = _mm512_permutexvar_ps(rev, _mm512_permutex2var_ps(x2, o_idx, x3));
        let odd_rev1 = _mm512_permutexvar_ps(rev, _mm512_permutex2var_ps(x0, o_idx, x1));
        let t0 = if dst {
            odd_rev0
        } else {
            xor512_ps!(odd_rev0, sign)
        };
        let t1 = if dst {
            odd_rev1
        } else {
            xor512_ps!(odd_rev1, sign)
        };
        (
            _mm512_permutex2var_ps(x0, e_idx, x1),
            t0,
            _mm512_permutex2var_ps(x2, e_idx, x3),
            t1,
        )
    };
    let mut re = [0.0f32; 32];
    let mut im = [0.0f32; 32];
    for (i, (r0, t)) in [(even0, t0), (even1, t1)].into_iter().enumerate() {
        unsafe {
            let c = _mm512_loadu_ps(tw.c.as_ptr().add(i * 16));
            let s = _mm512_loadu_ps(tw.s.as_ptr().add(i * 16));
            _mm512_storeu_ps(
                re.as_mut_ptr().add(i * 16),
                _mm512_sub_ps(_mm512_mul_ps(t, s), _mm512_mul_ps(r0, c)),
            );
            _mm512_storeu_ps(
                im.as_mut_ptr().add(i * 16),
                _mm512_add_ps(_mm512_mul_ps(t, c), _mm512_mul_ps(r0, s)),
            );
        }
    }
    unsafe { fft_dit_f32_avx512(&mut re, &mut im, fft32_f32()) };
    let sign = _mm512_set1_ps(-0.0f32);
    let mut af = [0.0f32; 32];
    let mut bf = [0.0f32; 32];
    for i in 0..2 {
        unsafe {
            let r0 = _mm512_loadu_ps(re.as_ptr().add(i * 16));
            let i0 = _mm512_loadu_ps(im.as_ptr().add(i * 16));
            let nr = xor512_ps!(r0, sign);
            let c = _mm512_loadu_ps(tw.c.as_ptr().add(i * 16));
            let s = _mm512_loadu_ps(tw.s.as_ptr().add(i * 16));
            _mm512_storeu_ps(
                af.as_mut_ptr().add(i * 16),
                _mm512_sub_ps(_mm512_mul_ps(nr, c), _mm512_mul_ps(i0, s)),
            );
            _mm512_storeu_ps(
                bf.as_mut_ptr().add(i * 16),
                _mm512_add_ps(_mm512_mul_ps(nr, s), _mm512_mul_ps(i0, c)),
            );
        }
    }
    unsafe {
        if dst {
            interleave_dct_f32(&bf, &af, out);
        } else {
            interleave_dct_f32(&af, &bf, out);
        }
    }
}

/// `even[p]` lands at `out[2p]` and `odd_rev[31 - p]` at `out[2p + 1]`.
///
/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[inline]
#[target_feature(enable = "avx512f")]
unsafe fn interleave_dct_f32(even: &[f32; 32], odd_rev: &[f32; 32], out: &mut [f32; 64]) {
    use std::arch::x86_64::{
        _mm512_loadu_ps, _mm512_permutex2var_ps, _mm512_permutexvar_ps, _mm512_setr_epi32,
        _mm512_storeu_ps, _mm512_unpackhi_ps, _mm512_unpacklo_ps,
    };
    let mut odd = [0.0f32; 32];
    let rev = _mm512_setr_epi32(15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0);
    unsafe {
        let hi = _mm512_loadu_ps(odd_rev.as_ptr().add(16));
        let lo = _mm512_loadu_ps(odd_rev.as_ptr());
        _mm512_storeu_ps(odd.as_mut_ptr(), _mm512_permutexvar_ps(rev, hi));
        _mm512_storeu_ps(odd.as_mut_ptr().add(16), _mm512_permutexvar_ps(rev, lo));
    }
    // unpacklo/hi stay inside 128-bit lanes; the permute puts pairs in order.
    let idx_lo = _mm512_setr_epi32(0, 1, 2, 3, 16, 17, 18, 19, 4, 5, 6, 7, 20, 21, 22, 23);
    let idx_hi = _mm512_setr_epi32(8, 9, 10, 11, 24, 25, 26, 27, 12, 13, 14, 15, 28, 29, 30, 31);
    for i in 0..2 {
        unsafe {
            let e = _mm512_loadu_ps(even.as_ptr().add(i * 16));
            let o = _mm512_loadu_ps(odd.as_ptr().add(i * 16));
            let lo = _mm512_unpacklo_ps(e, o);
            let hi = _mm512_unpackhi_ps(e, o);
            _mm512_storeu_ps(
                out.as_mut_ptr().add(i * 32),
                _mm512_permutex2var_ps(lo, idx_lo, hi),
            );
            _mm512_storeu_ps(
                out.as_mut_ptr().add(i * 32 + 16),
                _mm512_permutex2var_ps(lo, idx_hi, hi),
            );
        }
    }
}

/// f64 synthesis DFT. The 1e-12 GEMV check uses this, not the f32 path.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn synthesis_modulate_f64(bands: &[Complex]) -> [f64; 128] {
    let mut xr = [0.0f64; 64];
    let mut xi = [0.0f64; 64];
    for (k, x) in bands.iter().enumerate().take(64) {
        xr[k] = x.re;
        xi[k] = x.im;
    }
    let mut cr = [0.0f64; 64];
    let mut si = [0.0f64; 64];
    dct4_scalar(&xr, &mut cr, false);
    dct4_scalar(&xi, &mut si, true);
    let mut v = [0.0f64; 128];
    for n in 0..64 {
        v[n] = (-cr[n] + si[n]) * (1.0 / 64.0);
        v[64 + n] = (cr[63 - n] + si[63 - n]) * (1.0 / 64.0);
    }
    v
}

#[cfg(test)]
pub(super) fn analysis_modulate_ref(u: &[f64; 64], m: &[Complex]) -> [Complex; 32] {
    let mut w = [Complex::default(); 32];
    for (k, wk) in w.iter_mut().enumerate() {
        let row = &m[k * 64..(k + 1) * 64];
        let mut acc = Complex::default();
        for (n, cell) in row.iter().enumerate() {
            acc += *cell * u[n];
        }
        *wk = acc;
    }
    w
}

#[cfg(test)]
pub(super) fn synthesis_modulate_ref(bands: &[Complex], n_mat: &[Complex]) -> [f64; 128] {
    let mut v = [0.0f64; 128];
    for (n, vn) in v.iter_mut().enumerate() {
        let row = &n_mat[n * 64..(n + 1) * 64];
        let mut acc = 0.0;
        for (k, cell) in row.iter().enumerate() {
            let x = bands[k];
            acc += x.re * cell.re - x.im * cell.im;
        }
        *vn = acc;
    }
    v
}

#[cfg(all(test, target_arch = "x86_64"))]
mod avx_bits {
    #[test]
    fn avx_fft_matches_scalar_bits() {
        if !std::is_x86_feature_detected!("avx") {
            return;
        }
        for n in [64usize, 128] {
            let plan = super::fft_plan(n);
            let mut re_s = vec![0.0; n];
            let mut im_s = vec![0.0; n];
            let mut state = 1u32;
            for i in 0..n {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                re_s[i] = f64::from(state) / f64::from(u32::MAX);
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                im_s[i] = f64::from(state) / f64::from(u32::MAX) - 0.5;
            }
            let mut re_a = re_s.clone();
            let mut im_a = im_s.clone();
            super::fft_stages_scalar(&mut re_s, &mut im_s, &plan);
            // SAFETY: AVX was probed; both buffers have the plan length.
            unsafe { super::fft_stages_avx(&mut re_a, &mut im_a, &plan) };
            for i in 0..n {
                assert_eq!(re_s[i].to_bits(), re_a[i].to_bits(), "re n={n} i={i}");
                assert_eq!(im_s[i].to_bits(), im_a[i].to_bits(), "im n={n} i={i}");
            }
        }
    }

    #[test]
    fn avx512_fft_matches_scalar_bits() {
        if !std::is_x86_feature_detected!("avx512f") {
            return;
        }
        for (n, sign) in [(64usize, -1.0), (128, -1.0), (128, 1.0)] {
            let plan = super::fft_plan_signed(n, sign);
            let mut re_s = vec![0.0; n];
            let mut im_s = vec![0.0; n];
            let mut state = 1u32;
            for i in 0..n {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                re_s[i] = f64::from(state) / f64::from(u32::MAX);
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                im_s[i] = f64::from(state) / f64::from(u32::MAX) - 0.5;
            }
            let mut re_a = re_s.clone();
            let mut im_a = im_s.clone();
            super::fft_stages_scalar(&mut re_s, &mut im_s, &plan);
            // SAFETY: AVX-512F was probed; both buffers have the plan length.
            unsafe { super::fft_stages_avx512(&mut re_a, &mut im_a, &plan) };
            for i in 0..n {
                assert_eq!(
                    re_s[i].to_bits(),
                    re_a[i].to_bits(),
                    "re n={n} sign={sign} i={i}"
                );
                assert_eq!(
                    im_s[i].to_bits(),
                    im_a[i].to_bits(),
                    "im n={n} sign={sign} i={i}"
                );
            }
        }
    }

    /// The in-register length-32 and length-64 DFTs match the scalar
    /// radix-2 transform bit for bit, including the forward plans used
    /// by the QMF.
    #[test]
    fn avx512_register_fft_matches_scalar_bits() {
        if !std::is_x86_feature_detected!("avx512f") {
            return;
        }
        for n in [32usize, 64] {
            for seed in [1u32, 0x9e37_79b9] {
                let plan = super::fft_plan(n);
                let mut re_s = vec![0.0; n];
                let mut im_s = vec![0.0; n];
                let mut state = seed;
                for i in 0..n {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    re_s[i] = f64::from(state) / f64::from(u32::MAX) - 0.5;
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    im_s[i] = f64::from(state as i32) / f64::from(i32::MAX);
                }
                let mut re_a = re_s.clone();
                let mut im_a = im_s.clone();
                for (i, &rev) in plan.bitrev.iter().enumerate() {
                    let j = rev as usize;
                    if i < j {
                        re_s.swap(i, j);
                        im_s.swap(i, j);
                    }
                }
                super::fft_stages_scalar(&mut re_s, &mut im_s, &plan);
                // SAFETY: AVX-512F was probed; both buffers have the plan length.
                unsafe { super::fft_dit_avx512(&mut re_a, &mut im_a, &plan) };
                for i in 0..n {
                    assert_eq!(re_s[i].to_bits(), re_a[i].to_bits(), "re n={n} i={i}");
                    assert_eq!(im_s[i].to_bits(), im_a[i].to_bits(), "im n={n} i={i}");
                }
            }
        }
    }

    /// DCT-IV, DST-IV, and the analysis pre/post match the scalar
    /// expressions bit for bit. The shared DFT is the register kernel
    /// on this host either way.
    #[test]
    fn avx512_qmf_twiddles_match_scalar_bits() {
        if !std::is_x86_feature_detected!("avx512f") {
            return;
        }
        let mut state = 0x1234_5678u32;
        let mut x = [0.0f64; 64];
        for slot in &mut x {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *slot = f64::from(state) / f64::from(u32::MAX) - 0.5;
        }
        for dst in [false, true] {
            let mut slow = [0.0f64; 64];
            let mut fast = [0.0f64; 64];
            super::dct4_scalar(&x, &mut slow, dst);
            // SAFETY: AVX-512F was probed.
            unsafe { super::dct4_avx512(&x, &mut fast, dst) };
            for (i, (&a, &b)) in slow.iter().zip(fast.iter()).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "dst={dst} i={i}");
            }
        }
        let slow = super::analysis_modulate_scalar(&x);
        // SAFETY: AVX-512F was probed.
        let fast = unsafe { super::analysis_modulate_avx512(&x) };
        for k in 0..32 {
            assert_eq!(slow[k].re.to_bits(), fast[k].re.to_bits(), "re {k}");
            assert_eq!(slow[k].im.to_bits(), fast[k].im.to_bits(), "im {k}");
        }
    }

    /// The f32 register DFT matches a scalar f32 DFT that uses the same
    /// rounded twiddles, including a signed-zero lane.
    #[test]
    fn avx512_f32_fft_matches_scalar_f32_bits() {
        if !std::is_x86_feature_detected!("avx512f") {
            return;
        }
        for n in [32usize, 64] {
            for seed in [1u32, 0x9e37_79b9] {
                let plan = super::fft_plan_f32(n);
                let mut re_s = vec![0.0f32; n];
                let mut im_s = vec![0.0f32; n];
                let mut state = seed;
                for i in 0..n {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    re_s[i] = (state as f32) / (u32::MAX as f32) - 0.5;
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    im_s[i] = (state as i32 as f32) / (i32::MAX as f32);
                }
                re_s[1] = -0.0;
                im_s[3] = -0.0;
                let mut re_a = re_s.clone();
                let mut im_a = im_s.clone();
                fft_scalar_f32(&mut re_s, &mut im_s, &plan);
                // SAFETY: AVX-512F was probed; both buffers have the plan length.
                unsafe { super::fft_dit_f32_avx512(&mut re_a, &mut im_a, &plan) };
                for i in 0..n {
                    assert_eq!(
                        re_s[i].to_bits(),
                        re_a[i].to_bits(),
                        "re n={n} i={i} scalar={} avx={}",
                        re_s[i],
                        re_a[i]
                    );
                    assert_eq!(
                        im_s[i].to_bits(),
                        im_a[i].to_bits(),
                        "im n={n} i={i} scalar={} avx={}",
                        im_s[i],
                        im_a[i]
                    );
                }
            }
        }
    }

    fn fft_scalar_f32(re: &mut [f32], im: &mut [f32], plan: &super::FftPlanF32) {
        let n = re.len();
        for (i, &rev) in plan.bitrev.iter().enumerate() {
            let j = usize::from(rev);
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2usize;
        let mut off = 0usize;
        while len <= n {
            let half = len / 2;
            for i in (0..n).step_by(len) {
                for k in 0..half {
                    let wr = plan.tw_re[off + k];
                    let wi = plan.tw_im[off + k];
                    let j = i + k + half;
                    let tr = wr * re[j] - wi * im[j];
                    let ti = wr * im[j] + wi * re[j];
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
}
