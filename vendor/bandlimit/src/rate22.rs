//! 22.05 kHz to 16 kHz. The reduced ratio is 441/320.
//!
//! The rows are the 235-tap Kaiser table, one phase per denominator step.
//! Every fraction is zero, so each output is one left-to-right f64 dot, the
//! same product as [`super::sample`]. Eight outputs run together so those
//! chains overlap. Stored coefficients stay `f32`; each product widens one of
//! them. A window that leaves the input uses [`super::sample`].

use super::{Engine, Error};
use crate::coefficients::{TAPS_22_16, Table};

const PERIOD: u64 = 320;
const IN_STEP: i64 = 441;
const OUT_RATE: i64 = 320;
const DEN: f64 = 640.0;

#[derive(Clone, Copy)]
struct Step {
    origin: i64,
    phase: u16,
    phase_rem: u16,
}

#[derive(Clone, Copy)]
struct Resolved {
    origin: i64,
    phase: usize,
    frac: f64,
}

const fn place(n: i128) -> Step {
    let taps = TAPS_22_16 as i128;
    let phases = 640_i128;
    let shifted = n * 2 - taps + 1;
    let num = shifted * (IN_STEP as i128) - (taps - 1) * (OUT_RATE as i128);
    let den = (OUT_RATE as i128) * 2;
    let origin = num.div_euclid(den);
    let rem = num.rem_euclid(den);
    let scaled = rem * phases;
    Step {
        origin: origin as i64,
        phase: scaled.div_euclid(den) as u16,
        phase_rem: scaled.rem_euclid(den) as u16,
    }
}

const fn steps() -> [Step; PERIOD as usize] {
    let mut out = [Step {
        origin: 0,
        phase: 0,
        phase_rem: 0,
    }; PERIOD as usize];
    let mut n = 0_i128;
    while n < PERIOD as i128 {
        out[n as usize] = place(n);
        n += 1;
    }
    out
}

const STEPS: [Step; PERIOD as usize] = steps();

pub(super) fn fill(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
) -> Result<Vec<f32>, Error> {
    fill_wide(input, coeffs, raw_len, engine, first, None)
}

pub(super) fn fill_into(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
    output: &mut [f32],
) -> Result<(), Error> {
    write_range(input, coeffs, raw_len, engine, first, output, None)
}

pub(super) fn fill_wide(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
    wide: Option<&[f64]>,
) -> Result<Vec<f32>, Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let mut samples = Vec::new();
    if samples.try_reserve_exact(span).is_err() {
        return Err(Error::Overflow);
    }
    samples.resize(span, 0.0);
    write_range(input, coeffs, raw_len, engine, first, &mut samples, wide)?;
    Ok(samples)
}

fn write_range(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    engine: Engine,
    first: usize,
    output: &mut [f32],
    wide: Option<&[f64]>,
) -> Result<(), Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    if output.len() != span {
        return Err(Error::Overflow);
    }
    let rows = coeffs.taps == TAPS_22_16;
    let mut n = first;
    while n < raw_len {
        if rows
            && raw_len - n >= 8
            && let Ok(start) = u64::try_from(n)
            && let Some(block) = block8(input, coeffs, start)?
        {
            let at = n - first;
            output[at..at + 8].copy_from_slice(&block);
            n += 8;
            continue;
        }
        if rows
            && raw_len - n >= 4
            && let Ok(start) = u64::try_from(n)
            && let Some(block) = block4(input, coeffs, start, wide)?
        {
            let at = n - first;
            output[at..at + 4].copy_from_slice(&block);
            n += 4;
            continue;
        }
        let index = match u64::try_from(n) {
            Ok(index) => index,
            Err(_) => return Err(Error::Overflow),
        };
        output[n - first] = super::sample(
            input,
            coeffs,
            index,
            IN_STEP as u64,
            OUT_RATE as u64,
            engine,
        )?;
        n += 1;
    }
    Ok(())
}

fn resolve(n: u64) -> Result<Resolved, Error> {
    let step = STEPS[(n % PERIOD) as usize];
    let period = match i64::try_from(n / PERIOD) {
        Ok(period) => period,
        Err(_) => return Err(Error::Overflow),
    };
    let shift = match period.checked_mul(IN_STEP) {
        Some(shift) => shift,
        None => return Err(Error::Overflow),
    };
    let origin = match step.origin.checked_add(shift) {
        Some(origin) => origin,
        None => return Err(Error::Overflow),
    };
    Ok(Resolved {
        origin,
        phase: usize::from(step.phase),
        frac: f64::from(step.phase_rem) / DEN,
    })
}

