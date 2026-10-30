//! Integer decimation and integer-2 interpolation.
//!
//! The placement fraction is 0, so the second phase is not blended. The dot
//! is the scalar or AVX dot: each `f32` widens to `f64`, then multiplies and
//! adds run left to right, with no fused multiply-add. 32 kHz to 16 kHz keeps
//! that order. 96 kHz and 192 kHz to 16 kHz are a cascade of two such dots.
//! 48 kHz to 16 kHz uses the residue-class association in `dot_48` instead.

use super::{Engine, Error, PHASE_COUNT, dot};
use crate::coefficients::{STAGE_96_16, STAGE_192_16, TAPS_32_16, Table};

#[cfg(target_arch = "aarch64")]
#[path = "rate32.rs"]
mod rate32;

#[cfg(target_arch = "aarch64")]
#[path = "rate_int.rs"]
mod rate_int;

#[path = "cascade_down.rs"]
mod cascade_down;

pub(crate) use cascade_down::{
    Kind as CascadeKind, classify as cascade_classify, first_input as cascade_first_input,
    history as cascade_history, largest_ready as cascade_largest_ready,
    reference_one as cascade_reference_one, stage2_table as cascade_stage2_table,
    write_outputs as cascade_write,
};

/// `(taps - 1) / 2 * (in_rate + out_rate)` for the 373-tap 1/2 rows.
const CENTER: i128 = (crate::coefficients::TAPS as i128 - 1) / 2 * 3;

/// `div_euclid(n - center, 2)` matches placement. The 373-tap rows keep [`CENTER`].
fn up_center(taps: usize) -> i128 {
    if taps == crate::coefficients::TAPS as usize {
        CENTER
    } else {
        (taps as i128 - 1) / 2 * 3
    }
}

pub(super) enum Kind {
    /// Integer `in_rate / out_rate`.
    Down(u64),
    /// `in_rate / out_rate` is 1/2.
    Up,
}

pub(super) fn kind(in_rate: u64, out_rate: u64) -> Option<Kind> {
    match (in_rate, out_rate) {
        (numerator, 1) if numerator >= 2 => Some(Kind::Down(numerator)),
        (1, 2) => Some(Kind::Up),
        _ => None,
    }
}

pub(super) fn fill(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    kind: Kind,
    engine: Engine,
    first: usize,
) -> Result<Vec<f32>, Error> {
    let mut samples = Vec::new();
    if samples.try_reserve_exact(raw_len - first).is_err() {
        return Err(Error::Overflow);
    }
    let row_even = coeffs.row(0)?;
    match kind {
        Kind::Down(step) => fill_down(input, row_even, raw_len, engine, &mut samples, step, first)?,
        Kind::Up => {
            let row_odd = coeffs.row(PHASE_COUNT / 2)?;
            fill_up(
                input,
                row_even,
                row_odd,
                raw_len,
                engine,
                &mut samples,
                first,
            )?;
        }
    }
    Ok(samples)
}

fn fill_down(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    engine: Engine,
    samples: &mut Vec<f32>,
    step: u64,
    first: usize,
) -> Result<(), Error> {
    // 443 taps at step 3 is only 48 kHz to 16 kHz. Resize uses the capacity
    // `fill` already reserved.
    if step == 3 && row.len() == 443 {
        let Some(span) = raw_len.checked_sub(first) else {
            return Err(Error::Overflow);
        };
        if samples.len() > span {
            return Err(Error::Overflow);
        }
        if samples.capacity() < span {
            let Some(extra) = span.checked_sub(samples.len()) else {
                return Err(Error::Overflow);
            };
            if samples.try_reserve_exact(extra).is_err() {
                return Err(Error::Overflow);
            }
        }
        samples.resize(span, 0.0);
        return fill_48(input, row, raw_len, samples, first);
    }
    // 339 taps at step 2 is only 32 kHz to 16 kHz. The packed kernel keeps the
    // left-to-right dot.
    if step == 2 && row.len() == TAPS_32_16 as usize {
        let Some(span) = raw_len.checked_sub(first) else {
            return Err(Error::Overflow);
        };
        if samples.len() > span {
            return Err(Error::Overflow);
        }
        if samples.capacity() < span {
            let Some(extra) = span.checked_sub(samples.len()) else {
                return Err(Error::Overflow);
            };
            if samples.try_reserve_exact(extra).is_err() {
                return Err(Error::Overflow);
            }
        }
        samples.resize(span, 0.0);
        return fill_32(input, row, raw_len, samples, first, engine);
    }
    // 53 taps at step 6 is the 96 kHz cascade. 105 taps at step 12 is the
    // 192 kHz cascade. Both are two left-to-right dots.
    if (step == 6 && row.len() == STAGE_96_16 as usize)
        || (step == 12 && row.len() == STAGE_192_16 as usize)
    {
        let Some(span) = raw_len.checked_sub(first) else {
            return Err(Error::Overflow);
        };
        if samples.len() > span {
            return Err(Error::Overflow);
        }
        if samples.capacity() < span {
            let Some(extra) = span.checked_sub(samples.len()) else {
                return Err(Error::Overflow);
            };
            if samples.try_reserve_exact(extra).is_err() {
                return Err(Error::Overflow);
            }
        }
        samples.resize(span, 0.0);
        let kind = match step {
            6 => cascade_down::Kind::Hz96,
            _ => cascade_down::Kind::Hz192,
        };
        return cascade_down::fill(kind, input, row, raw_len, samples, first);
    }
    let center = (row.len() as i128 - 1) / 2 * (i128::from(step) + 1);
    let stride = usize::try_from(step).map_err(|_| Error::Overflow)?;
    let window = stride
        .checked_mul(3)
        .and_then(|span| span.checked_add(row.len()));
    let mut n = first;
    while n < raw_len {
        let origin = n as i128 * i128::from(step) - center;
        if raw_len - n >= 4
            && let Ok(start) = usize::try_from(origin)
            && let Some(end) = window.and_then(|len| start.checked_add(len))
            && let Some(input) = input.get(start..end)
        {
            samples.extend_from_slice(&four_outputs(input, row, stride)?);
            n += 4;
            continue;
        }
        push(samples, input, origin, row, engine)?;
        n += 1;
    }
    Ok(())
}

/// 33-tap odd phase at step 1. The products match [`four_outputs`]: each lane
/// adds `f64::from(sample) * f64::from(coeff)` from the first tap to the last.
fn four_halfband(window: &[f32], row: &[f32]) -> Result<[f32; 4], Error> {
    let Some(need) = row.len().checked_add(3) else {
        return Err(Error::Overflow);
    };
    if window.len() < need {
        return four_outputs(window, row, 1);
    }
    let mut sums = [0.0_f64; 4];
    for (q, coefficient) in row.iter().enumerate() {
        let coefficient = f64::from(*coefficient);
        sums[0] += f64::from(window[q]) * coefficient;
        sums[1] += f64::from(window[q + 1]) * coefficient;
        sums[2] += f64::from(window[q + 2]) * coefficient;
        sums[3] += f64::from(window[q + 3]) * coefficient;
    }
    let mut samples = [0.0_f32; 4];
    for (lane, sum) in sums.into_iter().enumerate() {
        samples[lane] = super::finite_sample(sum)?;
    }
    Ok(samples)
}

