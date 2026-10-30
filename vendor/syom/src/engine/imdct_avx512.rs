//! 16-wide FFT butterflies. Separate mul/add, never FMA, so each lane
//! matches the scalar rounding.

/// # Safety
///
/// AVX-512F and AVX are available. `k + 15 < half`, and `i + k + half + 15 < re.len()`.
/// `tw_re` and `tw_im` cover `off + k + 15`.
#[target_feature(enable = "avx512f,avx")]
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn butterfly16(
    re: &mut [f32],
    im: &mut [f32],
    i: usize,
    k: usize,
    half: usize,
    off: usize,
    tw_re: &[f32],
    tw_im: &[f32],
) {
    use std::arch::x86_64::{
        _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_storeu_ps, _mm512_sub_ps,
    };
    let j = i + k + half;
    unsafe {
        let wr = _mm512_loadu_ps(tw_re.as_ptr().add(off + k));
        let wi = _mm512_loadu_ps(tw_im.as_ptr().add(off + k));
        let rj = _mm512_loadu_ps(re.as_ptr().add(j));
        let ij = _mm512_loadu_ps(im.as_ptr().add(j));
        let tr = _mm512_sub_ps(_mm512_mul_ps(wr, rj), _mm512_mul_ps(wi, ij));
        let ti = _mm512_add_ps(_mm512_mul_ps(wr, ij), _mm512_mul_ps(wi, rj));
        let ur = _mm512_loadu_ps(re.as_ptr().add(i + k));
        let ui = _mm512_loadu_ps(im.as_ptr().add(i + k));
        _mm512_storeu_ps(re.as_mut_ptr().add(j), _mm512_sub_ps(ur, tr));
        _mm512_storeu_ps(im.as_mut_ptr().add(j), _mm512_sub_ps(ui, ti));
        _mm512_storeu_ps(re.as_mut_ptr().add(i + k), _mm512_add_ps(ur, tr));
        _mm512_storeu_ps(im.as_mut_ptr().add(i + k), _mm512_add_ps(ui, ti));
    }
}

/// # Safety
///
/// AVX is available. `k + 7 < half`.
#[target_feature(enable = "avx512f,avx")]
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
    use std::arch::x86_64::{
        _mm256_add_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_storeu_ps, _mm256_sub_ps,
    };
    let j = i + k + half;
    unsafe {
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

/// # Safety
///
/// SSE2 is available (implied by AVX). `k + 3 < half`.
#[target_feature(enable = "avx512f,avx")]
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
    use std::arch::x86_64::{_mm_add_ps, _mm_loadu_ps, _mm_mul_ps, _mm_storeu_ps, _mm_sub_ps};
    let j = i + k + half;
    unsafe {
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

/// Radix-2 stages. 16-wide, then 8-wide, then 4-wide, then one lane.
/// Same mul/add/sub order as the scalar butterfly.
///
/// # Safety
///
/// AVX-512F and AVX are available. `re.len() == im.len()` is a power of two.
/// `tw_re` and `tw_im` each have at least `n - 1` elements.
#[target_feature(enable = "avx512f,avx")]
pub(super) unsafe fn ifft_stages_avx512(
    re: &mut [f32],
    im: &mut [f32],
    tw_re: &[f32],
    tw_im: &[f32],
) {
    let n = re.len();
    let mut len = 2usize;
    let mut off = 0usize;
    while len <= n {
        let half = len / 2;
        let mut i = 0usize;
        while i < n {
            let mut k = 0usize;
            while k + 16 <= half {
                unsafe { butterfly16(re, im, i, k, half, off, tw_re, tw_im) };
                k += 16;
            }
            while k + 8 <= half {
                unsafe { butterfly8(re, im, i, k, half, off, tw_re, tw_im) };
                k += 8;
            }
            while k + 4 <= half {
                unsafe { butterfly4(re, im, i, k, half, off, tw_re, tw_im) };
                k += 4;
            }
            while k < half {
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
