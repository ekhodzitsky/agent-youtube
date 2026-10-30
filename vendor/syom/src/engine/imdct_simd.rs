//! 4-wide FFT butterflies. Separate mul/add, never FMA, so aarch64 NEON
//! and x86 SSE2 match the scalar rounding (encoder bytes stay exact).

#[cfg(target_arch = "x86_64")]
#[inline]
pub(super) fn sse2_ok() -> bool {
    std::is_x86_feature_detected!("sse2")
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(super) fn avx_ok() -> bool {
    std::is_x86_feature_detected!("avx")
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn butterfly4(
    re: &mut [f32],
    im: &mut [f32],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f32],
    tw_im: &[f32],
) {
    use std::arch::aarch64::*;
    let j = i + k + half;
    unsafe {
        // SAFETY: NEON is baseline aarch64; caller ensures k+3 < half
        // and i+k+3, j+3 are in-range.
        let wr = vld1q_f32(tw_re.as_ptr().add(off + k));
        let wi = vld1q_f32(tw_im.as_ptr().add(off + k));
        let rj = vld1q_f32(re.as_ptr().add(j));
        let ij = vld1q_f32(im.as_ptr().add(j));
        let tr = vsubq_f32(vmulq_f32(wr, rj), vmulq_f32(wi, ij));
        let ti = vaddq_f32(vmulq_f32(wr, ij), vmulq_f32(wi, rj));
        let ur = vld1q_f32(re.as_ptr().add(i + k));
        let ui = vld1q_f32(im.as_ptr().add(i + k));
        vst1q_f32(re.as_mut_ptr().add(j), vsubq_f32(ur, tr));
        vst1q_f32(im.as_mut_ptr().add(j), vsubq_f32(ui, ti));
        vst1q_f32(re.as_mut_ptr().add(i + k), vaddq_f32(ur, tr));
        vst1q_f32(im.as_mut_ptr().add(i + k), vaddq_f32(ui, ti));
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn butterfly4(
    re: &mut [f32],
    im: &mut [f32],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f32],
    tw_im: &[f32],
) {
    use std::arch::x86_64::*;
    let j = i + k + half;
    unsafe {
        // SAFETY: caller probed SSE2 and ensures k+3 < half and the
        // four-lane loads at i+k and j are in-range. Mul/add/sub only
        // (no `_mm_fmadd_ps`) so lanes match scalar IEEE rounding.
        let wr = _mm_loadu_ps(tw_re.as_ptr().add(off + k));
        let wi = _mm_loadu_ps(tw_im.as_ptr().add(off + k));
        let rj = _mm_loadu_ps(re.as_ptr().add(j));
        let ij = _mm_loadu_ps(im.as_ptr().add(j));
        let tr = _mm_sub_ps(_mm_mul_ps(wr, rj), _mm_mul_ps(wi, ij));
        let ti = _mm_add_ps(_mm_mul_ps(wr, ij), _mm_mul_ps(wi, rj));
        let ur = _mm_loadu_ps(re.as_ptr().add(i + k));
        let ui = _mm_loadu_ps(im.as_ptr().add(i + k));
        _mm_storeu_ps(re.as_mut_ptr().add(j), _mm_sub_ps(ur, tr));
        _mm_storeu_ps(im.as_mut_ptr().add(j), _mm_sub_ps(ui, ti));
        _mm_storeu_ps(re.as_mut_ptr().add(i + k), _mm_add_ps(ur, tr));
        _mm_storeu_ps(im.as_mut_ptr().add(i + k), _mm_add_ps(ui, ti));
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn butterfly8(
    re: &mut [f32],
    im: &mut [f32],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f32],
    tw_im: &[f32],
) {
    use std::arch::x86_64::*;
    let j = i + k + half;
    unsafe {
        // SAFETY: caller probed AVX and ensures k+7 < half. Explicit
        // mul/add/sub, never `_mm256_fmadd_ps`.
        let wr = _mm256_loadu_ps(tw_re.as_ptr().add(off + k));
        let wi = _mm256_loadu_ps(tw_im.as_ptr().add(off + k));
        let rj = _mm256_loadu_ps(re.as_ptr().add(j));
        let ij = _mm256_loadu_ps(im.as_ptr().add(j));
        let tr = _mm256_sub_ps(_mm256_mul_ps(wr, rj), _mm256_mul_ps(wi, ij));
        let ti = _mm256_add_ps(_mm256_mul_ps(wr, ij), _mm256_mul_ps(wi, rj));
        let ur = _mm256_loadu_ps(re.as_ptr().add(i + k));
        let ui = _mm256_loadu_ps(im.as_ptr().add(i + k));
        _mm256_storeu_ps(re.as_mut_ptr().add(j), _mm256_sub_ps(ur, tr));
        _mm256_storeu_ps(im.as_mut_ptr().add(j), _mm256_sub_ps(ui, ti));
        _mm256_storeu_ps(re.as_mut_ptr().add(i + k), _mm256_add_ps(ur, tr));
        _mm256_storeu_ps(im.as_mut_ptr().add(i + k), _mm256_add_ps(ui, ti));
    }
}

/// In-place bit-reversal. Equal lengths take the unchecked path (the IMDCT
/// plan's table is a permutation of `0..n`). A mismatched table uses `swap`,
/// which panics on an out-of-range index the same way the old loop did.
pub(super) fn bitrev_swap(re: &mut [f32], im: &mut [f32], bitrev: &[u16]) {
    let n = re.len();
    if im.len() != n || bitrev.len() != n {
        for (i, &rev) in bitrev.iter().enumerate() {
            let j = rev as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        return;
    }
    let rp = re.as_mut_ptr();
    let ip = im.as_mut_ptr();
    for (i, &rev) in bitrev.iter().enumerate() {
        let j = rev as usize;
        if i < j && j < n {
            // SAFETY: i < j < n and both slices have length n.
            unsafe {
                std::ptr::swap(rp.add(i), rp.add(j));
                std::ptr::swap(ip.add(i), ip.add(j));
            }
        }
    }
}

/// Radix-2 stages with inlined AVX butterflies (same mul/add/sub as scalar).
///
/// # Safety
///
/// AVX is available. `re.len() == im.len()` is a power of two. `tw_re` and
/// `tw_im` each have at least `n - 1` elements from [`super::twiddle_table`].
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
pub(super) unsafe fn ifft_stages_avx(re: &mut [f32], im: &mut [f32], tw_re: &[f32], tw_im: &[f32]) {
    let n = re.len();
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        let mut i = 0usize;
        while i < n {
            let mut k = 0usize;
            while k + 8 <= half {
                // SAFETY: k+7 < half, so the 8-wide loads stay inside the stage.
                unsafe { butterfly8(re, im, i, k, half, off, tw_re, tw_im) };
                k += 8;
            }
            while k + 4 <= half {
                // SAFETY: k+3 < half. AVX includes SSE2.
                unsafe { butterfly4(re, im, i, k, half, off, tw_re, tw_im) };
                k += 4;
            }
            while k < half {
                // SAFETY: i+len <= n for power-of-two n, and off+k < n-1.
                unsafe {
                    let wr = *tw_re.get_unchecked(off + k);
                    let wi = *tw_im.get_unchecked(off + k);
                    let j = i + k + half;
                    let rj = *re.get_unchecked(j);
                    let ij = *im.get_unchecked(j);
                    let ur = *re.get_unchecked(i + k);
                    let ui = *im.get_unchecked(i + k);
                    let tr = wr * rj - wi * ij;
                    let ti = wr * ij + wi * rj;
                    *re.get_unchecked_mut(j) = ur - tr;
                    *im.get_unchecked_mut(j) = ui - ti;
                    *re.get_unchecked_mut(i + k) = ur + tr;
                    *im.get_unchecked_mut(i + k) = ui + ti;
                }
                k += 1;
            }
            i += len;
        }
        off += half;
        len *= 2;
    }
}

/// Pre-rotate, IFFT, post-rotate, and the IMDCT output map.
pub(super) fn apply_soa(
    plan: &super::Plan,
    spec: &[f32],
    out: &mut [f32],
    re: &mut [f32],
    im: &mut [f32],
) {
    #[cfg(target_arch = "x86_64")]
    {
        let n = plan.n;
        let n2 = n / 2;
        let n4 = n / 4;
        if spec.len() == n2
            && out.len() == n
            && re.len() == n4
            && im.len() == n4
            && plan.pre.len() == n4
            && plan.post.len() == n4
        {
            // SAFETY: lengths match Plan::apply_into (n is 256, 1024, or 2048).
            unsafe { prerot_sse(spec, &plan.pre, re, im) };
            super::ifft_soa(re, im, &plan.bitrev, &plan.tw_re, &plan.tw_im);
            unsafe {
                postrot_sse(&plan.post, re, im);
                permute_unchecked(out, re, im, n);
            }
            return;
        }
    }
    apply_soa_scalar(plan, spec, out, re, im);
}

fn apply_soa_scalar(
    plan: &super::Plan,
    spec: &[f32],
    out: &mut [f32],
    re: &mut [f32],
    im: &mut [f32],
) {
    let n = plan.n;
    let n2 = n / 2;
    let n4 = n / 4;
    for k in 0..n4 {
        let z = super::C::new(spec[n2 - 2 * k - 1], spec[2 * k]).mul(plan.pre[k]);
        re[k] = z.re;
        im[k] = z.im;
    }
    super::ifft_soa(re, im, &plan.bitrev, &plan.tw_re, &plan.tw_im);
    for k in 0..n4 {
        let z = super::C::new(re[k], im[k]).mul(plan.post[k]);
        re[k] = z.re;
        im[k] = z.im;
    }
    let scale = 2.0 / n as f32;
    let half = n4 / 2;
    for p in 0..half {
        let i = p * 2;
        out[i] = scale * im[half + p];
        out[n2 + i] = scale * re[half + p];
    }
    for p in 0..half {
        let i = p * 2 + 1;
        out[i] = -scale * re[half - 1 - p];
        out[n2 + i] = -scale * im[half - 1 - p];
    }
    for n_i in 0..n4 {
        out[n2 - 1 - n_i] = -out[n_i];
        out[n - 1 - n_i] = out[n2 + n_i];
    }
}

/// # Safety
///
/// `spec.len() == 2 * pre.len()`, and `re`/`im` are exactly `pre.len()` long.
/// SSE2 is the x86_64 baseline. Products use mul then add/sub, never FMA.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn prerot_sse(spec: &[f32], pre: &[super::C], re: &mut [f32], im: &mut [f32]) {
    use std::arch::x86_64::{
        _mm_add_ps, _mm_loadu_ps, _mm_mul_ps, _mm_setr_ps, _mm_shuffle_ps, _mm_storeu_ps,
        _mm_sub_ps,
    };
    let n4 = pre.len();
    let n2 = n4 * 2;
    let pf = pre.as_ptr().cast::<f32>();
    let mut k = 0usize;
    while k + 4 <= n4 {
        // SAFETY: k+3 < n4, so spec gathers and the 8 pre floats are in range.
        unsafe {
            let ar = _mm_setr_ps(
                *spec.get_unchecked(n2 - 2 * k - 1),
                *spec.get_unchecked(n2 - 2 * (k + 1) - 1),
                *spec.get_unchecked(n2 - 2 * (k + 2) - 1),
                *spec.get_unchecked(n2 - 2 * (k + 3) - 1),
            );
            let ai = _mm_setr_ps(
                *spec.get_unchecked(2 * k),
                *spec.get_unchecked(2 * (k + 1)),
                *spec.get_unchecked(2 * (k + 2)),
                *spec.get_unchecked(2 * (k + 3)),
            );
            let p0 = _mm_loadu_ps(pf.add(2 * k));
            let p1 = _mm_loadu_ps(pf.add(2 * k + 4));
            // Even lanes are real, odd lanes imaginary (AoS pairs).
            let br = _mm_shuffle_ps(p0, p1, 0x88);
            let bi = _mm_shuffle_ps(p0, p1, 0xDD);
            let zr = _mm_sub_ps(_mm_mul_ps(ar, br), _mm_mul_ps(ai, bi));
            let zi = _mm_add_ps(_mm_mul_ps(ar, bi), _mm_mul_ps(ai, br));
            _mm_storeu_ps(re.as_mut_ptr().add(k), zr);
            _mm_storeu_ps(im.as_mut_ptr().add(k), zi);
        }
        k += 4;
    }
    while k < n4 {
        let z = super::C::new(spec[n2 - 2 * k - 1], spec[2 * k]).mul(pre[k]);
        re[k] = z.re;
        im[k] = z.im;
        k += 1;
    }
}

/// # Safety
///
/// `post`, `re`, and `im` all have the same length, a multiple of 4 on the
/// fast path. SSE2, no FMA.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn postrot_sse(post: &[super::C], re: &mut [f32], im: &mut [f32]) {
    use std::arch::x86_64::{
        _mm_add_ps, _mm_loadu_ps, _mm_mul_ps, _mm_shuffle_ps, _mm_storeu_ps, _mm_sub_ps,
    };
    let n4 = post.len();
    let pf = post.as_ptr().cast::<f32>();
    let mut k = 0usize;
    while k + 4 <= n4 {
        // SAFETY: k+3 < n4; re/im have that many lanes.
        unsafe {
            let ar = _mm_loadu_ps(re.as_ptr().add(k));
            let ai = _mm_loadu_ps(im.as_ptr().add(k));
            let p0 = _mm_loadu_ps(pf.add(2 * k));
            let p1 = _mm_loadu_ps(pf.add(2 * k + 4));
            let br = _mm_shuffle_ps(p0, p1, 0x88);
            let bi = _mm_shuffle_ps(p0, p1, 0xDD);
            let zr = _mm_sub_ps(_mm_mul_ps(ar, br), _mm_mul_ps(ai, bi));
            let zi = _mm_add_ps(_mm_mul_ps(ar, bi), _mm_mul_ps(ai, br));
            _mm_storeu_ps(re.as_mut_ptr().add(k), zr);
            _mm_storeu_ps(im.as_mut_ptr().add(k), zi);
        }
        k += 4;
    }
    while k < n4 {
        let z = super::C::new(re[k], im[k]).mul(post[k]);
        re[k] = z.re;
        im[k] = z.im;
        k += 1;
    }
}

/// # Safety
///
/// `out.len() >= n`, `re.len()` and `im.len()` are at least `n/4`, `n >= 4`.
#[cfg(target_arch = "x86_64")]
unsafe fn permute_unchecked(out: &mut [f32], re: &[f32], im: &[f32], n: usize) {
    let n2 = n / 2;
    let n4 = n / 4;
    let half = n4 / 2;
    let scale = 2.0 / n as f32;
    let neg = -scale;
    let op = out.as_mut_ptr();
    let rp = re.as_ptr();
    let ip = im.as_ptr();
    for p in 0..half {
        let i = p * 2;
        // SAFETY: caller guarantees the indices derived below stay in range.
        unsafe {
            *op.add(i) = scale * *ip.add(half + p);
            *op.add(n2 + i) = scale * *rp.add(half + p);
        }
    }
    for p in 0..half {
        let i = p * 2 + 1;
        unsafe {
            *op.add(i) = neg * *rp.add(half - 1 - p);
            *op.add(n2 + i) = neg * *ip.add(half - 1 - p);
        }
    }
    for n_i in 0..n4 {
        unsafe {
            let a = *op.add(n_i);
            let b = *op.add(n2 + n_i);
            *op.add(n2 - 1 - n_i) = -a;
            *op.add(n - 1 - n_i) = b;
        }
    }
}
