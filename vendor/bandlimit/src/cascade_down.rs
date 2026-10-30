//! 96 kHz and 192 kHz to 16 kHz as two left-to-right dots.
//!
//! Stage 1 is a short Kaiser, 96 kHz to 32 kHz or 192 kHz to 32 kHz. Stage 2
//! is the 339-tap 32 kHz to 16 kHz dot. Each stage adds tap `q` before tap
//! `q + 1`, with `f64::from(sample) * f64::from(coeff)` and no fused product.
//! Stage 1 is stored as `f32` before stage 2 reads it. Samples outside the
//! input are skipped. A negative stage-2 index is skipped.

use crate::coefficients::{STAGE_96_16, STAGE_192_16, TAPS_32_16, Table};
use crate::{Error, Rate};

const STAGE2_TAPS: i64 = TAPS_32_16 as i64;
const STAGE2_HALF: i64 = (STAGE2_TAPS - 1) / 2;
const STAGE2_CENTER: i64 = STAGE2_HALF * (2 + 1);
const OUT_CHUNK: usize = 1_024;
const SCALAR_BATCH: usize = 256;
const MID_CAP: usize = 4_096;
/// First 32 kHz output whose window starts at a non-negative input.
const STAGE2_FULL: usize = 254;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Hz96,
    Hz192,
}

impl Kind {
    const fn step1(self) -> i64 {
        match self {
            Self::Hz96 => 3,
            Self::Hz192 => 6,
        }
    }

    const fn taps1(self) -> i64 {
        match self {
            Self::Hz96 => STAGE_96_16 as i64,
            Self::Hz192 => STAGE_192_16 as i64,
        }
    }

    const fn half1(self) -> i64 {
        (self.taps1() - 1) / 2
    }

    fn origin1(self, index: i64) -> Result<i64, Error> {
        let step = self.step1();
        let center = self.half1().checked_mul(step + 1).ok_or(Error::Overflow)?;
        index
            .checked_mul(step)
            .and_then(|scaled| scaled.checked_sub(center))
            .ok_or(Error::Overflow)
    }
}

pub(crate) fn classify(in_rate: u64, out_rate: u64, taps: u32) -> Option<Kind> {
    if out_rate != 1 {
        return None;
    }
    if in_rate == 6 && taps == STAGE_96_16 {
        Some(Kind::Hz96)
    } else if in_rate == 12 && taps == STAGE_192_16 {
        Some(Kind::Hz192)
    } else {
        None
    }
}

/// Input samples one output window spans. The delay line reserves one more.
pub(crate) fn history(kind: Kind) -> usize {
    (TAPS_32_16 as usize - 1) * kind.step1() as usize + kind.taps1() as usize
}

pub(crate) fn first_input(kind: Kind, n: u64) -> Result<i64, Error> {
    let n = usize::try_from(n).map_err(|_| Error::Overflow)?;
    kind.origin1(origin2(n)?)
}

fn last_input(kind: Kind, n: u64) -> Result<i64, Error> {
    let n = usize::try_from(n).map_err(|_| Error::Overflow)?;
    let origin = kind.origin1(last_intermediate(n)?)?;
    origin.checked_add(kind.taps1() - 1).ok_or(Error::Overflow)
}

/// Largest raw output whose last input is strictly below `available`.
/// `-1` means none.
pub(crate) fn largest_ready(kind: Kind, available: i64) -> Result<i64, Error> {
    let last0 = last_input(kind, 0)?;
    let step = kind.step1().checked_mul(2).ok_or(Error::Overflow)?;
    if step <= 0 {
        return Err(Error::Overflow);
    }
    let room = available
        .checked_sub(last0)
        .and_then(|shifted| shifted.checked_sub(1))
        .ok_or(Error::Overflow)?;
    if room < 0 {
        return Ok(-1);
    }
    Ok(room / step)
}

pub(crate) fn stage2_table() -> Result<Table, Error> {
    let from = Rate::new(32_000).map_err(|_| Error::Overflow)?;
    let to = Rate::new(16_000).map_err(|_| Error::Overflow)?;
    Table::new(from, to)
}

pub(super) fn fill(
    kind: Kind,
    input: &[f32],
    stage1: &[f32],
    raw_len: usize,
    samples: &mut [f32],
    first: usize,
) -> Result<(), Error> {
    if stage1.len() != kind.taps1() as usize {
        return Err(Error::Overflow);
    }
    let stage2_owned = stage2_table()?;
    let stage2 = stage2_owned.row(0)?;
    if stage2.len() != TAPS_32_16 as usize {
        return Err(Error::Overflow);
    }
    let mut n = first;
    while n < raw_len {
        let end = n.saturating_add(OUT_CHUNK).min(raw_len);
        fill_chunk(kind, input, stage1, stage2, samples, first, n, end)?;
        n = end;
    }
    Ok(())
}

