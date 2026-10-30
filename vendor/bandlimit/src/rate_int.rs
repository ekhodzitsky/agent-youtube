//! Integer decimation on aarch64.
//!
//! Each output is one left-to-right `f64` accumulator. Tap `q` adds
//! `f64::from(sample) * f64::from(coeff)` before tap `q + 1`. Products are not
//! fused. The window starts on one residue of the integer step, so tap `q`
//! always reads that same residue row. Consecutive outputs are adjacent in
//! the row, and one `f64x2` holds two of them. The 96 kHz and 192 kHz cascades
//! use this for the first stage.

use crate::Error;

const BLOCK: usize = 16;
/// Eight `f64x2` lanes. The last lane starts this many outputs after the first.
const LAST_LANE: usize = 14;
/// Samples kept in one residue row. A block needs fewer than this.
const PACK: usize = 1_024;

struct Pack<const STEP: usize, const TAPS: usize> {
    coeffs: [f64; TAPS],
    rows: [[f64; PACK]; STEP],
    count: usize,
    n0: usize,
    live: bool,
}

const fn center(step: usize, taps: usize) -> usize {
    (taps - 1) / 2 * (step + 1)
}

/// `(-center) mod step`. Every output of this ratio has the same residue.
const fn residue(step: usize, taps: usize) -> usize {
    let leftover = center(step, taps) % step;
    if leftover == 0 { 0 } else { step - leftover }
}

const fn need(step: usize, taps: usize) -> usize {
    let rem = residue(step, taps);
    (taps - 1 + rem) / step + LAST_LANE + 2
}

const fn full(step: usize, taps: usize) -> usize {
    (center(step, taps) + residue(step, taps)) / step
}

impl<const STEP: usize, const TAPS: usize> Pack<STEP, TAPS> {
    fn new() -> Self {
        Self {
            coeffs: [0.0; TAPS],
            rows: [[0.0; PACK]; STEP],
            count: 0,
            n0: 0,
            live: false,
        }
    }