/// Independent accumulators preserve scalar order and permit auto-vectorization.
/// 48 kHz to 16 kHz uses the residue-class association instead.
pub(crate) fn four_outputs(input: &[f32], row: &[f32], step: usize) -> Result<[f32; 4], Error> {
    if step == 3 && row.len() == 443 {
        return four_residues(input, row);
    }
    let mut sums = [0.0_f64; 4];
    let windows = input
        .iter()
        .zip(&input[step..])
        .zip(&input[2 * step..])
        .zip(&input[3 * step..]);
    for ((((a, b), c), d), coefficient) in windows.zip(row) {
        let coefficient = f64::from(*coefficient);
        sums[0] += f64::from(*a) * coefficient;
        sums[1] += f64::from(*b) * coefficient;
        sums[2] += f64::from(*c) * coefficient;
        sums[3] += f64::from(*d) * coefficient;
    }
    let mut samples = [0.0_f32; 4];
    for (lane, sum) in sums.into_iter().enumerate() {
        samples[lane] = super::finite_sample(sum)?;
    }
    Ok(samples)
}

/// `(443 - 1) / 2 * (3 + 1)`. Raw output `n` reads input `3 * n - CENTER_48`.
const CENTER_48: i128 = 884;
/// First raw index whose window starts at a non-negative input.
const FULL_48: usize = 295;

fn four_residues(input: &[f32], row: &[f32]) -> Result<[f32; 4], Error> {
    let mut samples = [0.0_f32; 4];
    for (lane, origin) in [0_i64, 3, 6, 9].into_iter().enumerate() {
        let acc = super::dot_48(origin, row, |index| super::sample_at(input, index));
        samples[lane] = super::finite_sample(acc)?;
    }
    Ok(samples)
}

fn origin_48(n: usize) -> Result<i64, Error> {
    let n = match i128::try_from(n) {
        Ok(n) => n,
        Err(_) => return Err(Error::Overflow),
    };
    let origin = match n
        .checked_mul(3)
        .and_then(|scaled| scaled.checked_sub(CENTER_48))
    {
        Some(origin) => origin,
        None => return Err(Error::Overflow),
    };
    match i64::try_from(origin) {
        Ok(origin) => Ok(origin),
        Err(_) => Err(Error::Overflow),
    }
}

fn store_48(samples: &mut [f32], first: usize, n: usize, value: f32) -> Result<(), Error> {
    let Some(at) = n.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let Some(slot) = samples.get_mut(at) else {
        return Err(Error::Overflow);
    };
    *slot = value;
    Ok(())
}

fn push_48(
    samples: &mut [f32],
    first: usize,
    input: &[f32],
    n: usize,
    row: &[f32],
) -> Result<(), Error> {
    let acc = super::dot_48(origin_48(n)?, row, |index| super::sample_at(input, index));
    store_48(samples, first, n, super::finite_sample(acc)?)
}

/// `samples` has one slot per output in `first..raw_len`.
pub(super) fn fill_into_32(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
    engine: Engine,
) -> Result<(), Error> {
    if row.len() != TAPS_32_16 as usize {
        return Err(Error::Overflow);
    }
    fill_32(input, row, raw_len, samples, first, engine)
}

fn fill_32(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
    engine: Engine,
) -> Result<(), Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    if samples.len() != span {
        return Err(Error::Overflow);
    }
    #[cfg(target_arch = "aarch64")]
    let (lo, hi) = rate32::widen(input, row, raw_len, samples, first)?;
    #[cfg(not(target_arch = "aarch64"))]
    let (lo, hi) = (first, first);
    write_scalar_span(input, row, samples, first, engine, first, lo)?;
    write_scalar_span(input, row, samples, first, engine, hi, raw_len)
}

/// Outputs `n` in `from..stop` that the packed kernel did not write.
fn write_scalar_span(
    input: &[f32],
    row: &[f32],
    samples: &mut [f32],
    first: usize,
    engine: Engine,
    mut n: usize,
    stop: usize,
) -> Result<(), Error> {
    let center = (row.len() as i128 - 1) / 2 * 3;
    let stride = 2_usize;
    let window = stride
        .checked_mul(3)
        .and_then(|span| span.checked_add(row.len()));
    while n < stop {
        let Some(origin) = i128::try_from(n)
            .ok()
            .and_then(|index| index.checked_mul(2))
            .and_then(|scaled| scaled.checked_sub(center))
        else {
            return Err(Error::Overflow);
        };
        if stop - n >= 4
            && let Ok(start) = usize::try_from(origin)
            && let Some(end) = window.and_then(|len| start.checked_add(len))
            && let Some(window_samples) = input.get(start..end)
        {
            let four = four_outputs(window_samples, row, stride)?;
            for (lane, value) in four.into_iter().enumerate() {
                let Some(at) = n.checked_add(lane) else {
                    return Err(Error::Overflow);
                };
                store_down(samples, first, at, value)?;
            }
            n += 4;
            continue;
        }
        let Ok(origin) = i64::try_from(origin) else {
            return Err(Error::Overflow);
        };
        store_down(
            samples,
            first,
            n,
            super::finite_sample(dot(input, origin, row, engine))?,
        )?;
        n += 1;
    }
    Ok(())
}

pub(super) fn fill_into_96(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
    _engine: Engine,
) -> Result<(), Error> {
    if row.len() != STAGE_96_16 as usize {
        return Err(Error::Overflow);
    }
    cascade_down::fill(
        cascade_down::Kind::Hz96,
        input,
        row,
        raw_len,
        samples,
        first,
    )
}

pub(super) fn fill_into_192(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
    _engine: Engine,
) -> Result<(), Error> {
    if row.len() != STAGE_192_16 as usize {
        return Err(Error::Overflow);
    }
    cascade_down::fill(
        cascade_down::Kind::Hz192,
        input,
        row,
        raw_len,
        samples,
        first,
    )
}

fn store_down(samples: &mut [f32], first: usize, n: usize, value: f32) -> Result<(), Error> {
    let Some(at) = n.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let Some(slot) = samples.get_mut(at) else {
        return Err(Error::Overflow);
    };
    *slot = value;
    Ok(())
}