fn block8(input: &[f32], coeffs: &Table, n: u64) -> Result<Option<[f32; 8]>, Error> {
    let mut resolved = [Resolved {
        origin: 0,
        phase: 0,
        frac: 0.0,
    }; 8];
    for (lane, slot) in resolved.iter_mut().enumerate() {
        let lane = match u64::try_from(lane) {
            Ok(lane) => lane,
            Err(_) => return Err(Error::Overflow),
        };
        let index = match n.checked_add(lane) {
            Some(index) => index,
            None => return Err(Error::Overflow),
        };
        let step = resolve(index)?;
        if step.phase >= coeffs.phases() || step.frac != 0.0 {
            return Ok(None);
        }
        *slot = step;
    }
    block8_exact(input, coeffs, &resolved)
}

fn block8_exact(
    input: &[f32],
    coeffs: &Table,
    resolved: &[Resolved; 8],
) -> Result<Option<[f32; 8]>, Error> {
    let taps = coeffs.taps as usize;
    let mut windows: [&[f32]; 8] = [&[]; 8];
    let mut rows: [&[f32]; 8] = [&[]; 8];
    for lane in 0..8 {
        let Some(window) = window(input, resolved[lane].origin, taps) else {
            return Ok(None);
        };
        let row = coeffs.row(resolved[lane].phase)?;
        if window.len() != taps || row.len() != taps {
            return Ok(None);
        }
        windows[lane] = window;
        rows[lane] = row;
    }
    let sums = dots8_one(windows, rows);
    let mut out = [0.0_f32; 8];
    for lane in 0..8 {
        out[lane] = scale(sums[lane], 0.0, 0.0)?;
    }
    Ok(Some(out))
}

