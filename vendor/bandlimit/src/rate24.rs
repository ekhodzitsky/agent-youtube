//! 24 kHz to 16 kHz. The reduced ratio is 3/2.
//!
//! Even outputs read phase 0 and odd outputs read phase 128. The placement
//! fraction is 0, so each output is one left-to-right f64 dot of the 373-tap
//! row. Eight outputs share two calls to [`super::four_outputs`]. On aarch64
//! the same eight dots run in NEON lanes, multiplying then adding. A window
//! that leaves the input uses [`super::sample`].

use super::{Engine, Error, PHASE_COUNT};
use crate::coefficients::{TAPS, Table};

const WINDOW: usize = 383;

/// Origin of raw output 0. Output `2m` starts `3m` samples later.
const ORIGIN_0: i64 = -465;

pub(super) fn fill(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
) -> Result<Vec<f32>, Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let mut samples = Vec::new();
    if samples.try_reserve_exact(span).is_err() {
        return Err(Error::Overflow);
    }
    samples.resize(span, 0.0);
    fill_into(input, coeffs, raw_len, engine, first, &mut samples)?;
    Ok(samples)
}

pub(super) fn fill_into(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
    output: &mut [f32],
) -> Result<(), Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    if output.len() != span {
        return Err(Error::Overflow);
    }
    let row_even = coeffs.row(0)?;
    let row_odd = coeffs.row(PHASE_COUNT / 2)?;
    let rows =
        row_even.len() == TAPS as usize && row_odd.len() == TAPS as usize && coeffs.taps == TAPS;
    #[cfg(target_arch = "aarch64")]
    let packed = if rows {
        pack_24(input, row_even, row_odd).ok()
    } else {
        None
    };
    #[cfg(target_arch = "aarch64")]
    let mut wide = Wide24::new();
    let mut n = first;
    while n < raw_len {
        if rows && n.is_multiple_of(2) && raw_len - n >= 8 {
            #[cfg(target_arch = "aarch64")]
            if let Some(packed) = packed.as_ref()
                && let Some(block) = neon_wide(packed, n, &mut wide)?
            {
                let at = n - first;
                output[at..at + 8].copy_from_slice(&block);
                n += 8;
                continue;
            }
            #[cfg(target_arch = "aarch64")]
            if let Some(packed) = packed.as_ref()
                && let Some(block) = neon_block(packed, n)?
            {
                let at = n - first;
                output[at..at + 8].copy_from_slice(&block);
                n += 8;
                continue;
            }
            if let Some(block) = positioned(input, row_even, row_odd, n)? {
                let at = n - first;
                output[at..at + 8].copy_from_slice(&block);
                n += 8;
                continue;
            }
        }
        let index = match u64::try_from(n) {
            Ok(index) => index,
            Err(_) => return Err(Error::Overflow),
        };
        output[n - first] = super::sample(input, coeffs, index, 3, 2, engine)?;
        n += 1;
    }
    Ok(())
}

fn even_origin(n: u64) -> Option<i64> {
    let pair = i64::try_from(n / 2).ok()?;
    pair.checked_mul(3)?.checked_add(ORIGIN_0)
}

fn positioned(
    input: &[f32],
    row_even: &[f32],
    row_odd: &[f32],
    n: usize,
) -> Result<Option<[f32; 8]>, Error> {
    let n = match u64::try_from(n) {
        Ok(n) => n,
        Err(_) => return Err(Error::Overflow),
    };
    let Some(origin) = even_origin(n) else {
        return Ok(None);
    };
    if origin < 0 {
        return Ok(None);
    }
    let start = match usize::try_from(origin) {
        Ok(start) => start,
        Err(_) => return Ok(None),
    };
    let Some(end) = start.checked_add(WINDOW) else {
        return Ok(None);
    };
    let Some(window) = input.get(start..end) else {
        return Ok(None);
    };
    block8(window, row_even, row_odd)
}

/// `window` begins at the even output. Phase 0 uses step 3. Phase 128 starts
/// one sample later and uses the same step.
fn block8(window: &[f32], row_even: &[f32], row_odd: &[f32]) -> Result<Option<[f32; 8]>, Error> {
    let Some(odd) = window.get(1..) else {
        return Ok(None);
    };
    let evens = super::four_outputs(window, row_even, 3)?;
    let odds = super::four_outputs(odd, row_odd, 3)?;
    Ok(Some([
        evens[0], odds[0], evens[1], odds[1], evens[2], odds[2], evens[3], odds[3],
    ]))
}

