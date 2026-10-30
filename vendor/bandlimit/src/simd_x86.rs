//! x86_64 AVX dots checked against the scalar engine.
//!
//! Four `f32` values are widened and multiplied with `vmulpd`. The products
//! are added in lane order. Samples outside the input are skipped. That is
//! the scalar association, with no fused multiply-add.

use std::arch::x86_64::{_mm_loadu_ps, _mm256_cvtps_pd, _mm256_mul_pd, _mm256_storeu_pd};

pub(super) fn avx_available() -> bool {
    std::arch::is_x86_feature_detected!("avx")
}

/// # Safety
///
/// AVX must be enabled on this thread. [`avx_available`] is the check.
#[target_feature(enable = "avx")]
pub(super) unsafe fn dot(input: &[f32], mut origin: i64, coeffs: &[f32]) -> f64 {
    let mut acc = 0.0_f64;
    let mut coeff_pos = 0usize;
    while coeff_pos < coeffs.len() {
        if let Some(sample_index) = block_start(input.len(), origin, coeffs.len(), coeff_pos) {
            let lanes = unsafe { packed_products(input, sample_index, coeffs, coeff_pos) };
            acc += lanes[0];
            acc += lanes[1];
            acc += lanes[2];
            acc += lanes[3];
            coeff_pos = match coeff_pos.checked_add(4) {
                Some(next) => next,
                None => break,
            };
            origin = match origin.checked_add(4) {
                Some(next) => next,
                None => break,
            };
            continue;
        }
        if let Some(sample) = super::sample_at(input, origin)
            && let Some(coeff) = coeffs.get(coeff_pos)
        {
            acc += f64::from(sample) * f64::from(*coeff);
        }
        coeff_pos = match coeff_pos.checked_add(1) {
            Some(next) => next,
            None => break,
        };
        origin = match origin.checked_add(1) {
            Some(next) => next,
            None => break,
        };
    }
    acc
}

fn block_start(input_len: usize, origin: i64, coeff_len: usize, coeff_pos: usize) -> Option<usize> {
    if origin < 0 {
        return None;
    }
    let start = match usize::try_from(origin) {
        Ok(start) => start,
        Err(_) => return None,
    };
    match start.checked_add(4) {
        Some(end) if end <= input_len => {}
        Some(_) | None => return None,
    }
    match coeff_pos.checked_add(4) {
        Some(end) if end <= coeff_len => {}
        Some(_) | None => return None,
    }
    Some(start)
}

/// # Safety
///
/// `sample_index` and `coeff_index` each have four in-range elements.
/// AVX must be enabled on this thread.
#[target_feature(enable = "avx")]
unsafe fn packed_products(
    input: &[f32],
    sample_index: usize,
    coeffs: &[f32],
    coeff_index: usize,
) -> [f64; 4] {
    unsafe {
        // Four lanes, not one-past-the-end: `block_start` kept `index + 3` in range.
        let samples = input.as_ptr().add(sample_index);
        let coeff_ptr = coeffs.as_ptr().add(coeff_index);
        let prod = _mm256_mul_pd(
            _mm256_cvtps_pd(_mm_loadu_ps(samples)),
            _mm256_cvtps_pd(_mm_loadu_ps(coeff_ptr)),
        );
        let mut lanes = [0.0_f64; 4];
        _mm256_storeu_pd(lanes.as_mut_ptr(), prod);
        lanes
    }
}