    fn load_coeffs(&mut self, row: &[f32]) -> Result<(), Error> {
        if row.len() != TAPS {
            return Err(Error::Overflow);
        }
        for (slot, coeff) in self.coeffs.iter_mut().zip(row) {
            *slot = f64::from(*coeff);
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
        match delta.checked_add(need(STEP, TAPS)) {
            Some(need) => self.count >= need,
            None => false,
        }
    }

    fn reload(&mut self, input: &[f32], n: usize) -> Result<(), Error> {
        let Some(abs) = n
            .checked_mul(STEP)
            .and_then(|scaled| scaled.checked_sub(center(STEP, TAPS) + residue(STEP, TAPS)))
        else {
            return Err(Error::Overflow);
        };
        if abs > input.len() {
            self.live = false;
            return Ok(());
        }
        let take = (input.len() - abs).min(PACK * STEP) / STEP * STEP;
        let Some(window) = input.get(abs..abs + take) else {
            return Err(Error::Overflow);
        };
        self.count = pack_rows(&mut self.rows, window)?;
        self.n0 = n;
        self.live = true;
        Ok(())
    }
}

fn last_full<const STEP: usize, const TAPS: usize>(input_len: usize) -> Option<usize> {
    // `step * n - center + taps - 1 < input_len`.
    let room = input_len
        .checked_add(center(STEP, TAPS))?
        .checked_sub(TAPS)?;
    Some(room / STEP)
}

fn block_fits(n: usize, last: usize, raw_len: usize) -> bool {
    match (n.checked_add(BLOCK - 1), n.checked_add(BLOCK)) {
        (Some(tail), Some(end)) => tail <= last && end <= raw_len,
        (None, _) | (_, None) => false,
    }
}

fn widen<const STEP: usize, const TAPS: usize>(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(usize, usize), Error> {
    let Some(last) = last_full::<STEP, TAPS>(input.len()) else {
        return Ok((first, first));
    };
    let start = first.max(full(STEP, TAPS));
    if !block_fits(start, last, raw_len) {
        return Ok((first, first));
    }
    let mut pack = Pack::<STEP, TAPS>::new();
    pack.load_coeffs(row)?;
    let mut n = start;
    while block_fits(n, last, raw_len) {
        if !pack.covers(n) {
            pack.reload(input, n)?;
            if !pack.covers(n) {
                break;
            }
        }
        // SAFETY: `covers` keeps every residue load inside its packed row.
        // NEON is baseline. Each lane is one output and adds tap `q` before
        // tap `q + 1`, with `vmul` then `vadd`.
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

fn pack_rows<const STEP: usize>(
    rows: &mut [[f64; PACK]; STEP],
    input: &[f32],
) -> Result<usize, Error> {
    let (chunks, rest) = input.as_chunks::<STEP>();
    if !rest.is_empty() || chunks.len() > PACK {
        return Err(Error::Overflow);
    }
    for (k, chunk) in chunks.iter().enumerate() {
        for (resid, sample) in chunk.iter().enumerate() {
            rows[resid][k] = f64::from(*sample);
        }
    }
    Ok(chunks.len())
}

/// Sixteen outputs from raw index `n`. Lane pair `r` is outputs `n + 2r` and
/// `n + 2r + 1`.
///
/// # Safety
///
/// [`Pack::covers`] is true for `n`. NEON is available.
#[inline(never)]
unsafe fn sixteen<const STEP: usize, const TAPS: usize>(
    pack: &Pack<STEP, TAPS>,
    n: usize,
) -> [f64; BLOCK] {
    use std::arch::aarch64::{
        vaddq_f64, vdupq_n_f64, vgetq_lane_f64, vld1q_dup_f64, vld1q_f64, vmulq_f64,
    };
    let delta = n - pack.n0;
    let rem = residue(STEP, TAPS);
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
            ($ptr:expr, $coeff:expr) => {
                a0 = vaddq_f64(a0, vmulq_f64(vld1q_f64($ptr), $coeff));
                a1 = vaddq_f64(a1, vmulq_f64(vld1q_f64($ptr.add(2)), $coeff));
                a2 = vaddq_f64(a2, vmulq_f64(vld1q_f64($ptr.add(4)), $coeff));
                a3 = vaddq_f64(a3, vmulq_f64(vld1q_f64($ptr.add(6)), $coeff));
                a4 = vaddq_f64(a4, vmulq_f64(vld1q_f64($ptr.add(8)), $coeff));
                a5 = vaddq_f64(a5, vmulq_f64(vld1q_f64($ptr.add(10)), $coeff));
                a6 = vaddq_f64(a6, vmulq_f64(vld1q_f64($ptr.add(12)), $coeff));
                a7 = vaddq_f64(a7, vmulq_f64(vld1q_f64($ptr.add(14)), $coeff));
            };
        }
        // Tap `q` reads residue `(q + rem) % STEP` at index `(q + rem) / STEP`.
        // Walking the residue avoids a division on every tap. `covers` keeps
        // `k + delta + LAST_LANE + 1` inside the row.
        let mut q = 0_usize;
        let mut k = 0_usize;
        let mut resid = rem;
        while q < TAPS {
            let base = k + delta;
            let end = resid + (TAPS - q);
            let end = if end < STEP { end } else { STEP };
            while resid < end {
                let coeff = vld1q_dup_f64(pack.coeffs.as_ptr().add(q));
                let ptr = pack.rows.get_unchecked(resid).as_ptr().add(base);
                mac_lanes!(ptr, coeff);
                q += 1;
                resid += 1;
            }
            if resid == STEP {
                resid = 0;
                k += 1;
            }
        }
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

/// Writes fitted 16-output blocks of one integer ratio. `input[0]` is sample 0.
pub(super) fn widen_span<const STEP: usize, const TAPS: usize>(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(usize, usize), Error> {
    widen::<STEP, TAPS>(input, row, raw_len, samples, first)
}