/// Even lanes share `even`. Odd lanes share `odd`. Samples stay in `input`
/// and are widened from `f32` at each multiply.
#[cfg(target_arch = "aarch64")]
struct Packed24<'a> {
    input: &'a [f32],
    even: &'a [f64],
    odd: &'a [f64],
}

#[cfg(target_arch = "aarch64")]
fn widen_row(row: &[f32]) -> Result<Vec<f64>, Error> {
    let mut out = Vec::new();
    if out.try_reserve_exact(row.len()).is_err() {
        return Err(Error::Overflow);
    }
    for coeff in row {
        out.push(f64::from(*coeff));
    }
    Ok(out)
}

/// Widens the two phase rows once. Later calls borrow that copy.
#[cfg(target_arch = "aarch64")]
fn cached_row(which: usize, row: &[f32]) -> Result<&'static [f64], Error> {
    static EVEN: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    static ODD: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    let slot = if which == 0 { &EVEN } else { &ODD };
    if let Some(wide) = slot.get() {
        return Ok(wide.as_slice());
    }
    let wide = widen_row(row)?;
    Ok(slot.get_or_init(|| wide).as_slice())
}

/// Widens the two phase rows. The input is borrowed, not copied.
#[cfg(target_arch = "aarch64")]
fn pack_24<'a>(input: &'a [f32], row_even: &[f32], row_odd: &[f32]) -> Result<Packed24<'a>, Error> {
    Ok(Packed24 {
        input,
        even: cached_row(0, row_even)?,
        odd: cached_row(1, row_odd)?,
    })
}

/// Furthest `f32` is `start + 7 + (TAPS - 1) + 3`, the high lane of `po1`.
#[cfg(target_arch = "aarch64")]
fn group_fits(packed: &Packed24<'_>, start: usize) -> bool {
    if packed.even.len() != TAPS as usize || packed.odd.len() != TAPS as usize {
        return false;
    }
    let Some(last) = start
        .checked_add(7)
        .and_then(|at| at.checked_add(TAPS as usize - 1))
        .and_then(|at| at.checked_add(3))
    else {
        return false;
    };
    last < packed.input.len()
}

#[cfg(target_arch = "aarch64")]
fn neon_block(packed: &Packed24<'_>, n: usize) -> Result<Option<[f32; 8]>, Error> {
    let Ok(n) = u64::try_from(n) else {
        return Err(Error::Overflow);
    };
    let Some(origin) = even_origin(n) else {
        return Ok(None);
    };
    if origin < 0 {
        return Ok(None);
    }
    let Ok(start) = usize::try_from(origin) else {
        return Ok(None);
    };
    if !group_fits(packed, start) {
        return Ok(None);
    }
    // SAFETY: `group_fits` kept every f32 index inside `packed.input`.
    // NEON is baseline on aarch64. Products are `vmul` then `vadd`.
    let lanes = unsafe { eight_dots(packed, start) };
    let mut out = [0.0_f32; 8];
    for (lane, acc) in lanes.into_iter().enumerate() {
        out[lane] = super::finite_sample(acc)?;
    }
    Ok(Some(out))
}

/// One steal chunk spans fewer than this many input samples past its first
/// fitted origin. The buffer is stack storage. It is not allocated.
#[cfg(target_arch = "aarch64")]
const WIDE_CAP: usize = 8_192;

#[cfg(target_arch = "aarch64")]
struct Wide24 {
    samples: [std::mem::MaybeUninit<f64>; WIDE_CAP],
    base: usize,
    len: usize,
}

#[cfg(target_arch = "aarch64")]
impl Wide24 {
    fn new() -> Self {
        Self {
            // SAFETY: `MaybeUninit<f64>` may be uninitialized. `cover` writes every
            // element the dots read, and that prefix is the only one passed down.
            samples: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            base: 0,
            len: 0,
        }
    }

    fn cover(&mut self, input: &[f32], start: usize) -> bool {
        if start >= self.base && start + WINDOW <= self.base + self.len {
            return true;
        }
        let available = match input.len().checked_sub(start) {
            Some(available) => available,
            None => return false,
        };
        let take = available.min(WIDE_CAP);
        if take < WINDOW {
            return false;
        }
        for (slot, sample) in self.samples[..take].iter_mut().zip(input[start..].iter()) {
            slot.write(f64::from(*sample));
        }
        self.base = start;
        self.len = take;
        true
    }
}

