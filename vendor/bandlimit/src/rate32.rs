//! 32 kHz to 16 kHz on aarch64.
//!
//! Each output is one left-to-right `f64` accumulator. Tap `q` adds
//! `f64::from(sample) * f64::from(coeff)` before tap `q + 1`. Products are not
//! fused. Even taps read the odd input samples and odd taps read the even
//! samples, because the 339-tap window starts on an odd absolute index.
//! Consecutive outputs are adjacent in each of those rows, so one `f64x2`
//! holds two outputs.

use crate::Error;

/// First raw index whose window starts at a non-negative input.
const FULL: usize = 254;
/// `(339 - 1) / 2 * (2 + 1)`. Raw output `n` reads input `2 * n - CENTER`.
const CENTER_SUB: usize = 508;
const EVEN_TAPS: usize = 170;
const ODD_TAPS: usize = 169;
const BLOCK: usize = 16;
/// Eight `f64x2` lanes. The last lane is `2 * 7` samples after the first.
const LAST_LANE: usize = 14;
/// `vld1q` reads the last tap of the last lane and the `f64` after it.
const NEED: usize = (EVEN_TAPS - 1) + LAST_LANE + 2;
const PACK_PAIRS: usize = 2_048;

struct Pack {
    even_c: [f64; EVEN_TAPS],
    odd_c: [f64; ODD_TAPS],
    even_s: [f64; PACK_PAIRS],
    odd_s: [f64; PACK_PAIRS],
    pairs: usize,
    n0: usize,
    live: bool,
}

impl Pack {
    fn new() -> Self {
        Self {
            even_c: [0.0; EVEN_TAPS],
            odd_c: [0.0; ODD_TAPS],
            even_s: [0.0; PACK_PAIRS],
            odd_s: [0.0; PACK_PAIRS],
            pairs: 0,
            n0: 0,
            live: false,
        }
    }

    fn load_coeffs(&mut self, row: &[f32]) -> Result<(), Error> {
        if row.len() != EVEN_TAPS + ODD_TAPS {
            return Err(Error::Overflow);
        }
        for (q, coeff) in row.iter().copied().enumerate() {
            if q.is_multiple_of(2) {
                self.even_c[q / 2] = f64::from(coeff);
            } else {
                self.odd_c[q / 2] = f64::from(coeff);
            }
        }
        Ok(())
    }

    fn covers(&self, n: usize) -> bool {
        if !self.live {
            return false;
        }
        let Some(delta) = n.checked_sub(self.n0) else {
            return false;
        };
        match delta.checked_add(NEED) {
            Some(need) => self.pairs >= need,
            None => false,
        }
    }

    fn reload(&mut self, input: &[f32], n: usize, base: usize) -> Result<(), Error> {
        let Some(abs) = n
            .checked_mul(2)
            .and_then(|scaled| scaled.checked_sub(CENTER_SUB))
        else {
            return Err(Error::Overflow);
        };
        if abs < base {
            self.live = false;
            return Ok(());
        }
        let offset = abs - base;
        if offset > input.len() {
            self.live = false;
            return Ok(());
        }
        let take = (input.len() - offset).min(PACK_PAIRS * 2) & !1;
        let Some(window) = input.get(offset..offset + take) else {
            return Err(Error::Overflow);
        };
        self.pairs = pack_pairs(&mut self.even_s, &mut self.odd_s, window)?;
        self.n0 = n;
        self.live = true;
        Ok(())
    }
}

/// Largest `n` whose 339 taps all lie inside absolute length `base + input_len`.
fn last_full(base: usize, input_len: usize) -> Option<usize> {
    // `2 * n - 507 + 338 < base + input_len` => `n <= (abs_len + 168) / 2`.
    let abs_len = base.checked_add(input_len)?;
    Some(abs_len.checked_add(168)? / 2)
}