fn block4(
    input: &[f32],
    coeffs: &Table,
    n: u64,
    wide: Option<&[f64]>,
) -> Result<Option<[f32; 4]>, Error> {
    let taps = coeffs.taps as usize;
    let mut resolved = [Resolved {
        origin: 0,
        phase: 0,
        frac: 0.0,
    }; 4];
    for (lane, slot) in resolved.iter_mut().enumerate() {
        let lane = match u64::try_from(lane) {
            Ok(lane) => lane,
            Err(_) => return Err(Error::Overflow),
        };
        let index = match n.checked_add(lane) {
            Some(index) => index,
            None => return Err(Error::Overflow),
        };
        let step = resolve(index)?;
        if step.phase >= coeffs.phases() {
            return Ok(None);
        }
        *slot = step;
    }
    if resolved.iter().all(|step| step.frac == 0.0) {
        return block4_exact(input, coeffs, &resolved, wide);
    }
    for step in &resolved {
        if step.phase + 1 >= coeffs.phases() {
            return Ok(None);
        }
    }
    let Some(w0) = window(input, resolved[0].origin, taps) else {
        return Ok(None);
    };
    let Some(w1) = window(input, resolved[1].origin, taps) else {
        return Ok(None);
    };
    let Some(w2) = window(input, resolved[2].origin, taps) else {
        return Ok(None);
    };
    let Some(w3) = window(input, resolved[3].origin, taps) else {
        return Ok(None);
    };
    let a0 = coeffs.row(resolved[0].phase)?;
    let b0 = coeffs.row(resolved[0].phase + 1)?;
    let a1 = coeffs.row(resolved[1].phase)?;
    let b1 = coeffs.row(resolved[1].phase + 1)?;
    let a2 = coeffs.row(resolved[2].phase)?;
    let b2 = coeffs.row(resolved[2].phase + 1)?;
    let a3 = coeffs.row(resolved[3].phase)?;
    let b3 = coeffs.row(resolved[3].phase + 1)?;
    if w0.len() != taps
        || w1.len() != taps
        || w2.len() != taps
        || w3.len() != taps
        || a0.len() != taps
        || b0.len() != taps
        || a1.len() != taps
        || b1.len() != taps
        || a2.len() != taps
        || b2.len() != taps
        || a3.len() != taps
        || b3.len() != taps
    {
        return Ok(None);
    }
    #[cfg(target_arch = "aarch64")]
    if let Some(wide) = wide
        && let Some(fa0) = super::phase_f64(wide, resolved[0].phase, taps)
        && let Some(fb0) = super::phase_f64(wide, resolved[0].phase + 1, taps)
        && let Some(fa1) = super::phase_f64(wide, resolved[1].phase, taps)
        && let Some(fb1) = super::phase_f64(wide, resolved[1].phase + 1, taps)
        && let Some(fa2) = super::phase_f64(wide, resolved[2].phase, taps)
        && let Some(fb2) = super::phase_f64(wide, resolved[2].phase + 1, taps)
        && let Some(fa3) = super::phase_f64(wide, resolved[3].phase, taps)
        && let Some(fb3) = super::phase_f64(wide, resolved[3].phase + 1, taps)
        && let Some(sums) =
            super::f64_dots4([w0, w1, w2, w3], [fa0, fa1, fa2, fa3], [fb0, fb1, fb2, fb3])
    {
        let mut out = [0.0_f32; 4];
        for lane in 0..4 {
            out[lane] = scale(sums[lane], sums[lane + 4], resolved[lane].frac)?;
        }
        return Ok(Some(out));
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = wide;
    let sums = dots4([w0, w1, w2, w3], [a0, a1, a2, a3], [b0, b1, b2, b3]);
    let mut out = [0.0_f32; 4];
    for lane in 0..4 {
        out[lane] = scale(sums[lane], sums[lane + 4], resolved[lane].frac)?;
    }
    Ok(Some(out))
}

fn window(input: &[f32], origin: i64, taps: usize) -> Option<&[f32]> {
    if origin < 0 {
        return None;
    }
    let start = usize::try_from(origin).ok()?;
    let end = start.checked_add(taps)?;
    input.get(start..end)
}

/// Eight left-to-right dots. Lanes 0..4 are the first phase, 4..8 the next.
/// On aarch64 each lane is one NEON accumulator. Products are `vmul` then `vadd`.
#[inline(always)]
fn dots4(w: [&[f32]; 4], a: [&[f32]; 4], b: [&[f32]; 4]) -> [f64; 8] {
    #[cfg(target_arch = "aarch64")]
    if let Some(sums) = dots4_neon(w, a, b) {
        return sums;
    }
    dots4_scalar(w, a, b)
}

#[inline(always)]
fn dots4_scalar(w: [&[f32]; 4], a: [&[f32]; 4], b: [&[f32]; 4]) -> [f64; 8] {
    let (w0, w1, w2, w3) = (w[0], w[1], w[2], w[3]);
    let (a0, a1, a2, a3) = (a[0], a[1], a[2], a[3]);
    let (b0, b1, b2, b3) = (b[0], b[1], b[2], b[3]);
    let mut s0 = 0.0_f64;
    let mut s1 = 0.0_f64;
    let mut s2 = 0.0_f64;
    let mut s3 = 0.0_f64;
    let mut t0 = 0.0_f64;
    let mut t1 = 0.0_f64;
    let mut t2 = 0.0_f64;
    let mut t3 = 0.0_f64;
    let n = w0.len();
    for i in 0..n {
        let x0 = f64::from(w0[i]);
        let x1 = f64::from(w1[i]);
        let x2 = f64::from(w2[i]);
        let x3 = f64::from(w3[i]);
        s0 += x0 * f64::from(a0[i]);
        t0 += x0 * f64::from(b0[i]);
        s1 += x1 * f64::from(a1[i]);
        t1 += x1 * f64::from(b1[i]);
        s2 += x2 * f64::from(a2[i]);
        t2 += x2 * f64::from(b2[i]);
        s3 += x3 * f64::from(a3[i]);
        t3 += x3 * f64::from(b3[i]);
    }
    [s0, s1, s2, s3, t0, t1, t2, t3]
}

/// A pair load reads two `f32`s. Every row must cover `n`, or the scalar dots run.
#[cfg(target_arch = "aarch64")]
fn rows_fit(n: usize, w: [&[f32]; 4], a: [&[f32]; 4], b: [&[f32]; 4]) -> bool {
    let fit = |rows: [&[f32]; 4]| {
        rows[0].len() == n && rows[1].len() == n && rows[2].len() == n && rows[3].len() == n
    };
    fit(w) && fit(a) && fit(b)
}

#[cfg(target_arch = "aarch64")]
fn dots4_neon(w: [&[f32]; 4], a: [&[f32]; 4], b: [&[f32]; 4]) -> Option<[f64; 8]> {
    let n = w[0].len();
    if !rows_fit(n, w, a, b) {
        return None;
    }
    // SAFETY: `rows_fit` kept every pair load and the odd tail inside each row.
    // NEON is baseline on aarch64. Products are `vmul` then `vadd`.
    Some(unsafe { dots4_lanes(w, a, b, n) })
}

/// Each lane is one running sum. Two taps are widened and zipped back into
/// window order, then added. Tap `i` joins that lane before tap `i + 1`.
/// The zip is not a sum across taps.
///
/// # Safety
///
/// Every slice length is `n`. NEON is available.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn dots4_lanes(w: [&[f32]; 4], a: [&[f32]; 4], b: [&[f32]; 4], n: usize) -> [f64; 8] {
    use std::arch::aarch64::{
        vaddq_f64, vcvt_f64_f32, vdup_n_f32, vdupq_n_f64, vgetq_lane_f64, vld1_f32, vld1_lane_f32,
        vmulq_f64, vzip1q_f64, vzip2q_f64,
    };
    unsafe {
        let w0 = w[0].as_ptr();
        let w1 = w[1].as_ptr();
        let w2 = w[2].as_ptr();
        let w3 = w[3].as_ptr();
        let a0 = a[0].as_ptr();
        let a1 = a[1].as_ptr();
        let a2 = a[2].as_ptr();
        let a3 = a[3].as_ptr();
        let b0 = b[0].as_ptr();
        let b1 = b[1].as_ptr();
        let b2 = b[2].as_ptr();
        let b3 = b[3].as_ptr();
        let mut s01 = vdupq_n_f64(0.0);
        let mut s23 = vdupq_n_f64(0.0);
        let mut t01 = vdupq_n_f64(0.0);
        let mut t23 = vdupq_n_f64(0.0);
        let mut i = 0;
        let pairs = n / 2 * 2;
        while i < pairs {
            let x0 = vcvt_f64_f32(vld1_f32(w0.add(i)));
            let x1 = vcvt_f64_f32(vld1_f32(w1.add(i)));
            let x2 = vcvt_f64_f32(vld1_f32(w2.add(i)));
            let x3 = vcvt_f64_f32(vld1_f32(w3.add(i)));
            let p0 = vcvt_f64_f32(vld1_f32(a0.add(i)));
            let p1 = vcvt_f64_f32(vld1_f32(a1.add(i)));
            let p2 = vcvt_f64_f32(vld1_f32(a2.add(i)));
            let p3 = vcvt_f64_f32(vld1_f32(a3.add(i)));
            let q0 = vcvt_f64_f32(vld1_f32(b0.add(i)));
            let q1 = vcvt_f64_f32(vld1_f32(b1.add(i)));
            let q2 = vcvt_f64_f32(vld1_f32(b2.add(i)));
            let q3 = vcvt_f64_f32(vld1_f32(b3.add(i)));
            let (x01_0, x01_1) = (vzip1q_f64(x0, x1), vzip2q_f64(x0, x1));
            let (x23_0, x23_1) = (vzip1q_f64(x2, x3), vzip2q_f64(x2, x3));
            let (a01_0, a01_1) = (vzip1q_f64(p0, p1), vzip2q_f64(p0, p1));
            let (a23_0, a23_1) = (vzip1q_f64(p2, p3), vzip2q_f64(p2, p3));
            let (b01_0, b01_1) = (vzip1q_f64(q0, q1), vzip2q_f64(q0, q1));
            let (b23_0, b23_1) = (vzip1q_f64(q2, q3), vzip2q_f64(q2, q3));
            s01 = vaddq_f64(s01, vmulq_f64(x01_0, a01_0));
            s01 = vaddq_f64(s01, vmulq_f64(x01_1, a01_1));
            s23 = vaddq_f64(s23, vmulq_f64(x23_0, a23_0));
            s23 = vaddq_f64(s23, vmulq_f64(x23_1, a23_1));
            t01 = vaddq_f64(t01, vmulq_f64(x01_0, b01_0));
            t01 = vaddq_f64(t01, vmulq_f64(x01_1, b01_1));
            t23 = vaddq_f64(t23, vmulq_f64(x23_0, b23_0));
            t23 = vaddq_f64(t23, vmulq_f64(x23_1, b23_1));
            i += 2;
        }
        if i < n {
            let one = |left: *const f32, right: *const f32| {
                let mut pair = vdup_n_f32(0.0);
                pair = vld1_lane_f32(left, pair, 0);
                pair = vld1_lane_f32(right, pair, 1);
                vcvt_f64_f32(pair)
            };
            let x01 = one(w0.add(i), w1.add(i));
            let x23 = one(w2.add(i), w3.add(i));
            s01 = vaddq_f64(s01, vmulq_f64(x01, one(a0.add(i), a1.add(i))));
            t01 = vaddq_f64(t01, vmulq_f64(x01, one(b0.add(i), b1.add(i))));
            s23 = vaddq_f64(s23, vmulq_f64(x23, one(a2.add(i), a3.add(i))));
            t23 = vaddq_f64(t23, vmulq_f64(x23, one(b2.add(i), b3.add(i))));
        }
        [
            vgetq_lane_f64(s01, 0),
            vgetq_lane_f64(s01, 1),
            vgetq_lane_f64(s23, 0),
            vgetq_lane_f64(s23, 1),
            vgetq_lane_f64(t01, 0),
            vgetq_lane_f64(t01, 1),
            vgetq_lane_f64(t23, 0),
            vgetq_lane_f64(t23, 1),
        ]
    }
}

