//! Filterbank: windows + overlap-add — ISO/IEC 14496-3 §4.6.11.3.2 / §4.6.11.3.3.

use super::error::{Error, Result};
use super::ics::{IcsInfo, WindowSequence, WindowShape};
use super::imdct::imdct_into_f32;
use super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};
use std::sync::LazyLock;

#[path = "filterbank_grid.rs"]
mod grid;

const N_L: usize = 2048;
const N_S: usize = 256;
/// AAC-LD frame length in PCM samples (512-line transform).
const LD_FRAME_LEN: usize = super::swb::LD_WINDOW_LEN;

fn to_array<const N: usize>(v: Vec<f64>) -> [f32; N] {
    let mut a = [0.0f32; N];
    for (d, s) in a.iter_mut().zip(v) {
        *d = s as f32;
    }
    a
}

static SINE_LONG: LazyLock<[f32; 1024]> = LazyLock::new(|| to_array(sine_left(1024)));
static SINE_SHORT: LazyLock<[f32; 128]> = LazyLock::new(|| to_array(sine_left(128)));
static KBD_LONG: LazyLock<[f32; 1024]> = LazyLock::new(|| to_array(kbd_left(1024, 4.0)));
static KBD_SHORT: LazyLock<[f32; 128]> = LazyLock::new(|| to_array(kbd_left(128, 6.0)));
static SINE_LONG_REV: LazyLock<[f32; 1024]> = LazyLock::new(|| reverse(&SINE_LONG));
static KBD_LONG_REV: LazyLock<[f32; 1024]> = LazyLock::new(|| reverse(&KBD_LONG));

fn reverse(src: &[f32; 1024]) -> [f32; 1024] {
    let mut out = [0.0f32; 1024];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = src[1023 - i];
    }
    out
}

/// AAC-LD window halves (window length 1024, frame 512): sine slope, and the
/// low-overlap shape — 192 zeros, a 128-sample sine slope, 192 ones
/// (ISO/IEC 14496-3 LD windowing; same geometry in FDK and libxaac).
static SINE_LD: LazyLock<[f32; 512]> = LazyLock::new(|| to_array(sine_left(512)));
static LOW_OVERLAP_LD: LazyLock<[f32; 512]> = LazyLock::new(|| {
    let rise = sine_left(128);
    let mut out = [0.0f32; 512];
    for (d, s) in out[192..320].iter_mut().zip(rise) {
        *d = s as f32;
    }
    out[320..].fill(1.0);
    out
});

fn bessel_i0(x: f64) -> f64 {
    let half_x = x / 2.0;
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    let mut k = 1.0f64;
    loop {
        term *= (half_x / k) * (half_x / k);
        sum += term;
        if term <= sum * 1e-18 || k > 256.0 {
            break;
        }
        k += 1.0;
    }
    sum
}

pub(crate) fn kbd_left(half: usize, alpha: f64) -> Vec<f64> {
    let quarter = half as f64 / 2.0;
    let denom = bessel_i0(std::f64::consts::PI * alpha);
    let kernel: Vec<f64> = (0..=half)
        .map(|n| {
            let t = (n as f64 - quarter) / quarter;
            let rad = (1.0 - t * t).max(0.0);
            bessel_i0(std::f64::consts::PI * alpha * rad.sqrt()) / denom
        })
        .collect();
    let total: f64 = kernel.iter().sum();
    let mut running = 0.0f64;
    let mut out = Vec::with_capacity(half);
    for &w in kernel.iter().take(half) {
        running += w;
        out.push((running / total).sqrt());
    }
    out
}

pub(crate) fn sine_left(half: usize) -> Vec<f64> {
    let n = (2 * half) as f64;
    (0..half)
        .map(|i| (std::f64::consts::PI / n * (i as f64 + 0.5)).sin())
        .collect()
}

fn half_rev(shape: WindowShape) -> &'static [f32] {
    match shape {
        WindowShape::Sine => SINE_LONG_REV.as_slice(),
        WindowShape::Kbd => KBD_LONG_REV.as_slice(),
    }
}

fn half(n: usize, shape: WindowShape) -> &'static [f32] {
    match (n, shape) {
        (N_L, WindowShape::Sine) => SINE_LONG.as_slice(),
        (N_L, WindowShape::Kbd) => KBD_LONG.as_slice(),
        (N_S, WindowShape::Sine) => SINE_SHORT.as_slice(),
        (N_S, WindowShape::Kbd) => KBD_SHORT.as_slice(),
        _ => SINE_LONG.as_slice(),
    }
}

/// Window left half for the encoder's analysis side (mirrors [`half`]; the
/// right half of an OnlyLong window is this reversed).
pub(crate) fn window_left(n: usize, shape: WindowShape) -> &'static [f32] {
    half(n, shape)
}