/// Eight dots from a window widened once. Lane 0 is the even output and lane
/// 1 is the odd output. Tap `k` multiplies the already-widened pair at
/// `rel + k`, then adds, in the same order as [`eight_dots`].
#[cfg(target_arch = "aarch64")]
fn neon_wide(
    packed: &Packed24<'_>,
    n: usize,
    wide: &mut Wide24,
) -> Result<Option<[f32; 8]>, Error> {
    let Ok(n) = u64::try_from(n) else {
        return Err(Error::Overflow);
    };
    let Some(origin) = even_origin(n) else {
        return Ok(None);
    };
    if origin < 0 {
        return Ok(None);
    }
    let Ok(start) = usize::try_from(origin) else {
        return Ok(None);
    };
    if !group_fits(packed, start) || !wide.cover(packed.input, start) {
        return Ok(None);
    }
    let Some(rel) = start.checked_sub(wide.base) else {
        return Ok(None);
    };
    if rel + WINDOW > wide.len {
        return Ok(None);
    }
    #[cfg(test)]
    WIDE_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: `cover` wrote `f64::from` of each f32 from `start` through
    // `start + WINDOW`. `MaybeUninit<f64>` has the same layout as `f64`.
    // `group_fits` keeps every pair inside that window. NEON is baseline
    // on aarch64. Products are `vmul` then `vadd`.
    let lanes = unsafe { eight_dots_wide(packed, wide.samples.as_ptr().cast(), rel) };
    let mut out = [0.0_f32; 8];
    for (lane, acc) in lanes.into_iter().enumerate() {
        out[lane] = super::finite_sample(acc)?;
    }
    Ok(Some(out))
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn eight_dots_wide(packed: &Packed24<'_>, samples: *const f64, rel: usize) -> [f64; 8] {
    use std::arch::aarch64::{
        vaddq_f64, vdupq_n_f64, vgetq_lane_f64, vld1q_dup_f64, vld1q_f64, vmulq_f64, vzip1q_f64,
    };
    unsafe {
        let at = samples.add(rel);
        let even = packed.even.as_ptr();
        let odd = packed.odd.as_ptr();
        let mut s01 = vdupq_n_f64(0.0);
        let mut s23 = vdupq_n_f64(0.0);
        let mut s45 = vdupq_n_f64(0.0);
        let mut s67 = vdupq_n_f64(0.0);
        let mut k = 0_usize;
        while k < TAPS as usize {
            let c_even = vld1q_dup_f64(even.add(k));
            let c_odd = vld1q_dup_f64(odd.add(k));
            let coeff = vzip1q_f64(c_even, c_odd);
            s01 = vaddq_f64(s01, vmulq_f64(vld1q_f64(at.add(k)), coeff));
            s23 = vaddq_f64(s23, vmulq_f64(vld1q_f64(at.add(k + 3)), coeff));
            s45 = vaddq_f64(s45, vmulq_f64(vld1q_f64(at.add(k + 6)), coeff));
            s67 = vaddq_f64(s67, vmulq_f64(vld1q_f64(at.add(k + 9)), coeff));
            k += 1;
        }
        [
            vgetq_lane_f64(s01, 0),
            vgetq_lane_f64(s01, 1),
            vgetq_lane_f64(s23, 0),
            vgetq_lane_f64(s23, 1),
            vgetq_lane_f64(s45, 0),
            vgetq_lane_f64(s45, 1),
            vgetq_lane_f64(s67, 0),
            vgetq_lane_f64(s67, 1),
        ]
    }
}

#[cfg(all(test, target_arch = "aarch64"))]
static WIDE_HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Eight left-to-right dots from absolute input `start`. Each lane is one
/// output. Tap `k` reads the adjacent pairs at offsets `(0, 1)`, `(3, 4)`,
/// `(6, 7)`, and `(9, 10)`, widens them, multiplies, then adds.
///
/// # Safety
///
/// [`group_fits`] is true for `start`. NEON is available. The
/// multiply-then-add order is unchanged. Each tap reads `start + k` through
/// `start + k + 10`. The last of those is the index [`group_fits`] allows.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn eight_dots(packed: &Packed24<'_>, start: usize) -> [f64; 8] {
    use std::arch::aarch64::{
        vaddq_f64, vcvt_f64_f32, vdupq_n_f64, vgetq_lane_f64, vld1_f32, vld1q_dup_f64, vmulq_f64,
        vzip1q_f64,
    };
    debug_assert!(group_fits(packed, start));
    unsafe {
        let at = packed.input.as_ptr().add(start);
        let even = packed.even.as_ptr();
        let odd = packed.odd.as_ptr();
        // Lane 0 is the even output of the pair. Lane 1 is the odd output.
        let mut s01 = vdupq_n_f64(0.0);
        let mut s23 = vdupq_n_f64(0.0);
        let mut s45 = vdupq_n_f64(0.0);
        let mut s67 = vdupq_n_f64(0.0);
        let mut k = 0_usize;
        while k < TAPS as usize {
            let c_even = vld1q_dup_f64(even.add(k));
            let c_odd = vld1q_dup_f64(odd.add(k));
            let coeff = vzip1q_f64(c_even, c_odd);
            let here = at.add(k);
            s01 = vaddq_f64(s01, vmulq_f64(vcvt_f64_f32(vld1_f32(here)), coeff));
            s23 = vaddq_f64(s23, vmulq_f64(vcvt_f64_f32(vld1_f32(here.add(3))), coeff));
            s45 = vaddq_f64(s45, vmulq_f64(vcvt_f64_f32(vld1_f32(here.add(6))), coeff));
            s67 = vaddq_f64(s67, vmulq_f64(vcvt_f64_f32(vld1_f32(here.add(9))), coeff));
            k += 1;
        }
        [
            vgetq_lane_f64(s01, 0),
            vgetq_lane_f64(s01, 1),
            vgetq_lane_f64(s23, 0),
            vgetq_lane_f64(s23, 1),
            vgetq_lane_f64(s45, 0),
            vgetq_lane_f64(s45, 1),
            vgetq_lane_f64(s67, 0),
            vgetq_lane_f64(s67, 1),
        ]
    }
}