/// One dot per lane. The fraction is zero, so the second phase is not read.
fn block4_exact(
    input: &[f32],
    coeffs: &Table,
    resolved: &[Resolved; 4],
    wide: Option<&[f64]>,
) -> Result<Option<[f32; 4]>, Error> {
    let taps = coeffs.taps as usize;
    let Some(w0) = window(input, resolved[0].origin, taps) else {
        return Ok(None);
    };
    let Some(w1) = window(input, resolved[1].origin, taps) else {
        return Ok(None);
    };
    let Some(w2) = window(input, resolved[2].origin, taps) else {
        return Ok(None);
    };
    let Some(w3) = window(input, resolved[3].origin, taps) else {
        return Ok(None);
    };
    let a0 = coeffs.row(resolved[0].phase)?;
    let a1 = coeffs.row(resolved[1].phase)?;
    let a2 = coeffs.row(resolved[2].phase)?;
    let a3 = coeffs.row(resolved[3].phase)?;
    if w0.len() != taps
        || w1.len() != taps
        || w2.len() != taps
        || w3.len() != taps
        || a0.len() != taps
        || a1.len() != taps
        || a2.len() != taps
        || a3.len() != taps
    {
        return Ok(None);
    }
    #[cfg(target_arch = "aarch64")]
    if let Some(wide) = wide
        && let Some(fa0) = super::phase_f64(wide, resolved[0].phase, taps)
        && let Some(fa1) = super::phase_f64(wide, resolved[1].phase, taps)
        && let Some(fa2) = super::phase_f64(wide, resolved[2].phase, taps)
        && let Some(fa3) = super::phase_f64(wide, resolved[3].phase, taps)
        && let Some(sums) = super::f64_dots4_one([w0, w1, w2, w3], [fa0, fa1, fa2, fa3])
    {
        let mut out = [0.0_f32; 4];
        for lane in 0..4 {
            out[lane] = scale(sums[lane], 0.0, 0.0)?;
        }
        return Ok(Some(out));
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = wide;
    let sums = dots4_one([w0, w1, w2, w3], [a0, a1, a2, a3]);
    let mut out = [0.0_f32; 4];
    for lane in 0..4 {
        out[lane] = scale(sums[lane], 0.0, 0.0)?;
    }
    Ok(Some(out))
}

/// Four left-to-right dots, first phase only. Tap `i` joins before tap `i + 1`.
#[inline(always)]
pub(super) fn dots4_one(w: [&[f32]; 4], a: [&[f32]; 4]) -> [f64; 4] {
    #[cfg(target_arch = "aarch64")]
    if let Some(sums) = dots4_one_neon(w, a) {
        return sums;
    }
    let mut sums = [0.0_f64; 4];
    let n = w[0].len();
    for i in 0..n {
        sums[0] += f64::from(w[0][i]) * f64::from(a[0][i]);
        sums[1] += f64::from(w[1][i]) * f64::from(a[1][i]);
        sums[2] += f64::from(w[2][i]) * f64::from(a[2][i]);
        sums[3] += f64::from(w[3][i]) * f64::from(a[3][i]);
    }
    sums
}

#[cfg(target_arch = "aarch64")]
fn dots4_one_neon(w: [&[f32]; 4], a: [&[f32]; 4]) -> Option<[f64; 4]> {
    let n = w[0].len();
    if w[1].len() != n || w[2].len() != n || w[3].len() != n {
        return None;
    }
    if a[0].len() != n || a[1].len() != n || a[2].len() != n || a[3].len() != n {
        return None;
    }
    // SAFETY: every pair load and the odd tail stay inside each row.
    // NEON is baseline on aarch64. Products are `vmul` then `vadd`.
    // Tap `i` joins the lane before tap `i + 1`, same as `dots4_lanes`.
    Some(unsafe { dots4_one_lanes(w, a, n) })
}

/// # Safety
///
/// Every slice length is `n`. NEON is available.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn dots4_one_lanes(w: [&[f32]; 4], a: [&[f32]; 4], n: usize) -> [f64; 4] {
    use std::arch::aarch64::{
        vaddq_f64, vcvt_f64_f32, vdup_n_f32, vdupq_n_f64, vgetq_lane_f64, vld1_f32, vld1_lane_f32,
        vmulq_f64, vzip1q_f64, vzip2q_f64,
    };
    unsafe {
        let w0 = w[0].as_ptr();
        let w1 = w[1].as_ptr();
        let w2 = w[2].as_ptr();
        let w3 = w[3].as_ptr();
        let a0 = a[0].as_ptr();
        let a1 = a[1].as_ptr();
        let a2 = a[2].as_ptr();
        let a3 = a[3].as_ptr();
        let mut s01 = vdupq_n_f64(0.0);
        let mut s23 = vdupq_n_f64(0.0);
        let mut i = 0;
        let pairs = n / 2 * 2;
        while i < pairs {
            let x0 = vcvt_f64_f32(vld1_f32(w0.add(i)));
            let x1 = vcvt_f64_f32(vld1_f32(w1.add(i)));
            let x2 = vcvt_f64_f32(vld1_f32(w2.add(i)));
            let x3 = vcvt_f64_f32(vld1_f32(w3.add(i)));
            let p0 = vcvt_f64_f32(vld1_f32(a0.add(i)));
            let p1 = vcvt_f64_f32(vld1_f32(a1.add(i)));
            let p2 = vcvt_f64_f32(vld1_f32(a2.add(i)));
            let p3 = vcvt_f64_f32(vld1_f32(a3.add(i)));
            let (x01_0, x01_1) = (vzip1q_f64(x0, x1), vzip2q_f64(x0, x1));
            let (x23_0, x23_1) = (vzip1q_f64(x2, x3), vzip2q_f64(x2, x3));
            let (a01_0, a01_1) = (vzip1q_f64(p0, p1), vzip2q_f64(p0, p1));
            let (a23_0, a23_1) = (vzip1q_f64(p2, p3), vzip2q_f64(p2, p3));
            s01 = vaddq_f64(s01, vmulq_f64(x01_0, a01_0));
            s01 = vaddq_f64(s01, vmulq_f64(x01_1, a01_1));
            s23 = vaddq_f64(s23, vmulq_f64(x23_0, a23_0));
            s23 = vaddq_f64(s23, vmulq_f64(x23_1, a23_1));
            i += 2;
        }
        if i < n {
            let one = |left: *const f32, right: *const f32| {
                let mut pair = vdup_n_f32(0.0);
                pair = vld1_lane_f32(left, pair, 0);
                pair = vld1_lane_f32(right, pair, 1);
                vcvt_f64_f32(pair)
            };
            let x01 = one(w0.add(i), w1.add(i));
            let x23 = one(w2.add(i), w3.add(i));
            s01 = vaddq_f64(s01, vmulq_f64(x01, one(a0.add(i), a1.add(i))));
            s23 = vaddq_f64(s23, vmulq_f64(x23, one(a2.add(i), a3.add(i))));
        }
        [
            vgetq_lane_f64(s01, 0),
            vgetq_lane_f64(s01, 1),
            vgetq_lane_f64(s23, 0),
            vgetq_lane_f64(s23, 1),
        ]
    }
}