/// Full 1024-point LD analysis window. Left half is [`half_ld`]; the right
/// half is that half reversed, matching synthesis in [`Filterbank::synthesize_ld_into`].
/// Encoder analysis window. `Kbd` is the low-overlap shape.
pub(crate) fn ld_analysis_window(shape: WindowShape) -> &'static [f32; 1024] {
    match shape {
        WindowShape::Sine => &LD_SINE_FULL,
        WindowShape::Kbd => &LD_LOW_OVERLAP_FULL,
    }
}

static LD_SINE_FULL: LazyLock<[f32; 1024]> = LazyLock::new(|| full_ld(&SINE_LD));
static LD_LOW_OVERLAP_FULL: LazyLock<[f32; 1024]> = LazyLock::new(|| full_ld(&LOW_OVERLAP_LD));

fn full_ld(half: &[f32; 512]) -> [f32; 1024] {
    let mut w = [0.0f32; 1024];
    for i in 0..512 {
        w[i] = half[i];
        w[512 + i] = half[511 - i];
    }
    w
}

/// LD window left half; on the LD path `Kbd` is the low-overlap shape.
fn half_ld(shape: WindowShape) -> &'static [f32; 512] {
    match shape {
        WindowShape::Sine => &SINE_LD,
        WindowShape::Kbd => &LOW_OVERLAP_LD,
    }
}

/// Per-channel overlap-add state.
#[derive(Clone, Debug)]
pub struct Filterbank {
    overlap: [f32; LONG_WINDOW_LEN],
    prev_shape: Option<WindowShape>,
    scratch: [f32; N_L],
}

impl Default for Filterbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Filterbank {
    pub(crate) fn checkpoint(&self, out: &mut Vec<u8>) {
        out.push(match self.prev_shape {
            None => 0,
            Some(WindowShape::Sine) => 1,
            Some(WindowShape::Kbd) => 2,
        });
        for sample in self.overlap {
            out.extend_from_slice(&sample.to_bits().to_le_bytes());
        }
    }
    pub(crate) fn restore_checkpoint(&mut self, bytes: &[u8]) -> Option<()> {
        if bytes.len() != 1 + LONG_WINDOW_LEN * 4 {
            return None;
        }
        self.prev_shape = match bytes[0] {
            0 => None,
            1 => Some(WindowShape::Sine),
            2 => Some(WindowShape::Kbd),
            _ => return None,
        };
        for (sample, bytes) in self.overlap.iter_mut().zip(bytes[1..].chunks_exact(4)) {
            *sample = f32::from_bits(u32::from_le_bytes(bytes.try_into().ok()?));
            if !sample.is_finite() {
                return None;
            }
        }
        Some(())
    }
    /// Windowed overlap saved for the next frame (filterbank scale).
    pub(crate) fn saved_overlap(&self) -> &[f32] {
        &self.overlap
    }

    /// Zero overlap, previous shape sine (lavc `use_kb_window[1]` starts at 0).
    #[must_use]
    pub fn new() -> Self {
        Self {
            overlap: [0.0; LONG_WINDOW_LEN],
            prev_shape: None,
            scratch: [0.0; N_L],
        }
    }

    /// Synthesize 1024 PCM samples from a 1024-bin window-major spectrum.
    #[cfg(test)]
    pub fn synthesize(&mut self, spec: &[f32], ics: &IcsInfo) -> Result<Vec<f32>> {
        let mut out = Vec::new();
        self.synthesize_into(spec, ics, &mut out)?;
        Ok(out)
    }

    /// Write one frame into `dst`.
    pub fn synthesize_into(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        dst: &mut Vec<f32>,
    ) -> Result<()> {
        if ics.core_len() == 960 {
            return grid::synthesize_960(self, spec, ics, dst);
        }
        if ics.ld {
            if ics.core_len() == 480 {
                return grid::synthesize_ld480(self, spec, ics, dst);
            }
            return self.synthesize_ld_into(spec, ics, dst);
        }
        if ics.window_sequence == WindowSequence::OnlyLong {
            return self.synthesize_only_long(spec, ics, dst);
        }
        self.windowed(spec, ics)?;
        let half_n = LONG_WINDOW_LEN;
        dst.resize(half_n, 0.0);
        let (left, right) = self.scratch.split_at(half_n);
        super::filterbank_simd::overlap_add(dst, &mut self.overlap, left, right);
        self.prev_shape = Some(ics.window_shape);
        Ok(())
    }

    /// OnlyLong folds the window multiply into the overlap-add. The product
    /// is rounded before the add, matching the two-step scalar path.
    fn synthesize_only_long(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        dst: &mut Vec<f32>,
    ) -> Result<()> {
        if spec.len() != LONG_WINDOW_LEN {
            return Err(Error::FilterbankInvalid);
        }
        imdct_into_f32(spec, &mut self.scratch);
        let right = ics.window_shape;
        let left = self.prev_shape.unwrap_or(WindowShape::Sine);
        dst.resize(LONG_WINDOW_LEN, 0.0);
        super::filterbank_simd::window_overlap(
            &self.scratch,
            &mut self.overlap,
            dst,
            half(N_L, left),
            half_rev(right),
        );
        self.prev_shape = Some(right);
        Ok(())
    }