fn block_fits(n: usize, last: usize, raw_len: usize) -> bool {
    match (n.checked_add(BLOCK - 1), n.checked_add(BLOCK)) {
        (Some(tail), Some(end)) => tail <= last && end <= raw_len,
        (None, _) | (_, None) => false,
    }
}

/// Writes fitted 16-output blocks. The returned span is what this call wrote.
/// A span of `(first, first)` means every output stays on the scalar dot.
/// `input[0]` is absolute sample 0.
pub(super) fn widen(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(usize, usize), Error> {
    widen_at(input, row, raw_len, samples, first, 0)
}

/// Same kernel as [`widen`]. `base` is the absolute index of `input[0]`.
pub(super) fn widen_at(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
    base: usize,
) -> Result<(usize, usize), Error> {
    let Some(last) = last_full(base, input.len()) else {
        return Ok((first, first));
    };
    let start = first.max(FULL);
    if !block_fits(start, last, raw_len) {
        return Ok((first, first));
    }
    let mut pack = Pack::new();
    pack.load_coeffs(row)?;
    let mut n = start;
    while block_fits(n, last, raw_len) {
        if !pack.covers(n) {
            pack.reload(input, n, base)?;
            if !pack.covers(n) {
                break;
            }
        }
        // SAFETY: `covers` keeps every even and odd load inside the packed
        // rows. NEON is baseline. Each lane is one output and adds tap `q`
        // before tap `q + 1`, with `vmul` then `vadd`.
        let lanes = unsafe { sixteen(&pack, n) };
        store_block(samples, first, n, lanes)?;
        let Some(next) = n.checked_add(BLOCK) else {
            return Err(Error::Overflow);
        };
        n = next;
    }
    if n == start {
        return Ok((first, first));
    }
    Ok((start, n))
}

fn store_block(
    samples: &mut [f32],
    first: usize,
    n: usize,
    lanes: [f64; BLOCK],
) -> Result<(), Error> {
    let Some(at) = n.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let Some(end) = at.checked_add(BLOCK) else {
        return Err(Error::Overflow);
    };
    let Some(dest) = samples.get_mut(at..end) else {
        return Err(Error::Overflow);
    };
    for (slot, acc) in dest.iter_mut().zip(lanes) {
        *slot = super::super::finite_sample(acc)?;
    }
    Ok(())
}

fn pack_pairs(even: &mut [f64], odd: &mut [f64], input: &[f32]) -> Result<usize, Error> {
    if !input.len().is_multiple_of(2) {
        return Err(Error::Overflow);
    }
    let pairs = input.len() / 2;
    if pairs > even.len() || pairs > odd.len() {
        return Err(Error::Overflow);
    }
    let mut p = 0;
    while p + 4 <= pairs {
        // SAFETY: `p + 4 <= pairs` and `input` holds `pairs * 2` samples, so
        // eight `f32`s at `p * 2` are in range. Four slots in each row are in
        // range. NEON is baseline. `vcvt_f64_f32` matches `f64::from` for a
        // finite `f32`.
        unsafe {
            use std::arch::aarch64::{
                vcvt_f64_f32, vget_high_f32, vget_low_f32, vld2q_f32, vst1q_f64,
            };
            let split = vld2q_f32(input.as_ptr().add(p * 2));
            vst1q_f64(
                even.as_mut_ptr().add(p),
                vcvt_f64_f32(vget_low_f32(split.0)),
            );
            vst1q_f64(
                even.as_mut_ptr().add(p + 2),
                vcvt_f64_f32(vget_high_f32(split.0)),
            );
            vst1q_f64(odd.as_mut_ptr().add(p), vcvt_f64_f32(vget_low_f32(split.1)));
            vst1q_f64(
                odd.as_mut_ptr().add(p + 2),
                vcvt_f64_f32(vget_high_f32(split.1)),
            );
        }
        p += 4;
    }
    while p < pairs {
        even[p] = f64::from(input[p * 2]);
        odd[p] = f64::from(input[p * 2 + 1]);
        p += 1;
    }
    Ok(pairs)
}

/// Sixteen outputs from raw index `n`. Lane pair `r` is outputs `n + 2r` and
/// `n + 2r + 1`.
///
/// # Safety
///
/// [`Pack::covers`] is true for `n`. NEON is available.
#[inline(never)]
unsafe fn sixteen(pack: &Pack, n: usize) -> [f64; BLOCK] {
    use std::arch::aarch64::{
        vaddq_f64, vdupq_n_f64, vgetq_lane_f64, vld1q_dup_f64, vld1q_f64, vmulq_f64,
    };
    let delta = n - pack.n0;
    let even_c = pack.even_c.as_ptr();
    let odd_c = pack.odd_c.as_ptr();
    // SAFETY: `covers` kept `delta + NEED` inside both packed rows, and the
    // even row is read one sample later than the odd row.
    let (odd_s, even_s) = unsafe {
        (
            pack.odd_s.as_ptr().add(delta),
            pack.even_s.as_ptr().add(delta + 1),
        )
    };
    unsafe {
        let mut a0 = vdupq_n_f64(0.0);
        let mut a1 = vdupq_n_f64(0.0);
        let mut a2 = vdupq_n_f64(0.0);
        let mut a3 = vdupq_n_f64(0.0);
        let mut a4 = vdupq_n_f64(0.0);
        let mut a5 = vdupq_n_f64(0.0);
        let mut a6 = vdupq_n_f64(0.0);
        let mut a7 = vdupq_n_f64(0.0);
        macro_rules! mac_lanes {
            ($ptr:expr, $j:expr, $coeff:expr) => {
                a0 = vaddq_f64(a0, vmulq_f64(vld1q_f64($ptr.add($j)), $coeff));
                a1 = vaddq_f64(a1, vmulq_f64(vld1q_f64($ptr.add($j + 2)), $coeff));
                a2 = vaddq_f64(a2, vmulq_f64(vld1q_f64($ptr.add($j + 4)), $coeff));
                a3 = vaddq_f64(a3, vmulq_f64(vld1q_f64($ptr.add($j + 6)), $coeff));
                a4 = vaddq_f64(a4, vmulq_f64(vld1q_f64($ptr.add($j + 8)), $coeff));
                a5 = vaddq_f64(a5, vmulq_f64(vld1q_f64($ptr.add($j + 10)), $coeff));
                a6 = vaddq_f64(a6, vmulq_f64(vld1q_f64($ptr.add($j + 12)), $coeff));
                a7 = vaddq_f64(a7, vmulq_f64(vld1q_f64($ptr.add($j + 14)), $coeff));
            };
        }
        let mut j = 0_usize;
        while j < ODD_TAPS {
            let ce = vld1q_dup_f64(even_c.add(j));
            let co = vld1q_dup_f64(odd_c.add(j));
            mac_lanes!(odd_s, j, ce);
            mac_lanes!(even_s, j, co);
            j += 1;
        }
        let ce = vld1q_dup_f64(even_c.add(ODD_TAPS));
        mac_lanes!(odd_s, ODD_TAPS, ce);
        [
            vgetq_lane_f64(a0, 0),
            vgetq_lane_f64(a0, 1),
            vgetq_lane_f64(a1, 0),
            vgetq_lane_f64(a1, 1),
            vgetq_lane_f64(a2, 0),
            vgetq_lane_f64(a2, 1),
            vgetq_lane_f64(a3, 0),
            vgetq_lane_f64(a3, 1),
            vgetq_lane_f64(a4, 0),
            vgetq_lane_f64(a4, 1),
            vgetq_lane_f64(a5, 0),
            vgetq_lane_f64(a5, 1),
            vgetq_lane_f64(a6, 0),
            vgetq_lane_f64(a6, 1),
            vgetq_lane_f64(a7, 0),
            vgetq_lane_f64(a7, 1),
        ]
    }
}