pub(crate) fn write_outputs(
    kind: Kind,
    stage1: &[f32],
    stage2: &[f32],
    n0: usize,
    count: usize,
    out: &mut [f32],
    mut at: impl FnMut(i64) -> Option<f32>,
) -> Result<(), Error> {
    if count > out.len()
        || stage1.len() != kind.taps1() as usize
        || stage2.len() != TAPS_32_16 as usize
    {
        return Err(Error::Overflow);
    }
    let mut done = 0_usize;
    while done < count {
        let take = (count - done).min(SCALAR_BATCH);
        let start = n0.checked_add(done).ok_or(Error::Overflow)?;
        write_batch(
            kind,
            stage1,
            stage2,
            start,
            take,
            &mut out[done..done + take],
            &mut at,
        )?;
        done += take;
    }
    Ok(())
}

/// One output, nested the same way as [`write_outputs`].
pub(crate) fn reference_one(
    kind: Kind,
    stage1: &[f32],
    stage2: &[f32],
    n: usize,
    mut at: impl FnMut(i64) -> Option<f32>,
) -> Result<f32, Error> {
    if stage1.len() != kind.taps1() as usize || stage2.len() != TAPS_32_16 as usize {
        return Err(Error::Overflow);
    }
    let origin = origin2(n)?;
    let mut acc = 0.0_f64;
    for (q, coeff) in stage2.iter().enumerate() {
        let q = i64::try_from(q).map_err(|_| Error::Overflow)?;
        let Some(intermediate) = origin.checked_add(q) else {
            return Err(Error::Overflow);
        };
        if intermediate < 0 {
            continue;
        }
        let mid =
            super::super::finite_sample(dot_at(kind.origin1(intermediate)?, stage1, &mut at)?)?;
        acc += f64::from(mid) * f64::from(*coeff);
    }
    super::super::finite_sample(acc)
}

fn fill_chunk(
    kind: Kind,
    input: &[f32],
    stage1: &[f32],
    stage2: &[f32],
    samples: &mut [f32],
    range_first: usize,
    start: usize,
    end: usize,
) -> Result<(), Error> {
    let count = end.checked_sub(start).ok_or(Error::Overflow)?;
    let out_lo = start.checked_sub(range_first).ok_or(Error::Overflow)?;
    let out_hi = end.checked_sub(range_first).ok_or(Error::Overflow)?;
    let Some((scalar_lo, i_hi)) = mid_range(start, count)? else {
        for n in start..end {
            store(samples, range_first, n, 0.0)?;
        }
        return Ok(());
    };
    let i_lo = scalar_lo.min(pack_base(start)?);
    let span = i_hi.checked_sub(i_lo).ok_or(Error::Overflow)?;
    if span > MID_CAP {
        let out = samples.get_mut(out_lo..out_hi).ok_or(Error::Overflow)?;
        return write_outputs(kind, stage1, stage2, start, count, out, |index| {
            slice_at(input, index)
        });
    }
    let mut mid = [0.0_f32; MID_CAP];
    fill_stage1(kind, input, stage1, i_lo, i_hi, &mut mid[..span])?;
    let (lo, hi) = {
        let out = samples.get_mut(out_lo..out_hi).ok_or(Error::Overflow)?;
        #[cfg(target_arch = "aarch64")]
        {
            super::rate32::widen_at(&mid[..span], stage2, end, out, start, i_lo)?
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (out, i_lo, stage2);
            (start, start)
        }
    };
    for n in start..end {
        if lo < hi && n >= lo && n < hi {
            continue;
        }
        let value = stage2_from_mid(stage2, n, &mid[..span], i_lo)?;
        store(samples, range_first, n, value)?;
    }
    Ok(())
}