/// `samples` has one slot per output in `first..raw_len`.
pub(super) fn fill_into_48(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(), Error> {
    if row.len() != 443 {
        return Err(Error::Overflow);
    }
    fill_48(input, row, raw_len, samples, first)
}

fn fill_48(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(), Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    if samples.len() != span {
        return Err(Error::Overflow);
    }
    #[cfg(target_arch = "aarch64")]
    let mut n = widen_48(input, row, raw_len, samples, first)?;
    #[cfg(not(target_arch = "aarch64"))]
    let mut n = first;
    while n < raw_len {
        push_48(samples, first, input, n, row)?;
        n += 1;
    }
    Ok(())
}

/// Packed residue classes of the 443-tap row. `sample[r][k]` is absolute input
/// `3 * (base[r] + k) + r`.
#[cfg(target_arch = "aarch64")]
struct Packed48 {
    coeff: [Vec<f64>; 3],
    sample: [Vec<f64>; 3],
    base: [usize; 3],
}

/// Borrowed residue classes. The heap pack and the stack pack both produce one.
#[cfg(target_arch = "aarch64")]
struct View<'a> {
    coeff: [&'a [f64]; 3],
    sample: [&'a [f64]; 3],
    base: [usize; 3],
}

#[cfg(target_arch = "aarch64")]
impl Packed48 {
    fn view(&self) -> View<'_> {
        View {
            coeff: [&self.coeff[0], &self.coeff[1], &self.coeff[2]],
            sample: [&self.sample[0], &self.sample[1], &self.sample[2]],
            base: self.base,
        }
    }
}

/// A 4096-sample chunk fits here, so a stream write does not have to allocate.
#[cfg(target_arch = "aarch64")]
pub(crate) const STACK_INPUT: usize = 4_096;
#[cfg(target_arch = "aarch64")]
const COEFF_CAP: usize = 148;
#[cfg(target_arch = "aarch64")]
const SAMPLE_CAP: usize = STACK_INPUT.div_ceil(3);

#[cfg(target_arch = "aarch64")]
pub(crate) struct StackPack {
    coeff: [[f64; COEFF_CAP]; 3],
    sample: [[f64; SAMPLE_CAP]; 3],
    coeff_len: [usize; 3],
    sample_len: [usize; 3],
    base: [usize; 3],
}

#[cfg(target_arch = "aarch64")]
impl StackPack {
    pub(crate) fn new() -> Self {
        Self {
            coeff: [[0.0; COEFF_CAP]; 3],
            sample: [[0.0; SAMPLE_CAP]; 3],
            coeff_len: [0; 3],
            sample_len: [0; 3],
            base: [0; 3],
        }
    }

    fn view(&self) -> View<'_> {
        View {
            coeff: [
                &self.coeff[0][..self.coeff_len[0]],
                &self.coeff[1][..self.coeff_len[1]],
                &self.coeff[2][..self.coeff_len[2]],
            ],
            sample: [
                &self.sample[0][..self.sample_len[0]],
                &self.sample[1][..self.sample_len[1]],
                &self.sample[2][..self.sample_len[2]],
            ],
            base: self.base,
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn pack_stack(
    dest: &mut StackPack,
    input: &[f32],
    abs_start: usize,
    row: &[f32],
) -> Result<(), Error> {
    if row.len() != 443 || input.len() > STACK_INPUT {
        return Err(Error::Overflow);
    }
    dest.coeff_len = [0; 3];
    dest.sample_len = [0; 3];
    dest.base = [0; 3];
    for (k, coeff_k) in row.iter().copied().enumerate() {
        let lane = k % 3;
        let at = dest.coeff_len[lane];
        if at >= COEFF_CAP {
            return Err(Error::Overflow);
        }
        dest.coeff[lane][at] = f64::from(coeff_k);
        dest.coeff_len[lane] = at + 1;
    }
    pack_samples(dest, input, abs_start)
}

/// Split `input` into the three residue rows. `f64::from` of an `f32` is exact.
/// A full stack window has at most [`SAMPLE_CAP`] entries in one row.
#[cfg(target_arch = "aarch64")]
fn pack_samples(dest: &mut StackPack, input: &[f32], abs_start: usize) -> Result<(), Error> {
    if abs_start.checked_add(input.len()).is_none() {
        return Err(Error::Overflow);
    }
    let mut len = [0_usize; 3];
    let mut base = [0_usize; 3];
    let mut filled = [false; 3];
    let mut k = 0;
    let mut residue = abs_start % 3;
    while k < input.len() && residue != 0 {
        store_sample(
            dest,
            input,
            abs_start,
            k,
            residue,
            &mut len,
            &mut base,
            &mut filled,
        )?;
        k += 1;
        residue += 1;
        if residue == 3 {
            residue = 0;
        }
    }
    let rest = match input.get(k..) {
        Some(rest) => rest,
        None => return Err(Error::Overflow),
    };
    let groups = rest.len() / 3;
    if groups > 0 {
        let abs0 = match abs_start.checked_add(k) {
            Some(abs0) => abs0,
            None => return Err(Error::Overflow),
        };
        if !filled[0] {
            base[0] = abs0 / 3;
            filled[0] = true;
        }
        if !filled[1] {
            base[1] = (abs0 + 1) / 3;
            filled[1] = true;
        }
        if !filled[2] {
            base[2] = (abs0 + 2) / 3;
            filled[2] = true;
        }
    }
    let mut n0 = len[0];
    let mut n1 = len[1];
    let mut n2 = len[2];
    let mut g = 0;
    // Twelve samples are four of each residue. `vld3q` splits them in order.
    while g + 4 <= groups {
        let at = g * 3;
        if n0 + 4 > SAMPLE_CAP || n1 + 4 > SAMPLE_CAP || n2 + 4 > SAMPLE_CAP {
            return Err(Error::Overflow);
        }
        // SAFETY: `at + 12` is inside `rest` because `g + 4 <= groups`.
        // The row cursors stay inside `SAMPLE_CAP`. NEON is baseline here.
        unsafe {
            use std::arch::aarch64::{
                vcvt_f64_f32, vget_high_f32, vget_low_f32, vld3q_f32, vst1q_f64,
            };
            let tri = vld3q_f32(rest.as_ptr().add(at));
            vst1q_f64(
                dest.sample[0].as_mut_ptr().add(n0),
                vcvt_f64_f32(vget_low_f32(tri.0)),
            );
            vst1q_f64(
                dest.sample[0].as_mut_ptr().add(n0 + 2),
                vcvt_f64_f32(vget_high_f32(tri.0)),
            );
            vst1q_f64(
                dest.sample[1].as_mut_ptr().add(n1),
                vcvt_f64_f32(vget_low_f32(tri.1)),
            );
            vst1q_f64(
                dest.sample[1].as_mut_ptr().add(n1 + 2),
                vcvt_f64_f32(vget_high_f32(tri.1)),
            );
            vst1q_f64(
                dest.sample[2].as_mut_ptr().add(n2),
                vcvt_f64_f32(vget_low_f32(tri.2)),
            );
            vst1q_f64(
                dest.sample[2].as_mut_ptr().add(n2 + 2),
                vcvt_f64_f32(vget_high_f32(tri.2)),
            );
        }
        n0 += 4;
        n1 += 4;
        n2 += 4;
        g += 4;
    }
    len[0] = n0;
    len[1] = n1;
    len[2] = n2;
    let mut k = k + g * 3;
    while k < input.len() {
        store_sample(
            dest,
            input,
            abs_start,
            k,
            residue,
            &mut len,
            &mut base,
            &mut filled,
        )?;
        k += 1;
        residue += 1;
        if residue == 3 {
            residue = 0;
        }
    }
    dest.sample_len = len;
    dest.base = base;
    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn store_sample(
    dest: &mut StackPack,
    input: &[f32],
    abs_start: usize,
    k: usize,
    residue: usize,
    len: &mut [usize; 3],
    base: &mut [usize; 3],
    filled: &mut [bool; 3],
) -> Result<(), Error> {
    let Some(sample_k) = input.get(k).copied() else {
        return Err(Error::Overflow);
    };
    let Some(abs) = abs_start.checked_add(k) else {
        return Err(Error::Overflow);
    };
    if !filled[residue] {
        base[residue] = abs / 3;
        filled[residue] = true;
    }
    let at = len[residue];
    if at >= SAMPLE_CAP {
        return Err(Error::Overflow);
    }
    dest.sample[residue][at] = f64::from(sample_k);
    len[residue] = at + 1;
    Ok(())
}

/// `input[0]` is absolute sample `abs_start`. Each residue records the first
/// absolute index it stored.
#[cfg(target_arch = "aarch64")]
fn pack_at(input: &[f32], abs_start: usize, row: &[f32]) -> Result<Packed48, Error> {
    let mut coeff: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    let coeff_room = row.len().div_ceil(3);
    for lane in &mut coeff {
        if lane.try_reserve_exact(coeff_room).is_err() {
            return Err(Error::Overflow);
        }
    }
    for (k, coeff_k) in row.iter().copied().enumerate() {
        coeff[k % 3].push(f64::from(coeff_k));
    }
    let mut sample: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    let sample_room = match input.len().checked_div(3).and_then(|n| n.checked_add(1)) {
        Some(room) => room,
        None => return Err(Error::Overflow),
    };
    for lane in &mut sample {
        if lane.try_reserve_exact(sample_room).is_err() {
            return Err(Error::Overflow);
        }
    }
    let mut base = [0_usize; 3];
    let mut seen = [false; 3];
    for (k, sample_k) in input.iter().copied().enumerate() {
        let abs = match abs_start.checked_add(k) {
            Some(abs) => abs,
            None => return Err(Error::Overflow),
        };
        let residue = abs % 3;
        if !seen[residue] {
            base[residue] = abs / 3;
            seen[residue] = true;
        }
        sample[residue].push(f64::from(sample_k));
    }
    Ok(Packed48 {
        coeff,
        sample,
        base,
    })
}

/// Exclusive bound on a group start `n` whose output `n + 7` still ends in `len`.
/// That last index is `3 * n - 421`.
#[cfg(target_arch = "aarch64")]
fn group_limit(len: usize) -> Result<usize, Error> {
    let len = match i128::try_from(len) {
        Ok(len) => len,
        Err(_) => return Err(Error::Overflow),
    };
    let room = match len.checked_add(420) {
        Some(room) => room,
        None => return Err(Error::Overflow),
    };
    match usize::try_from(room / 3 + 1) {
        Ok(limit) => Ok(limit),
        Err(_) => Ok(usize::MAX),
    }
}

#[cfg(target_arch = "aarch64")]
fn slot_48(n: usize, phase: usize) -> Option<(usize, usize)> {
    let center = match usize::try_from(CENTER_48) {
        Ok(center) => center,
        Err(_) => return None,
    };
    let index = n.checked_mul(3)?.checked_add(phase)?.checked_sub(center)?;
    Some((index % 3, index / 3))
}

/// Furthest `vld1q_f64` reads two `f64`s at packed offset `coeff_len + 6`.
#[cfg(target_arch = "aarch64")]
fn group_fits(packed: &View<'_>, n: usize) -> bool {
    for phase in 0..3 {
        let Some((residue, slot)) = slot_48(n, phase) else {
            return false;
        };
        let Some(local) = slot.checked_sub(packed.base[residue]) else {
            return false;
        };
        let Some(need) = local
            .checked_add(packed.coeff[phase].len())
            .and_then(|end| end.checked_add(7))
        else {
            return false;
        };
        if need > packed.sample[residue].len() {
            return false;
        }
    }
    true
}

/// Absolute input of packed index 0. Packed `q` is `origin + 3 * q`.
#[cfg(target_arch = "aarch64")]
fn phase_origin(n: usize, phase: usize) -> Option<usize> {
    let (residue, slot) = slot_48(n, phase)?;
    slot.checked_mul(3)?.checked_add(residue)
}

#[cfg(target_arch = "aarch64")]
fn widen_48(
    input: &[f32],
    row: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<usize, Error> {
    let fast_lo = first.max(FULL_48);
    let limit = group_limit(input.len())?;
    let Some(end) = fast_lo.checked_add(8) else {
        return Ok(first);
    };
    // One short group stays on the scalar dot. Input samples stay `f32`.
    if fast_lo >= limit || end > raw_len || limit - fast_lo < 8 {
        return Ok(first);
    }
    let Some(last_by_raw) = raw_len.checked_sub(8) else {
        return Ok(first);
    };
    let Some(last_by_limit) = limit.checked_sub(1) else {
        return Ok(first);
    };
    let last_cap = last_by_raw.min(last_by_limit);
    if last_cap < fast_lo {
        return Ok(first);
    }
    // One reused residue pack. Contiguous `f64` loads stay in the dot; the
    // strided `f32` gather did not. The pack is stack storage, same as a stream
    // chunk, so this convert does not allocate it.
    let mut packed = StackPack::new();
    let mut n = first;
    while n < fast_lo {
        push_48(samples, first, input, n, row)?;
        n += 1;
    }
    while n <= last_cap {
        let Some(origin) = phase_origin(n, 0) else {
            break;
        };
        if origin >= input.len() {
            break;
        }
        let Some(window_end) = origin.checked_add(STACK_INPUT) else {
            break;
        };
        let window_end = window_end.min(input.len());
        let Some(window) = input.get(origin..window_end) else {
            break;
        };
        if pack_stack(&mut packed, window, origin, row).is_err() {
            break;
        }
        let mut produced = false;
        while n <= last_cap {
            let Some(out) = eight_stack(&packed, n)? else {
                break;
            };
            let Some(at) = n.checked_sub(first) else {
                return Err(Error::Overflow);
            };
            let Some(at_end) = at.checked_add(out.len()) else {
                return Err(Error::Overflow);
            };
            let Some(dest) = samples.get_mut(at..at_end) else {
                return Err(Error::Overflow);
            };
            dest.copy_from_slice(&out);
            let Some(next) = n.checked_add(out.len()) else {
                return Err(Error::Overflow);
            };
            n = next;
            produced = true;
        }
        if !produced {
            break;
        }
    }
    Ok(n)
}

/// Eight outputs from raw index `n`. Phase 0, then 1, then 2. Tap `q` in a
/// phase adds into partial `q % 4`. Phases 0 and 1 are multiples of four, so
/// each phase starts on partial 0. The partials fold `p0 + p1 + p2 + p3`.
///
/// # Safety
///
/// [`group_fits`] is true for `n`. NEON is available. A nonzero residue base
/// only slides the load; the multiply-then-add order is unchanged.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn eight_outputs(packed: &View<'_>, n: usize) -> [f64; 8] {
    use std::arch::aarch64::{
        vaddq_f64, vdupq_n_f64, vgetq_lane_f64, vld1q_dup_f64, vld1q_f64, vmulq_f64,
    };
    debug_assert_eq!(packed.coeff[0].len() % 4, 0);
    debug_assert_eq!(packed.coeff[1].len() % 4, 0);
    unsafe {
        let mut p0a = vdupq_n_f64(0.0);
        let mut p0b = vdupq_n_f64(0.0);
        let mut p0c = vdupq_n_f64(0.0);
        let mut p0d = vdupq_n_f64(0.0);
        let mut p1a = vdupq_n_f64(0.0);
        let mut p1b = vdupq_n_f64(0.0);
        let mut p1c = vdupq_n_f64(0.0);
        let mut p1d = vdupq_n_f64(0.0);
        let mut p2a = vdupq_n_f64(0.0);
        let mut p2b = vdupq_n_f64(0.0);
        let mut p2c = vdupq_n_f64(0.0);
        let mut p2d = vdupq_n_f64(0.0);
        let mut p3a = vdupq_n_f64(0.0);
        let mut p3b = vdupq_n_f64(0.0);
        let mut p3c = vdupq_n_f64(0.0);
        let mut p3d = vdupq_n_f64(0.0);
        for phase in 0..3 {
            let coeffs = &packed.coeff[phase];
            let Some((residue, slot)) = slot_48(n, phase) else {
                break;
            };
            // `group_fits` kept `slot` at or after this residue's stored base.
            let Some(local) = slot.checked_sub(packed.base[residue]) else {
                break;
            };
            let samples = packed.sample[residue].as_ptr().add(local);
            let mut q = 0_usize;
            let n4 = coeffs.len() / 4 * 4;
            while q < n4 {
                let c0 = vld1q_dup_f64(coeffs.as_ptr().add(q));
                let c1 = vld1q_dup_f64(coeffs.as_ptr().add(q + 1));
                let c2 = vld1q_dup_f64(coeffs.as_ptr().add(q + 2));
                let c3 = vld1q_dup_f64(coeffs.as_ptr().add(q + 3));
                p0a = vaddq_f64(p0a, vmulq_f64(vld1q_f64(samples.add(q)), c0));
                p0b = vaddq_f64(p0b, vmulq_f64(vld1q_f64(samples.add(q + 2)), c0));
                p0c = vaddq_f64(p0c, vmulq_f64(vld1q_f64(samples.add(q + 4)), c0));
                p0d = vaddq_f64(p0d, vmulq_f64(vld1q_f64(samples.add(q + 6)), c0));
                p1a = vaddq_f64(p1a, vmulq_f64(vld1q_f64(samples.add(q + 1)), c1));
                p1b = vaddq_f64(p1b, vmulq_f64(vld1q_f64(samples.add(q + 3)), c1));
                p1c = vaddq_f64(p1c, vmulq_f64(vld1q_f64(samples.add(q + 5)), c1));
                p1d = vaddq_f64(p1d, vmulq_f64(vld1q_f64(samples.add(q + 7)), c1));
                p2a = vaddq_f64(p2a, vmulq_f64(vld1q_f64(samples.add(q + 2)), c2));
                p2b = vaddq_f64(p2b, vmulq_f64(vld1q_f64(samples.add(q + 4)), c2));
                p2c = vaddq_f64(p2c, vmulq_f64(vld1q_f64(samples.add(q + 6)), c2));
                p2d = vaddq_f64(p2d, vmulq_f64(vld1q_f64(samples.add(q + 8)), c2));
                p3a = vaddq_f64(p3a, vmulq_f64(vld1q_f64(samples.add(q + 3)), c3));
                p3b = vaddq_f64(p3b, vmulq_f64(vld1q_f64(samples.add(q + 5)), c3));
                p3c = vaddq_f64(p3c, vmulq_f64(vld1q_f64(samples.add(q + 7)), c3));
                p3d = vaddq_f64(p3d, vmulq_f64(vld1q_f64(samples.add(q + 9)), c3));
                q += 4;
            }
            while q < coeffs.len() {
                let c = vld1q_dup_f64(coeffs.as_ptr().add(q));
                let (a, b, creg, d) = match q & 3 {
                    0 => (&mut p0a, &mut p0b, &mut p0c, &mut p0d),
                    1 => (&mut p1a, &mut p1b, &mut p1c, &mut p1d),
                    2 => (&mut p2a, &mut p2b, &mut p2c, &mut p2d),
                    _ => (&mut p3a, &mut p3b, &mut p3c, &mut p3d),
                };
                *a = vaddq_f64(*a, vmulq_f64(vld1q_f64(samples.add(q)), c));
                *b = vaddq_f64(*b, vmulq_f64(vld1q_f64(samples.add(q + 2)), c));
                *creg = vaddq_f64(*creg, vmulq_f64(vld1q_f64(samples.add(q + 4)), c));
                *d = vaddq_f64(*d, vmulq_f64(vld1q_f64(samples.add(q + 6)), c));
                q += 1;
            }
        }
        let fold = |a, b, c, d| {
            let acc = vaddq_f64(a, b);
            let acc = vaddq_f64(acc, c);
            vaddq_f64(acc, d)
        };
        let o0 = fold(p0a, p1a, p2a, p3a);
        let o1 = fold(p0b, p1b, p2b, p3b);
        let o2 = fold(p0c, p1c, p2c, p3c);
        let o3 = fold(p0d, p1d, p2d, p3d);
        [
            vgetq_lane_f64(o0, 0),
            vgetq_lane_f64(o0, 1),
            vgetq_lane_f64(o1, 0),
            vgetq_lane_f64(o1, 1),
            vgetq_lane_f64(o2, 0),
            vgetq_lane_f64(o2, 1),
            vgetq_lane_f64(o3, 0),
            vgetq_lane_f64(o3, 1),
        ]
    }
}

/// One packed chunk of a 48 kHz to 16 kHz stream. `input[0]` is absolute
/// sample `abs_start`. [`Block48::eight`] is `None` when eight residue-class
/// dots do not fit, and on targets without this kernel.
pub(crate) struct Block48 {
    #[cfg(target_arch = "aarch64")]
    packed: Packed48,
}

pub(crate) fn block_48(input: &[f32], abs_start: usize, row: &[f32]) -> Result<Block48, Error> {
    #[cfg(target_arch = "aarch64")]
    {
        if row.len() != 443 {
            return Err(Error::Overflow);
        }
        return Ok(Block48 {
            packed: pack_at(input, abs_start, row)?,
        });
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (input, abs_start, row);
        Ok(Block48 {})
    }
}

#[cfg(target_arch = "aarch64")]
fn eight_view(view: &View<'_>, n: usize) -> Result<Option<[f32; 8]>, Error> {
    if !group_fits(view, n) {
        return Ok(None);
    }
    // SAFETY: `group_fits` kept every phase load inside that residue.
    // NEON is baseline on aarch64. Products are `vmul` then `vadd`.
    let lanes = unsafe { eight_outputs(view, n) };
    let mut out = [0.0_f32; 8];
    for (lane, acc) in lanes.into_iter().enumerate() {
        out[lane] = super::finite_sample(acc)?;
    }
    Ok(Some(out))
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn eight_stack(packed: &StackPack, n: usize) -> Result<Option<[f32; 8]>, Error> {
    eight_view(&packed.view(), n)
}

impl Block48 {
    /// Eight outputs from raw index `n`, or `None` when the windows leave
    /// this chunk.
    pub(crate) fn eight(&self, n: usize) -> Result<Option<[f32; 8]>, Error> {
        #[cfg(target_arch = "aarch64")]
        {
            return eight_view(&self.packed.view(), n);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (self, n);
            Ok(None)
        }
    }
}

fn fill_up(
    input: &[f32],
    row_even: &[f32],
    row_odd: &[f32],
    raw_len: usize,
    engine: Engine,
    samples: &mut Vec<f32>,
    first: usize,
) -> Result<(), Error> {
    // Eight outputs cover four input origins. `four_outputs` keeps each dot
    // left to right. A window that leaves the buffer, or an odd cursor, stays
    // on `push`, which is the short-buffer tail.
    let taps = row_even.len();
    let center = up_center(taps);
    let width = taps.checked_add(3);
    let paired = row_odd.len() == taps;
    let mut n = first;
    while n < raw_len {
        if paired
            && n.is_multiple_of(2)
            && raw_len - n >= 8
            && let Some(width) = width
        {
            let origin = (n as i128 - center).div_euclid(2);
            if let Ok(start) = usize::try_from(origin)
                && let Some(end) = start.checked_add(width)
                && let Some(window) = input.get(start..end)
            {
                // 33-tap phase 0 is an exact unit impulse. The accumulator
                // starts at +0, so the center tap flushes -0 to +0. Later
                // products with +0 do not change a finite sum.
                let evens = if taps == crate::coefficients::TAPS_8_16 as usize {
                    let tap = (taps - 1) / 2;
                    let mut evens = [0.0_f32; 4];
                    for (lane, out) in evens.iter_mut().enumerate() {
                        let mut acc = 0.0_f64;
                        acc += f64::from(window[lane + tap]);
                        *out = super::finite_sample(acc)?;
                    }
                    evens
                } else {
                    four_outputs(window, row_even, 1)?
                };
                let odds = if taps == crate::coefficients::TAPS_8_16 as usize {
                    four_halfband(window, row_odd)?
                } else {
                    four_outputs(window, row_odd, 1)?
                };
                samples.extend_from_slice(&[
                    evens[0], odds[0], evens[1], odds[1], evens[2], odds[2], evens[3], odds[3],
                ]);
                n += 8;
                continue;
            }
        }
        // `origin = div_euclid(n - center, 2)`. Odd `n` has remainder 2 and
        // `den` 4, so the phase is `phases / 2`. The fraction is 0.
        let origin = (n as i128 - center).div_euclid(2);
        let row = if n.is_multiple_of(2) {
            row_even
        } else {
            row_odd
        };
        push(samples, input, origin, row, engine)?;
        n += 1;
    }
    Ok(())
}

fn push(
    samples: &mut Vec<f32>,
    input: &[f32],
    origin: i128,
    row: &[f32],
    engine: Engine,
) -> Result<(), Error> {
    let origin = match i64::try_from(origin) {
        Ok(origin) => origin,
        Err(_) => return Err(Error::Overflow),
    };
    let acc = dot(input, origin, row, engine);
    samples.push(super::finite_sample(acc)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{dot, dot_48, finite_sample};
    use super::{CENTER, Engine, Kind, block_48, fill, fill_up, four_outputs, push, up_center};
    use crate::coefficients::{TAPS, TAPS_8_16, Table};
    use crate::{Error, Rate};

    fn probe(index: usize) -> f32 {
        if index == 10 {
            return -0.0;
        }
        if index == 11 {
            return f32::from_bits(0x0080_0001);
        }
        ((std::f64::consts::TAU * 1_000.0 * index as f64 / 48_000.0).sin() as f32) * 0.5
    }

    fn assert_block_lanes(input: &[f32], row: &[f32], n: usize, got: &[f32; 8]) {
        for lane in 0..8 {
            let cursor = match i64::try_from(n + lane) {
                Ok(cursor) => cursor,
                Err(err) => panic!("lane {lane} cursor: {err}"),
            };
            let origin = match cursor
                .checked_mul(3)
                .and_then(|scaled| scaled.checked_sub(884))
            {
                Some(origin) => origin,
                None => panic!("lane {lane} origin overflow"),
            };
            let acc = dot_48(origin, row, |index| {
                if index < 0 {
                    return None;
                }
                match usize::try_from(index) {
                    Ok(index) => input.get(index).copied(),
                    Err(_) => None,
                }
            });
            let sample = match finite_sample(acc) {
                Ok(sample) => sample,
                Err(err) => panic!("lane {lane} sample: {err:?}"),
            };
            assert_eq!(got[lane].to_bits(), sample.to_bits(), "lane {lane} at {n}");
        }
    }

    #[test]
    fn block_48_matches_scalar_dots_at_zero_ulp() {
        let from = match Rate::new(48_000) {
            Ok(rate) => rate,
            Err(err) => panic!("48 kHz rate: {err:?}"),
        };
        let to = match Rate::new(16_000) {
            Ok(rate) => rate,
            Err(err) => panic!("16 kHz rate: {err:?}"),
        };
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        let input: Vec<f32> = (0..2_000).map(probe).collect();
        let full = match block_48(&input, 0, row) {
            Ok(block) => block,
            Err(err) => panic!("block: {err:?}"),
        };
        match full.eight(295) {
            Ok(Some(got)) => assert_block_lanes(&input, row, 295, &got),
            Ok(None) => {
                assert!(
                    !cfg!(target_arch = "aarch64"),
                    "full buffer group did not fit"
                );
            }
            Err(err) => panic!("eight: {err:?}"),
        }
        let abs = 901_usize;
        let shifted = match block_48(&input[abs..], abs, row) {
            Ok(block) => block,
            Err(err) => panic!("shifted block: {err:?}"),
        };
        match shifted.eight(600) {
            Ok(Some(got)) => assert_block_lanes(&input, row, 600, &got),
            Ok(None) => {
                assert!(!cfg!(target_arch = "aarch64"), "shifted group did not fit");
            }
            Err(err) => panic!("shifted eight: {err:?}"),
        }
        match shifted.eight(900) {
            Ok(None) => {}
            Ok(Some(_)) => panic!("a window past the chunk fitted"),
            Err(err) => panic!("past eight: {err:?}"),
        }
    }

    #[test]
    fn down_32_matches_the_scalar_dot_at_zero_ulp() {
        let from = match Rate::new(32_000) {
            Ok(rate) => rate,
            Err(err) => panic!("32 kHz rate: {err:?}"),
        };
        let to = match Rate::new(16_000) {
            Ok(rate) => rate,
            Err(err) => panic!("16 kHz rate: {err:?}"),
        };
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        let input: Vec<f32> = (0..4_000).map(probe).collect();
        let raw_len = 2_000 + 169;
        let first = 169_usize;
        #[cfg(target_arch = "aarch64")]
        {
            let mut packed = vec![0.0_f32; raw_len - first];
            let (lo, hi) = match super::rate32::widen(&input, row, raw_len, &mut packed, first) {
                Ok(span) => span,
                Err(err) => panic!("widen: {err:?}"),
            };
            assert!(hi > lo, "packed 32 kHz kernel wrote no outputs");
            for n in lo..hi {
                let origin = (n as i64) * 2 - 507;
                let expect = match finite_sample(dot(&input, origin, row, Engine::Scalar)) {
                    Ok(sample) => sample,
                    Err(err) => panic!("packed sample {n}: {err:?}"),
                };
                assert_eq!(
                    packed[n - first].to_bits(),
                    expect.to_bits(),
                    "packed output {n}"
                );
            }
        }
        let produced = match fill(
            &input,
            &table,
            raw_len,
            Kind::Down(2),
            Engine::Scalar,
            first,
        ) {
            Ok(samples) => samples,
            Err(err) => panic!("fill: {err:?}"),
        };
        assert_eq!(produced.len(), raw_len - first);
        for (offset, sample) in produced.iter().enumerate() {
            let n = first + offset;
            let origin = (n as i64) * 2 - 507;
            let expect = match finite_sample(dot(&input, origin, row, Engine::Scalar)) {
                Ok(value) => value,
                Err(err) => panic!("sample {n}: {err:?}"),
            };
            assert_eq!(sample.to_bits(), expect.to_bits(), "output {n}");
        }
    }

    #[test]
    fn down_96_and_192_match_the_scalar_dot_at_zero_ulp() {
        // `probe` puts −0.0 at index 10 and a subnormal at index 11.
        let input: Vec<f32> = (0..6_144).map(probe).collect();
        for (hz, step, delay) in [(96_000, 6_u64, 182_usize), (192_000, 12, 195_usize)] {
            let ideal = 6_144 / step as usize;
            let raw_len = ideal + delay;
            let from = rate(hz);
            let to = rate(16_000);
            let table = match Table::new(from, to) {
                Ok(table) => table,
                Err(err) => panic!("{hz} table: {err:?}"),
            };
            let stage1 = match table.row(0) {
                Ok(row) => row,
                Err(err) => panic!("{hz} row: {err:?}"),
            };
            let stage2_owned = match super::cascade_down::stage2_table() {
                Ok(table) => table,
                Err(err) => panic!("{hz} stage 2: {err:?}"),
            };
            let stage2 = match stage2_owned.row(0) {
                Ok(row) => row,
                Err(err) => panic!("{hz} stage 2 row: {err:?}"),
            };
            let kind = match step {
                6 => super::cascade_down::Kind::Hz96,
                _ => super::cascade_down::Kind::Hz192,
            };
            assert_eq!(
                super::cascade_down::history(kind),
                if step == 6 { 1_067 } else { 2_133 },
                "{hz} history"
            );
            let ready = if step == 6 { 534 } else { 1_067 };
            assert_eq!(
                super::cascade_down::largest_ready(kind, ready),
                Ok(delay as i64),
                "{hz} ready"
            );
            assert_eq!(
                super::cascade_down::largest_ready(kind, ready - 1),
                Ok(delay as i64 - 1),
                "{hz} one short"
            );
            fn at(input: &[f32], index: i64) -> Option<f32> {
                if index < 0 {
                    return None;
                }
                let Ok(index) = usize::try_from(index) else {
                    return None;
                };
                input.get(index).copied()
            }
            let produced = match fill(&input, &table, raw_len, Kind::Down(step), Engine::Scalar, 0)
            {
                Ok(samples) => samples,
                Err(err) => panic!("{hz} fill: {err:?}"),
            };
            assert_eq!(produced.len(), raw_len);
            let mut expect = vec![0.0_f32; raw_len];
            if let Err(err) = super::cascade_down::write_outputs(
                kind,
                stage1,
                stage2,
                0,
                raw_len,
                &mut expect,
                |index| at(&input, index),
            ) {
                panic!("{hz} scalar: {err:?}");
            }
            for (n, (got, want)) in produced.iter().zip(expect.iter()).enumerate() {
                assert_eq!(got.to_bits(), want.to_bits(), "{hz} output {n}");
            }
            for n in [0, delay, raw_len / 2, raw_len - 1] {
                let one =
                    match super::cascade_down::reference_one(kind, stage1, stage2, n, |index| {
                        at(&input, index)
                    }) {
                        Ok(sample) => sample,
                        Err(err) => panic!("{hz} reference {n}: {err:?}"),
                    };
                assert_eq!(produced[n].to_bits(), one.to_bits(), "{hz} reference {n}");
            }
            let mut impulse = vec![0.0_f32; 6_144];
            impulse[0] = 1.0;
            let raw = match fill(
                &impulse,
                &table,
                raw_len,
                Kind::Down(step),
                Engine::Scalar,
                0,
            ) {
                Ok(samples) => samples,
                Err(err) => panic!("{hz} impulse: {err:?}"),
            };
            let mut peak = 0_usize;
            for (index, sample) in raw.iter().enumerate() {
                if sample.abs() > raw[peak].abs() {
                    peak = index;
                }
            }
            assert_eq!(peak, delay, "{hz} impulse peak");
        }
    }

    #[test]
    fn origin_wider_than_i64_is_overflow() {
        let n = (i64::MAX as u64 + CENTER as u64) / 2 + 1;
        let wide = i128::from(n) * 2 - CENTER;
        let mut samples = Vec::new();
        assert_eq!(
            push(&mut samples, &[], wide, &[], Engine::Scalar),
            Err(Error::Overflow)
        );
        assert!(samples.is_empty());
    }

    #[test]
    fn up_center_keeps_the_373_tap_constant() {
        assert_eq!(up_center(crate::coefficients::TAPS as usize), CENTER);
        assert_eq!(CENTER, (crate::coefficients::TAPS as i128 - 1) / 2 * 3);
    }

    #[test]
    fn short_table_is_overflow() {
        assert_eq!(
            fill(&[], &Table::empty(), 1, Kind::Down(2), Engine::Scalar, 0),
            Err(Error::Overflow)
        );
        let phase0 = Table::empty();
        assert_eq!(
            fill(&[], &phase0, 1, Kind::Up, Engine::Scalar, 0),
            Err(Error::Overflow)
        );
    }

    fn rate(hz: i64) -> Rate {
        match Rate::new(hz) {
            Ok(rate) => rate,
            Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
        }
    }

    fn must_four(window: &[f32], row: &[f32]) -> [f32; 4] {
        match four_outputs(window, row, 1) {
            Ok(samples) => samples,
            Err(err) => panic!("four outputs: {err:?}"),
        }
    }

    /// Left-to-right product sum. Tap `offset` multiplies `window[lane + offset]`.
    fn left_to_right(window: &[f32], lane: usize, row: &[f32]) -> f64 {
        let mut acc = 0.0_f64;
        for (offset, coeff) in row.iter().enumerate() {
            acc += f64::from(window[lane + offset]) * f64::from(*coeff);
        }
        acc
    }

    fn assert_impulse_matches(window: &[f32], row: &[f32], impulse: usize) {
        let got = must_four(window, row);
        for (lane, sample) in got.iter().enumerate() {
            let full = left_to_right(window, lane, row);
            let mut short = 0.0_f64;
            short += f64::from(window[lane + impulse]);
            assert_eq!(
                short.to_bits(),
                full.to_bits(),
                "lane {lane} shortcut f64 {:x} dot {:x}",
                short.to_bits(),
                full.to_bits()
            );
            let rounded = match finite_sample(full) {
                Ok(sample) => sample,
                Err(err) => panic!("finite sample: {err:?}"),
            };
            assert_eq!(
                sample.to_bits(),
                rounded.to_bits(),
                "lane {lane} four_outputs does not match the dot"
            );
            let center = window[lane + impulse];
            if center.to_bits() == (-0.0_f32).to_bits() {
                assert_ne!(
                    center.to_bits(),
                    sample.to_bits(),
                    "direct copy of -0 is not the dot"
                );
                assert_eq!(sample.to_bits(), 0.0_f32.to_bits());
            }
            if center.to_bits() == 0x0080_0001 {
                assert_eq!(sample.to_bits(), center.to_bits());
            }
        }
    }

    #[test]
    fn even_8k_impulse_matches_the_left_to_right_dot() {
        let table = match Table::new(rate(8_000), rate(16_000)) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        let even = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("phase 0: {err:?}"),
        };
        let odd = match table.row(crate::coefficients::PHASES as usize / 2) {
            Ok(row) => row,
            Err(err) => panic!("phase 128: {err:?}"),
        };
        assert_eq!(even.len(), TAPS_8_16 as usize);
        assert_eq!(odd.len(), even.len());
        let mut impulse = None;
        for (index, coeff) in even.iter().enumerate() {
            if coeff.to_bits() == 0.0_f32.to_bits() {
                continue;
            }
            assert_eq!(coeff.to_bits(), 1.0_f32.to_bits(), "tap {index}");
            assert!(impulse.is_none(), "second nonzero tap {index}");
            impulse = Some(index);
        }
        let impulse = match impulse {
            Some(index) => index,
            None => panic!("phase 0 has no unit tap"),
        };
        assert_eq!(impulse, (even.len() - 1) / 2);
        let odd_is_filter = odd.iter().any(|coeff| {
            let bits = coeff.to_bits();
            bits != 0.0_f32.to_bits() && bits != 1.0_f32.to_bits()
        });
        assert!(
            odd_is_filter,
            "phase 128 is not a dot the impulse shortcut can skip"
        );

        let width = even.len() + 3;
        for index in 0..width {
            let mut window = vec![0.0_f32; width];
            window[index] = 1.0;
            let got = must_four(&window, even);
            for (lane, sample) in got.iter().enumerate() {
                let expect = if index == lane + impulse {
                    1.0_f32
                } else {
                    0.0
                };
                assert_eq!(
                    sample.to_bits(),
                    expect.to_bits(),
                    "window[{index}] lane {lane} is not coefficient {impulse}"
                );
            }
        }

        let centers: &[f32] = &[
            0.0,
            -0.0,
            f32::from_bits(0x0080_0001),
            f32::from_bits(0x8080_0001),
            f32::from_bits(1),
            f32::from_bits(0x8000_0001),
            1.0,
            -1.0,
            0.5,
            -0.5,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
        ];
        let mut backgrounds = vec![
            vec![0.0_f32; width],
            vec![-0.0_f32; width],
            vec![f32::from_bits(0x0080_0001); width],
            vec![f32::MAX; width],
            vec![f32::MIN; width],
            vec![f32::MIN_POSITIVE; width],
        ];
        let mut alternating = vec![0.0_f32; width];
        for (index, sample) in alternating.iter_mut().enumerate() {
            *sample = if index % 2 == 0 {
                -0.0
            } else {
                f32::from_bits(0x0080_0001)
            };
        }
        backgrounds.push(alternating);
        for window in &backgrounds {
            assert_impulse_matches(window, even, impulse);
            for lane in 0..4 {
                for center in centers {
                    let mut planted = window.clone();
                    planted[lane + impulse] = *center;
                    assert_impulse_matches(&planted, even, impulse);
                }
            }
        }

        let mut input = vec![0.0_f32; 40];
        for (index, sample) in input.iter_mut().enumerate() {
            *sample = if index % 2 == 0 {
                -0.0
            } else {
                f32::from_bits(0x0080_0001)
            };
        }
        input[16] = -0.0;
        input[17] = f32::from_bits(0x0080_0001);
        input[18] = f32::from_bits(0x8080_0001);
        input[19] = -1.0;
        input[20] = -0.0;
        input[21] = 0.25;
        input[22] = 0.0;
        input[23] = 1.0;
        let mut produced = Vec::new();
        match fill_up(&input, even, odd, 64, Engine::Scalar, &mut produced, 48) {
            Ok(()) => {}
            Err(err) => panic!("8 kHz fill_up: {err:?}"),
        }
        assert_eq!(produced.len(), 16);
        for (block, start) in [0_usize, 4].into_iter().enumerate() {
            let evens = must_four(&input[start..start + width], even);
            let odds = must_four(&input[start..start + width], odd);
            for lane in 0..4 {
                let at = block * 8 + lane * 2;
                assert_eq!(
                    produced[at].to_bits(),
                    evens[lane].to_bits(),
                    "block {block} even {lane}"
                );
                assert_eq!(
                    produced[at + 1].to_bits(),
                    odds[lane].to_bits(),
                    "block {block} odd {lane}"
                );
            }
        }
        assert_eq!(produced[0].to_bits(), 0.0_f32.to_bits());
        assert_ne!(input[16].to_bits(), produced[0].to_bits());
        assert_eq!(produced[2].to_bits(), f32::from_bits(0x0080_0001).to_bits());
    }

    #[test]
    fn wider_upsample_keeps_both_row_dots() {
        let taps = TAPS as usize;
        assert_ne!(taps, TAPS_8_16 as usize);
        let mut even = vec![0.0_f32; taps];
        let mut odd = vec![0.0_f32; taps];
        for (index, coeff) in even.iter_mut().enumerate() {
            *coeff = (index as f32) * 1.0e-4;
        }
        for (index, coeff) in odd.iter_mut().enumerate() {
            *coeff = -((index + 1) as f32) * 1.0e-4;
        }
        let mut input = vec![0.0_f32; taps + 3];
        for (index, sample) in input.iter_mut().enumerate() {
            *sample = (index as f32) * 0.01 - 0.5;
        }
        input[0] = -0.0;
        input[1] = f32::from_bits(0x0080_0001);
        let first = match usize::try_from(up_center(taps)) {
            Ok(first) => first,
            Err(_) => panic!("up center does not fit"),
        };
        let mut produced = Vec::new();
        match fill_up(
            &input,
            &even,
            &odd,
            first + 8,
            Engine::Scalar,
            &mut produced,
            first,
        ) {
            Ok(()) => {}
            Err(err) => panic!("wide fill_up: {err:?}"),
        }
        let evens = must_four(&input, &even);
        let odds = must_four(&input, &odd);
        assert_eq!(produced.len(), 8);
        for lane in 0..4 {
            assert_eq!(produced[lane * 2].to_bits(), evens[lane].to_bits());
            assert_eq!(produced[lane * 2 + 1].to_bits(), odds[lane].to_bits());
        }
    }
}
