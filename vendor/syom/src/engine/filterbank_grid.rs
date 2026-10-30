//! 960/120 LC and 480-sample AAC-LD synthesis.
//!
//! The 1024 and 512 paths stay in the parent module so their overlap-add
//! rounding does not move.

use super::super::error::{Error, Result};
use super::super::ics::{IcsInfo, WindowSequence, WindowShape};
use super::super::imdct::imdct_into_f32;
use std::sync::LazyLock;

const N960: usize = 960;
const N480: usize = 480;
const LONG_960: usize = 1920;
const SHORT_960: usize = 240;

static SINE_L960: LazyLock<[f32; N960]> = LazyLock::new(|| super::to_array(super::sine_left(N960)));
static KBD_L960: LazyLock<[f32; N960]> =
    LazyLock::new(|| super::to_array(super::kbd_left(N960, 4.0)));
static SINE_S120: LazyLock<[f32; 120]> = LazyLock::new(|| super::to_array(super::sine_left(120)));
static KBD_S120: LazyLock<[f32; 120]> =
    LazyLock::new(|| super::to_array(super::kbd_left(120, 6.0)));

static SINE_LD480: LazyLock<[f32; N480]> =
    LazyLock::new(|| super::to_array(super::sine_left(N480)));
/// 180 zeros, a 120-sample sine slope, 180 ones. Same ratios as the 512-line
/// low-overlap window (192 / 128 / 192).
static LOW_LD480: LazyLock<[f32; N480]> = LazyLock::new(|| {
    let rise = super::sine_left(120);
    let mut out = [0.0f32; N480];
    for (d, s) in out[180..300].iter_mut().zip(rise) {
        *d = s as f32;
    }
    out[300..].fill(1.0);
    out
});

fn half_long(shape: WindowShape) -> &'static [f32; N960] {
    match shape {
        WindowShape::Sine => &SINE_L960,
        WindowShape::Kbd => &KBD_L960,
    }
}

fn half_short(shape: WindowShape) -> &'static [f32; 120] {
    match shape {
        WindowShape::Sine => &SINE_S120,
        WindowShape::Kbd => &KBD_S120,
    }
}

fn half_ld(shape: WindowShape) -> &'static [f32; N480] {
    match shape {
        WindowShape::Sine => &SINE_LD480,
        WindowShape::Kbd => &LOW_LD480,
    }
}

pub(super) fn synthesize_960(
    fb: &mut super::Filterbank,
    spec: &[f32],
    ics: &IcsInfo,
    dst: &mut Vec<f32>,
) -> Result<()> {
    if spec.len() != N960 {
        return Err(Error::FilterbankInvalid);
    }
    let left = fb.prev_shape.unwrap_or(WindowShape::Sine);
    let right = ics.window_shape;
    match ics.window_sequence {
        WindowSequence::OnlyLong => {
            imdct_into_f32(spec, &mut fb.scratch[..LONG_960]);
            let wl = half_long(left);
            let wr = half_long(right);
            for i in 0..N960 {
                fb.scratch[i] *= wl[i];
                fb.scratch[N960 + i] *= wr[N960 - 1 - i];
            }
        }
        WindowSequence::LongStart | WindowSequence::LongStop => {
            imdct_into_f32(spec, &mut fb.scratch[..LONG_960]);
            apply_long(
                &mut fb.scratch[..LONG_960],
                left,
                right,
                ics.window_sequence,
            );
        }
        WindowSequence::EightShort => short_into(fb, spec, left, right)?,
    }
    overlap(fb, dst, N960, right);
    Ok(())
}

pub(super) fn synthesize_ld480(
    fb: &mut super::Filterbank,
    spec: &[f32],
    ics: &IcsInfo,
    dst: &mut Vec<f32>,
) -> Result<()> {
    if spec.len() != N480 {
        return Err(Error::FilterbankInvalid);
    }
    imdct_into_f32(spec, &mut fb.scratch[..2 * N480]);
    let right = ics.window_shape;
    // No previous frame: both halves use this frame's shape (same as 512).
    let wl = half_ld(fb.prev_shape.unwrap_or(right));
    let wr = half_ld(right);
    for i in 0..N480 {
        fb.scratch[i] *= wl[i];
        fb.scratch[N480 + i] *= wr[N480 - 1 - i];
    }
    overlap(fb, dst, N480, right);
    Ok(())
}

fn overlap(fb: &mut super::Filterbank, dst: &mut Vec<f32>, n: usize, right: WindowShape) {
    dst.resize(n, 0.0);
    let (left, rest) = fb.scratch.split_at(n);
    let right_half = &rest[..n];
    for (((d, o), &s), &next) in dst
        .iter_mut()
        .zip(fb.overlap[..n].iter_mut())
        .zip(left)
        .zip(right_half)
    {
        *d = s + *o;
        *o = next;
    }
    fb.prev_shape = Some(right);
}

fn apply_long(z: &mut [f32], left: WindowShape, right: WindowShape, seq: WindowSequence) {
    let lh = half_long(left);
    let rh = half_long(right);
    let sh_l = half_short(left);
    let sh_r = half_short(right);
    match seq {
        WindowSequence::LongStart => {
            for i in 0..N960 {
                z[i] *= lh[i];
            }
            let b = (3 * LONG_960 - SHORT_960) / 4;
            for i in 0..120 {
                z[b + i] *= sh_r[119 - i];
            }
            z[b + 120..].fill(0.0);
        }
        WindowSequence::LongStop => {
            let a = (LONG_960 - SHORT_960) / 4;
            z[..a].fill(0.0);
            for (m, &sv) in sh_l.iter().enumerate() {
                z[a + m] *= sv;
            }
            for i in 0..N960 {
                z[N960 + i] *= rh[N960 - 1 - i];
            }
        }
        WindowSequence::OnlyLong | WindowSequence::EightShort => {}
    }
}

fn short_into(
    fb: &mut super::Filterbank,
    spec: &[f32],
    left: WindowShape,
    right: WindowShape,
) -> Result<()> {
    fb.scratch[..LONG_960].fill(0.0);
    let start = (LONG_960 - SHORT_960) / 4;
    let hop = SHORT_960 / 2;
    let mut x = [0.0f32; SHORT_960];
    for j in 0..8 {
        let coeffs = &spec[j * 120..(j + 1) * 120];
        imdct_into_f32(coeffs, &mut x);
        let this_left = if j == 0 { left } else { right };
        let lh = half_short(this_left);
        let rh = half_short(right);
        let base = start + j * hop;
        for n in 0..120 {
            fb.scratch[base + n] += x[n] * lh[n];
            fb.scratch[base + 120 + n] += x[120 + n] * rh[119 - n];
        }
    }
    Ok(())
}