fn fill_stage1(
    kind: Kind,
    input: &[f32],
    stage1: &[f32],
    i_lo: usize,
    i_hi: usize,
    mid: &mut [f32],
) -> Result<(), Error> {
    let (lo, hi) = {
        #[cfg(target_arch = "aarch64")]
        {
            match kind {
                Kind::Hz96 => super::rate_int::widen_span::<3, { STAGE_96_16 as usize }>(
                    input, stage1, i_hi, mid, i_lo,
                )?,
                Kind::Hz192 => super::rate_int::widen_span::<6, { STAGE_192_16 as usize }>(
                    input, stage1, i_hi, mid, i_lo,
                )?,
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            (i_lo, i_lo)
        }
    };
    for intermediate in i_lo..i_hi {
        if lo < hi && intermediate >= lo && intermediate < hi {
            continue;
        }
        let index = i64::try_from(intermediate).map_err(|_| Error::Overflow)?;
        let acc = dot_at(kind.origin1(index)?, stage1, |at| slice_at(input, at))?;
        let at = intermediate.checked_sub(i_lo).ok_or(Error::Overflow)?;
        let slot = mid.get_mut(at).ok_or(Error::Overflow)?;
        *slot = super::super::finite_sample(acc)?;
    }
    Ok(())
}

fn write_batch(
    kind: Kind,
    stage1: &[f32],
    stage2: &[f32],
    n0: usize,
    count: usize,
    out: &mut [f32],
    at: &mut impl FnMut(i64) -> Option<f32>,
) -> Result<(), Error> {
    let Some((i_lo, i_hi)) = mid_range(n0, count)? else {
        for slot in out.iter_mut().take(count) {
            *slot = 0.0;
        }
        return Ok(());
    };
    let span = i_hi.checked_sub(i_lo).ok_or(Error::Overflow)?;
    if span > MID_CAP {
        for lane in 0..count {
            let n = n0.checked_add(lane).ok_or(Error::Overflow)?;
            out[lane] = reference_one(kind, stage1, stage2, n, &mut *at)?;
        }
        return Ok(());
    }
    let mut mid = [0.0_f32; MID_CAP];
    for k in 0..span {
        let intermediate = i64::try_from(i_lo + k).map_err(|_| Error::Overflow)?;
        let acc = dot_at(kind.origin1(intermediate)?, stage1, &mut *at)?;
        mid[k] = super::super::finite_sample(acc)?;
    }
    for lane in 0..count {
        let n = n0.checked_add(lane).ok_or(Error::Overflow)?;
        out[lane] = stage2_from_mid(stage2, n, &mid[..span], i_lo)?;
    }
    Ok(())
}

fn stage2_from_mid(stage2: &[f32], n: usize, mid: &[f32], i_lo: usize) -> Result<f32, Error> {
    let origin = origin2(n)?;
    let mut acc = 0.0_f64;
    for (q, coeff) in stage2.iter().enumerate() {
        let q = i64::try_from(q).map_err(|_| Error::Overflow)?;
        let Some(intermediate) = origin.checked_add(q) else {
            return Err(Error::Overflow);
        };
        if intermediate < 0 {
            continue;
        }
        let intermediate = usize::try_from(intermediate).map_err(|_| Error::Overflow)?;
        if intermediate < i_lo {
            return Err(Error::Overflow);
        }
        let sample = match mid.get(intermediate - i_lo) {
            Some(sample) => *sample,
            None => return Err(Error::Overflow),
        };
        acc += f64::from(sample) * f64::from(*coeff);
    }
    super::super::finite_sample(acc)
}

fn dot_at(
    origin: i64,
    coeffs: &[f32],
    mut at: impl FnMut(i64) -> Option<f32>,
) -> Result<f64, Error> {
    let mut acc = 0.0_f64;
    let mut index = origin;
    for coeff in coeffs {
        if let Some(sample) = at(index) {
            acc += f64::from(sample) * f64::from(*coeff);
        }
        match index.checked_add(1) {
            Some(next) => index = next,
            None => break,
        }
    }
    Ok(acc)
}

/// `Some((lo, hi))` is the half-open intermediate span. `None` means every
/// stage-2 tap sits before the input.
fn mid_range(n0: usize, count: usize) -> Result<Option<(usize, usize)>, Error> {
    if count == 0 {
        return Ok(None);
    }
    let last = n0.checked_add(count - 1).ok_or(Error::Overflow)?;
    let i_last = last_intermediate(last)?;
    if i_last < 0 {
        return Ok(None);
    }
    let i_first = origin2(n0)?;
    let i_lo = if i_first > 0 {
        usize::try_from(i_first).map_err(|_| Error::Overflow)?
    } else {
        0
    };
    let i_hi = usize::try_from(i_last.checked_add(1).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Overflow)?;
    Ok(Some((i_lo, i_hi)))
}

fn origin2(n: usize) -> Result<i64, Error> {
    let n = i64::try_from(n).map_err(|_| Error::Overflow)?;
    n.checked_mul(2)
        .and_then(|scaled| scaled.checked_sub(STAGE2_CENTER))
        .ok_or(Error::Overflow)
}

fn last_intermediate(n: usize) -> Result<i64, Error> {
    let n = i64::try_from(n).map_err(|_| Error::Overflow)?;
    n.checked_mul(2)
        .and_then(|scaled| scaled.checked_sub(STAGE2_HALF))
        .ok_or(Error::Overflow)
}

/// Absolute index where the 32 kHz pack reloads. Clamps at 0.
fn pack_base(n: usize) -> Result<usize, Error> {
    let simd_n = n.max(STAGE2_FULL);
    let abs = simd_n.checked_mul(2).ok_or(Error::Overflow)?;
    Ok(abs.saturating_sub(508))
}

fn slice_at(input: &[f32], index: i64) -> Option<f32> {
    if index < 0 {
        return None;
    }
    let Ok(index) = usize::try_from(index) else {
        return None;
    };
    input.get(index).copied()
}

fn store(samples: &mut [f32], first: usize, n: usize, value: f32) -> Result<(), Error> {
    let at = n.checked_sub(first).ok_or(Error::Overflow)?;
    let slot = samples.get_mut(at).ok_or(Error::Overflow)?;
    *slot = value;
    Ok(())
}