#[cfg(test)]
fn assert_schedule(n: u64) {
    let place = match super::placement(n, 3, 2, TAPS) {
        Ok(place) => place,
        Err(err) => panic!("placement {n}: {err:?}"),
    };
    let even = n.is_multiple_of(2);
    assert_eq!(place.frac.to_bits(), 0.0_f64.to_bits(), "frac {n}");
    assert_eq!(
        place.phase,
        if even { 0 } else { PHASE_COUNT / 2 },
        "phase {n}"
    );
    if even {
        let origin = match even_origin(n) {
            Some(origin) => origin,
            None => panic!("origin {n} does not fit"),
        };
        assert_eq!(place.origin, origin, "origin {n}");
    } else {
        let origin = match even_origin(n - 1) {
            Some(origin) => origin,
            None => panic!("origin {n} does not fit"),
        };
        let next = match origin.checked_add(1) {
            Some(next) => next,
            None => panic!("origin {n} overflow"),
        };
        assert_eq!(place.origin, next, "origin {n}");
    }
}

#[cfg(test)]
fn blocked_matches(frames: usize) {
    let from = match crate::Rate::new(24_000) {
        Ok(rate) => rate,
        Err(err) => panic!("24 kHz: {err:?}"),
    };
    let to = match crate::Rate::new(16_000) {
        Ok(rate) => rate,
        Err(err) => panic!("16 kHz: {err:?}"),
    };
    let ratio = crate::Ratio::reduce(from, to);
    let delay = match crate::group_delay(TAPS, crate::coefficients::PHASES, ratio) {
        Ok(delay) => delay,
        Err(err) => panic!("group delay: {err:?}"),
    };
    let delay = match usize::try_from(delay) {
        Ok(delay) => delay,
        Err(_) => panic!("group delay {delay} does not fit"),
    };
    let mut input = vec![0.0_f32; frames];
    for (index, sample) in input.iter_mut().enumerate() {
        let n = index as f32;
        *sample = (n * 0.13).sin() * 0.6 + ((n + 4.0) * 0.05).cos() * 0.2;
    }
    if let Some(sample) = input.first_mut() {
        *sample = -0.0;
    }
    if let Some(sample) = input.get_mut(1) {
        *sample = f32::from_bits(0x0080_0001);
    }
    if let Some(sample) = input.get_mut(frames / 2) {
        *sample = f32::from_bits(0x0080_0001);
    }
    if let Some(sample) = input.last_mut() {
        *sample = -1.0;
    }
    let slow = match super::raw_samples(&input, from, to, super::Engine::Scalar, false, delay) {
        Ok(samples) => samples,
        Err(err) => panic!("{frames} slow failed: {err:?}"),
    };
    let fast = match super::raw_samples(&input, from, to, super::Engine::Scalar, true, delay) {
        Ok(samples) => samples,
        Err(err) => panic!("{frames} fast failed: {err:?}"),
    };
    assert_eq!(slow.len(), fast.len(), "{frames} length");
    assert!(!slow.is_empty(), "{frames} produced no samples");
    for (index, (left, right)) in slow.iter().zip(fast.iter()).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "{frames} sample {index}: slow {left:?} {:08x} fast {right:?} {:08x}",
            left.to_bits(),
            right.to_bits(),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn schedule_matches_placement() {
        for n in [0_u64, 1, 2, 3, 186, 187, 310, 311, 1_000, 10_000, 1_000_000] {
            super::assert_schedule(n);
        }
    }

    #[test]
    fn blocked_24000_matches_scalar_dots_at_zero_ulp() {
        // 3 divides each length. 3 stays on the scalar edges. The longer
        // buffers reach interior blocks of eight outputs.
        for frames in [3_usize, 768, 3_072] {
            super::blocked_matches(frames);
        }
        #[cfg(target_arch = "aarch64")]
        assert!(
            super::WIDE_HITS.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "the widened window was not used"
        );
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn hot_path_borrows_input_and_fits_only_indexes_inside_it() {
        // The two 373-tap rows are static borrows. The input is borrowed.
        let frames = 8_000_usize;
        let input = vec![0.0_f32; frames];
        let row = vec![0.0_f32; super::TAPS as usize];
        let packed = match super::pack_24(&input, &row, &row) {
            Ok(packed) => packed,
            Err(err) => panic!("pack failed: {err:?}"),
        };
        assert_eq!(packed.input.as_ptr(), input.as_ptr(), "input is borrowed");
        assert_eq!(packed.input.len(), input.len());
        assert_eq!(packed.even.len(), super::TAPS as usize);
        assert_eq!(packed.odd.len(), super::TAPS as usize);
        assert!(
            packed.even.len() < input.len() && packed.odd.len() < input.len(),
            "coefficient rows must stay smaller than the input"
        );
        assert_eq!(
            std::mem::size_of_val(&packed),
            std::mem::size_of::<&[f32]>() + 2 * std::mem::size_of::<&[f64]>(),
            "no sample vector is stored beside the two coefficient rows"
        );
        let offset = match 7_usize
            .checked_add(super::TAPS as usize - 1)
            .and_then(|at| at.checked_add(3))
        {
            Some(offset) => offset,
            None => panic!("furthest offset overflow"),
        };
        let last_fit = match frames
            .checked_sub(offset)
            .and_then(|end| end.checked_sub(1))
        {
            Some(start) => start,
            None => panic!("input shorter than one group"),
        };
        assert!(
            super::group_fits(&packed, 0),
            "a group at the start of a long input"
        );
        assert!(
            super::group_fits(&packed, last_fit),
            "furthest index still inside the input"
        );
        let past = match last_fit.checked_add(1) {
            Some(past) => past,
            None => panic!("past the input overflow"),
        };
        assert!(
            !super::group_fits(&packed, past),
            "furthest index leaves the input"
        );
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn short_pack_misses_a_group_that_leaves_the_input() {
        let frames = 1_000_usize;
        let input = vec![0.0_f32; frames];
        let row = vec![0.0_f32; super::TAPS as usize];
        let packed = match super::pack_24(&input, &row, &row) {
            Ok(packed) => packed,
            Err(err) => panic!("pack failed: {err:?}"),
        };
        let inside = match super::even_origin(400) {
            Some(origin) => origin,
            None => panic!("origin 400 does not fit"),
        };
        let outside = match super::even_origin(800) {
            Some(origin) => origin,
            None => panic!("origin 800 does not fit"),
        };
        let inside = match usize::try_from(inside) {
            Ok(start) => start,
            Err(_) => panic!("inside start does not fit"),
        };
        let outside = match usize::try_from(outside) {
            Ok(start) => start,
            Err(_) => panic!("outside start does not fit"),
        };
        assert!(super::group_fits(&packed, inside), "group inside the input");
        assert!(
            !super::group_fits(&packed, outside),
            "group that leaves the input"
        );
    }
}