/// Eight left-to-right dots, one phase each. Tap `i` joins a lane before tap `i + 1`.
pub(super) fn dots8_one(w: [&[f32]; 8], a: [&[f32]; 8]) -> [f64; 8] {
    #[cfg(target_arch = "aarch64")]
    if let Some(sums) = dots8_one_neon(w, a) {
        return sums;
    }
    let mut sums = [0.0_f64; 8];
    let n = w[0].len();
    for i in 0..n {
        for lane in 0..8 {
            sums[lane] += f64::from(w[lane][i]) * f64::from(a[lane][i]);
        }
    }
    sums
}

#[cfg(target_arch = "aarch64")]
fn dots8_one_neon(w: [&[f32]; 8], a: [&[f32]; 8]) -> Option<[f64; 8]> {
    let n = w[0].len();
    if w.iter().any(|row| row.len() != n) || a.iter().any(|row| row.len() != n) {
        return None;
    }
    // SAFETY: every pair load and the odd tail stay inside each row.
    // NEON is baseline on aarch64. Products are `vmul` then `vadd`.
    // Tap `i` joins the lane before tap `i + 1`, same as `dots4_one`.
    Some(unsafe { dots8_one_lanes(w, a, n) })
}

/// # Safety
///
/// Every slice length is `n`. NEON is available.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn dots8_one_lanes(w: [&[f32]; 8], a: [&[f32]; 8], n: usize) -> [f64; 8] {
    use std::arch::aarch64::{
        vaddq_f64, vcvt_f64_f32, vdup_n_f32, vdupq_n_f64, vgetq_lane_f64, vld1_f32, vld1_lane_f32,
        vmulq_f64, vzip1q_f64, vzip2q_f64,
    };
    unsafe {
        let wp: [*const f32; 8] = [
            w[0].as_ptr(),
            w[1].as_ptr(),
            w[2].as_ptr(),
            w[3].as_ptr(),
            w[4].as_ptr(),
            w[5].as_ptr(),
            w[6].as_ptr(),
            w[7].as_ptr(),
        ];
        let ap: [*const f32; 8] = [
            a[0].as_ptr(),
            a[1].as_ptr(),
            a[2].as_ptr(),
            a[3].as_ptr(),
            a[4].as_ptr(),
            a[5].as_ptr(),
            a[6].as_ptr(),
            a[7].as_ptr(),
        ];
        let mut s01 = vdupq_n_f64(0.0);
        let mut s23 = vdupq_n_f64(0.0);
        let mut s45 = vdupq_n_f64(0.0);
        let mut s67 = vdupq_n_f64(0.0);
        let mut i = 0;
        let pairs = n / 2 * 2;
        while i < pairs {
            let x0 = vcvt_f64_f32(vld1_f32(wp[0].add(i)));
            let x1 = vcvt_f64_f32(vld1_f32(wp[1].add(i)));
            let x2 = vcvt_f64_f32(vld1_f32(wp[2].add(i)));
            let x3 = vcvt_f64_f32(vld1_f32(wp[3].add(i)));
            let x4 = vcvt_f64_f32(vld1_f32(wp[4].add(i)));
            let x5 = vcvt_f64_f32(vld1_f32(wp[5].add(i)));
            let x6 = vcvt_f64_f32(vld1_f32(wp[6].add(i)));
            let x7 = vcvt_f64_f32(vld1_f32(wp[7].add(i)));
            let p0 = vcvt_f64_f32(vld1_f32(ap[0].add(i)));
            let p1 = vcvt_f64_f32(vld1_f32(ap[1].add(i)));
            let p2 = vcvt_f64_f32(vld1_f32(ap[2].add(i)));
            let p3 = vcvt_f64_f32(vld1_f32(ap[3].add(i)));
            let p4 = vcvt_f64_f32(vld1_f32(ap[4].add(i)));
            let p5 = vcvt_f64_f32(vld1_f32(ap[5].add(i)));
            let p6 = vcvt_f64_f32(vld1_f32(ap[6].add(i)));
            let p7 = vcvt_f64_f32(vld1_f32(ap[7].add(i)));
            let (x01_0, x01_1) = (vzip1q_f64(x0, x1), vzip2q_f64(x0, x1));
            let (x23_0, x23_1) = (vzip1q_f64(x2, x3), vzip2q_f64(x2, x3));
            let (x45_0, x45_1) = (vzip1q_f64(x4, x5), vzip2q_f64(x4, x5));
            let (x67_0, x67_1) = (vzip1q_f64(x6, x7), vzip2q_f64(x6, x7));
            let (a01_0, a01_1) = (vzip1q_f64(p0, p1), vzip2q_f64(p0, p1));
            let (a23_0, a23_1) = (vzip1q_f64(p2, p3), vzip2q_f64(p2, p3));
            let (a45_0, a45_1) = (vzip1q_f64(p4, p5), vzip2q_f64(p4, p5));
            let (a67_0, a67_1) = (vzip1q_f64(p6, p7), vzip2q_f64(p6, p7));
            s01 = vaddq_f64(s01, vmulq_f64(x01_0, a01_0));
            s01 = vaddq_f64(s01, vmulq_f64(x01_1, a01_1));
            s23 = vaddq_f64(s23, vmulq_f64(x23_0, a23_0));
            s23 = vaddq_f64(s23, vmulq_f64(x23_1, a23_1));
            s45 = vaddq_f64(s45, vmulq_f64(x45_0, a45_0));
            s45 = vaddq_f64(s45, vmulq_f64(x45_1, a45_1));
            s67 = vaddq_f64(s67, vmulq_f64(x67_0, a67_0));
            s67 = vaddq_f64(s67, vmulq_f64(x67_1, a67_1));
            i += 2;
        }
        if i < n {
            let one = |left: *const f32, right: *const f32| {
                let mut pair = vdup_n_f32(0.0);
                pair = vld1_lane_f32(left, pair, 0);
                pair = vld1_lane_f32(right, pair, 1);
                vcvt_f64_f32(pair)
            };
            let x01 = one(wp[0].add(i), wp[1].add(i));
            let x23 = one(wp[2].add(i), wp[3].add(i));
            let x45 = one(wp[4].add(i), wp[5].add(i));
            let x67 = one(wp[6].add(i), wp[7].add(i));
            s01 = vaddq_f64(s01, vmulq_f64(x01, one(ap[0].add(i), ap[1].add(i))));
            s23 = vaddq_f64(s23, vmulq_f64(x23, one(ap[2].add(i), ap[3].add(i))));
            s45 = vaddq_f64(s45, vmulq_f64(x45, one(ap[4].add(i), ap[5].add(i))));
            s67 = vaddq_f64(s67, vmulq_f64(x67, one(ap[6].add(i), ap[7].add(i))));
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

fn scale(dot0: f64, dot1: f64, frac: f64) -> Result<f32, Error> {
    let mut acc = (1.0 - frac) * dot0;
    if frac != 0.0 {
        acc += frac * dot1;
    }
    super::finite_sample(acc)
}

#[cfg(test)]
fn assert_schedule(n: u64) {
    let place = match super::placement(n, IN_STEP as u64, OUT_RATE as u64, TAPS_22_16) {
        Ok(place) => place,
        Err(err) => panic!("placement {n}: {err:?}"),
    };
    let step = match resolve(n) {
        Ok(step) => step,
        Err(err) => panic!("resolve {n}: {err:?}"),
    };
    assert_eq!(place.origin, step.origin, "origin {n}");
    assert_eq!(place.phase, step.phase, "phase {n}");
    assert_eq!(place.frac.to_bits(), step.frac.to_bits(), "frac {n}");
}

#[cfg(test)]
fn blocked_matches(frames: usize) {
    let from = match crate::Rate::new(22_050) {
        Ok(rate) => rate,
        Err(err) => panic!("22.05 kHz: {err:?}"),
    };
    let to = match crate::Rate::new(16_000) {
        Ok(rate) => rate,
        Err(err) => panic!("16 kHz: {err:?}"),
    };
    let ratio = crate::Ratio::reduce(from, to);
    let delay = match crate::group_delay(TAPS_22_16, crate::coefficients::PHASES, ratio) {
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
    if let Some(sample) = input.get_mut(frames / 2 + 1) {
        *sample = -0.0;
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
    fn periodic_steps_match_placement() {
        for n in [0_u64, 1, 186, 305, 320, 321, 625, 640, 1_000, 10_000_000] {
            super::assert_schedule(n);
        }
    }

    #[test]
    fn blocked_22050_matches_scalar_dots_at_zero_ulp() {
        // 441 divides each length. 441 stays on the scalar edges. The longer
        // buffers reach interior blocks of four outputs, including a wrapped phase.
        for frames in [441_usize, 4_410, 5_292] {
            super::blocked_matches(frames);
        }
    }
}
