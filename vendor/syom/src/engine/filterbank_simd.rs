//! Long-window overlap. Separate mul and add, never FMA, so each lane
//! matches the scalar rounding.

/// OnlyLong: `dst[i] = scratch[i] * left[i] + overlap[i]`,
/// `overlap[i] = scratch[1024 + i] * right_rev[i]`.
pub(crate) fn window_overlap(
    scratch: &[f32],
    overlap: &mut [f32],
    dst: &mut [f32],
    left: &[f32],
    right_rev: &[f32],
) {
    const N: usize = 1024;
    debug_assert!(scratch.len() >= 2 * N);
    debug_assert!(overlap.len() >= N && dst.len() >= N);
    debug_assert!(left.len() >= N && right_rev.len() >= N);
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F is available and each slice covers 1024 lanes
        // (scratch covers 2048). The loop steps by 16.
        unsafe { window_overlap_avx512(scratch, overlap, dst, left, right_rev) }
        return;
    }
    for i in 0..N {
        let prod = scratch[i] * left[i];
        dst[i] = prod + overlap[i];
        overlap[i] = scratch[N + i] * right_rev[i];
    }
}

/// # Safety
///
/// AVX-512F is available. `scratch` has 2048 lanes; `overlap`, `dst`,
/// `left`, and `right_rev` have 1024.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn window_overlap_avx512(
    scratch: &[f32],
    overlap: &mut [f32],
    dst: &mut [f32],
    left: &[f32],
    right_rev: &[f32],
) {
    use std::arch::x86_64::{_mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps, _mm512_storeu_ps};
    let mut i = 0usize;
    while i < 1024 {
        // SAFETY: `i + 15 < 1024` and `1024 + i + 15 < 2048`.
        unsafe {
            let prod = _mm512_mul_ps(
                _mm512_loadu_ps(scratch.as_ptr().add(i)),
                _mm512_loadu_ps(left.as_ptr().add(i)),
            );
            let acc = _mm512_add_ps(prod, _mm512_loadu_ps(overlap.as_ptr().add(i)));
            _mm512_storeu_ps(dst.as_mut_ptr().add(i), acc);
            let next = _mm512_mul_ps(
                _mm512_loadu_ps(scratch.as_ptr().add(1024 + i)),
                _mm512_loadu_ps(right_rev.as_ptr().add(i)),
            );
            _mm512_storeu_ps(overlap.as_mut_ptr().add(i), next);
        }
        i += 16;
    }
}

/// `dst[i] = left[i] + overlap[i]`, then `overlap[i] = right[i]`.
pub(crate) fn overlap_add(dst: &mut [f32], overlap: &mut [f32], left: &[f32], right: &[f32]) {
    let n = dst
        .len()
        .min(overlap.len())
        .min(left.len())
        .min(right.len());
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F is available. The vector loop stops 16 lanes
        // before `n`; the tail is scalar.
        unsafe { overlap_add_avx512(dst, overlap, left, right, n) }
        return;
    }
    for i in 0..n {
        dst[i] = left[i] + overlap[i];
        overlap[i] = right[i];
    }
}

/// # Safety
///
/// AVX-512F is available. The four slices each cover `n` lanes.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn overlap_add_avx512(
    dst: &mut [f32],
    overlap: &mut [f32],
    left: &[f32],
    right: &[f32],
    n: usize,
) {
    use std::arch::x86_64::{_mm512_add_ps, _mm512_loadu_ps, _mm512_storeu_ps};
    let mut i = 0usize;
    while i + 16 <= n {
        // SAFETY: 16 lanes at `i` are inside all four slices.
        unsafe {
            let sum = _mm512_add_ps(
                _mm512_loadu_ps(left.as_ptr().add(i)),
                _mm512_loadu_ps(overlap.as_ptr().add(i)),
            );
            _mm512_storeu_ps(dst.as_mut_ptr().add(i), sum);
            _mm512_storeu_ps(
                overlap.as_mut_ptr().add(i),
                _mm512_loadu_ps(right.as_ptr().add(i)),
            );
        }
        i += 16;
    }
    for j in i..n {
        dst[j] = left[j] + overlap[j];
        overlap[j] = right[j];
    }
}

#[cfg(test)]
mod tests {
    use super::{overlap_add, window_overlap};

    fn fill(n: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let bits = state ^ (state >> 9);
            out.push(f32::from_bits(bits));
        }
        out
    }

    #[test]
    fn window_overlap_matches_scalar_mul_then_add() {
        let mut scratch = fill(2048, 0x1234_5678);
        scratch[0] = 0.0;
        scratch[1] = -0.0;
        scratch[2] = 1.0e-20;
        scratch[1024] = -1.0e20;
        let left = fill(1024, 0x1111);
        let right = fill(1024, 0x2222);
        let mut rev = vec![0.0f32; 1024];
        for i in 0..1024 {
            rev[i] = right[1023 - i];
        }
        let overlap0 = fill(1024, 0x3333);
        let mut z = scratch.clone();
        let mut overlap_ref = overlap0.clone();
        let mut dst_ref = vec![0.0f32; 1024];
        for i in 0..1024 {
            z[i] *= left[i];
            z[1024 + i] *= right[1023 - i];
        }
        for i in 0..1024 {
            dst_ref[i] = z[i] + overlap_ref[i];
            overlap_ref[i] = z[1024 + i];
        }
        let mut overlap = overlap0;
        let mut dst = vec![0.0f32; 1024];
        window_overlap(&scratch, &mut overlap, &mut dst, &left, &rev);
        for i in 0..1024 {
            assert_eq!(dst[i].to_bits(), dst_ref[i].to_bits(), "dst {i}");
            assert_eq!(
                overlap[i].to_bits(),
                overlap_ref[i].to_bits(),
                "overlap {i}"
            );
        }
    }

    #[test]
    fn overlap_add_matches_scalar_bits() {
        let left = fill(1024, 7);
        let right = fill(1024, 8);
        let overlap0 = fill(1024, 9);
        let mut dst_ref = vec![0.0f32; 1024];
        let mut ov_ref = overlap0.clone();
        for i in 0..1024 {
            dst_ref[i] = left[i] + ov_ref[i];
            ov_ref[i] = right[i];
        }
        let mut dst = vec![0.0f32; 1024];
        let mut ov = overlap0;
        overlap_add(&mut dst, &mut ov, &left, &right);
        for i in 0..1024 {
            assert_eq!(dst[i].to_bits(), dst_ref[i].to_bits(), "dst {i}");
            assert_eq!(ov[i].to_bits(), ov_ref[i].to_bits(), "ov {i}");
        }
    }
}