    /// AAC-LD: one 512-line frame. The left half of the window carries the
    /// previous frame's shape, the right half this frame's (FDK startup uses
    /// the frame's own shape on both sides). Overlap state lives in the
    /// first 512 / 1024 entries of the LC-sized arrays.
    fn synthesize_ld_into(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        dst: &mut Vec<f32>,
    ) -> Result<()> {
        if spec.len() != LD_FRAME_LEN {
            return Err(Error::FilterbankInvalid);
        }
        let (scratch, overlap) = (&mut self.scratch, &mut self.overlap);
        imdct_into_f32(spec, &mut scratch[..2 * LD_FRAME_LEN]);
        let right = ics.window_shape;
        let wl = half_ld(self.prev_shape.unwrap_or(right));
        let wr = half_ld(right);
        for i in 0..LD_FRAME_LEN {
            scratch[i] *= wl[i];
            scratch[LD_FRAME_LEN + i] *= wr[LD_FRAME_LEN - 1 - i];
        }
        dst.resize(LD_FRAME_LEN, 0.0);
        for ((d, o), (&s, &next)) in dst.iter_mut().zip(overlap.iter_mut()).zip(
            scratch[..LD_FRAME_LEN]
                .iter()
                .zip(scratch[LD_FRAME_LEN..2 * LD_FRAME_LEN].iter()),
        ) {
            *d = s + *o;
            *o = next;
        }
        self.prev_shape = Some(ics.window_shape);
        Ok(())
    }

    fn windowed(&mut self, spec: &[f32], ics: &IcsInfo) -> Result<()> {
        let left = self.prev_shape.unwrap_or(WindowShape::Sine);
        let right = ics.window_shape;
        match ics.window_sequence {
            WindowSequence::LongStart | WindowSequence::LongStop => {
                self.long_windowed(spec, left, right, ics.window_sequence)
            }
            WindowSequence::EightShort => self.short_windowed(spec, left, right),
            // OnlyLong is synthesize_only_long. A second window here would
            // drift from that path.
            WindowSequence::OnlyLong => Err(Error::FilterbankInvalid),
        }
    }

    fn long_windowed(
        &mut self,
        spec: &[f32],
        left: WindowShape,
        right: WindowShape,
        seq: WindowSequence,
    ) -> Result<()> {
        if spec.len() != LONG_WINDOW_LEN {
            return Err(Error::FilterbankInvalid);
        }
        imdct_into_f32(spec, &mut self.scratch);
        apply_long_window(&mut self.scratch, left, right, seq);
        Ok(())
    }

    fn short_windowed(
        &mut self,
        spec: &[f32],
        left: WindowShape,
        right: WindowShape,
    ) -> Result<()> {
        if spec.len() != 8 * SHORT_WINDOW_LEN {
            return Err(Error::FilterbankInvalid);
        }
        self.scratch.fill(0.0);
        let start = (N_L - N_S) / 4;
        let hop = N_S / 2;
        let mut x = [0.0f32; N_S];
        for j in 0..8 {
            let coeffs = &spec[j * SHORT_WINDOW_LEN..(j + 1) * SHORT_WINDOW_LEN];
            imdct_into_f32(coeffs, &mut x);
            let this_left = if j == 0 { left } else { right };
            let lh = half(N_S, this_left);
            let rh = half(N_S, right);
            let base = start + j * hop;
            for n in 0..N_S / 2 {
                self.scratch[base + n] += x[n] * lh[n];
            }
            for n in 0..N_S / 2 {
                self.scratch[base + N_S / 2 + n] += x[N_S / 2 + n] * rh[N_S / 2 - 1 - n];
            }
        }
        Ok(())
    }
}

fn apply_long_window(z: &mut [f32], left: WindowShape, right: WindowShape, seq: WindowSequence) {
    let half_l = N_L / 2;
    let lh = half(N_L, left);
    let rh = half(N_L, right);
    let sh_l = half(N_S, left);
    let sh_r = half(N_S, right);
    match seq {
        WindowSequence::LongStart => {
            for i in 0..half_l {
                z[i] *= lh[i];
            }
            let b = (3 * N_L - N_S) / 4;
            for (m, i) in (0..N_S / 2).enumerate() {
                z[b + m] *= sh_r[N_S / 2 - 1 - i];
            }
            z[b + N_S / 2..].fill(0.0);
        }
        WindowSequence::LongStop => {
            let a = (N_L - N_S) / 4;
            z[..a].fill(0.0);
            for (m, &sv) in sh_l.iter().enumerate() {
                z[a + m] *= sv;
            }
            for i in 0..half_l {
                z[half_l + i] *= rh[half_l - 1 - i];
            }
        }
        WindowSequence::OnlyLong | WindowSequence::EightShort => {}
    }
}
