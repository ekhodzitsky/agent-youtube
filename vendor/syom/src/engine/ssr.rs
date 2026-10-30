//! AAC SSR — ISO/IEC 14496-3 gain control and the 4-band PQF filterbank.
//!
//! Each 256-bin band is inverse-transformed on its own (odd bands are
//! frequency-reversed first), windowed, multiplied by the gain function,
//! then overlap-added. An inverse polyphase quadrature filter joins the
//! four 256-sample bands into 1024 PCM samples. Band 0 is never gain
//! controlled. A gain step is `2^(alev/4)` (1.5 dB) and is interpolated
//! geometrically up to the `aloc` sample.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::filterbank::{kbd_left, sine_left};
use super::ics::{IcsInfo, WindowSequence, WindowShape};
use super::imdct::imdct_into_f32;
use std::sync::LazyLock;

pub(super) const BANDS: usize = 4;
pub(super) const BAND: usize = 256;
pub(super) const KK: usize = 12;
pub(super) const BUF: usize = 24;

/// `adjust_num` is 3 bits, so at most 7 changes per window.
const MAX_ADJ: usize = 7;

/// `(n_windows, first window uses 4-bit aloc, other aloc bits)`.
const GAIN_MODE: [[u8; 3]; 4] = [
    [1, 0, 5], // only long
    [2, 1, 2], // long start
    [8, 0, 2], // eight short
    [2, 1, 5], // long stop
];

fn half_win(n: usize, shape: WindowShape, alpha: f64) -> [f32; 256] {
    let src = match shape {
        WindowShape::Sine => sine_left(n),
        WindowShape::Kbd => kbd_left(n, alpha),
    };
    let mut out = [0.0f32; 256];
    for (d, s) in out.iter_mut().take(n).zip(src) {
        *d = s as f32;
    }
    out
}

static SINE256: LazyLock<[f32; 256]> = LazyLock::new(|| half_win(256, WindowShape::Sine, 4.0));
static KBD256: LazyLock<[f32; 256]> = LazyLock::new(|| half_win(256, WindowShape::Kbd, 4.0));
static SINE32: LazyLock<[f32; 256]> = LazyLock::new(|| half_win(32, WindowShape::Sine, 6.0));
static KBD32: LazyLock<[f32; 256]> = LazyLock::new(|| half_win(32, WindowShape::Kbd, 6.0));

fn win256(shape: WindowShape) -> &'static [f32; 256] {
    match shape {
        WindowShape::Sine => &SINE256,
        WindowShape::Kbd => &KBD256,
    }
}

fn win32(shape: WindowShape) -> &'static [f32; 256] {
    match shape {
        WindowShape::Sine => &SINE32,
        WindowShape::Kbd => &KBD32,
    }
}

/// One channel's `gain_control_data()`. Bands are stored 0..max_band-1,
/// which the bitstream numbers as bands 1..max_band (band 0 has no data).
#[derive(Clone, Debug)]
pub(crate) struct GainControl {
    pub max_band: u8,
    adjust_num: [[u8; 8]; 3],
    alev: [[[u8; MAX_ADJ]; 8]; 3],
    aloc: [[[u8; MAX_ADJ]; 8]; 3],
}

/// Per-channel SSR delay: four overlap halves, the PQF state, last gain.
#[derive(Clone, Debug)]
pub(crate) struct SsrChan {
    ready: bool,
    prev_shape: WindowShape,
    overlap: [[f32; BAND]; BANDS],
    pqf_mem: [[f32; BUF]; BANDS],
    /// Gain at the last sample of the previous frame, per band.
    prev_g: [f32; BANDS],
}

impl Default for SsrChan {
    fn default() -> Self {
        Self {
            ready: false,
            prev_shape: WindowShape::Sine,
            overlap: [[0.0; BAND]; BANDS],
            pqf_mem: [[0.0; BUF]; BANDS],
            prev_g: [1.0; BANDS],
        }
    }
}

impl SsrChan {
    pub(crate) fn synthesize(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        gain: Option<&GainControl>,
        dst: &mut Vec<f32>,
    ) -> Result<()> {
        if spec.len() < BANDS * BAND {
            return Err(Error::FilterbankInvalid);
        }
        // Eight short windows need a 512-sample overlap per band. That path
        // is refused until it can be checked; a long window still decodes.
        if ics.window_sequence.is_eight_short() {
            return Err(Error::Format("SSR eight-short"));
        }
        if !self.ready {
            *self = Self::default();
            self.ready = true;
        }
        let prev = self.prev_shape;
        let mut bands = [[0.0f32; BAND]; BANDS];
        for band in 0..BANDS {
            let mut freq = [0.0f32; BAND];
            freq.copy_from_slice(&spec[band * BAND..(band + 1) * BAND]);
            if band == 1 || band == 3 {
                freq.reverse();
            }
            let mut time = [0.0f32; BAND * 2];
            window_band(&freq, &mut time, ics, prev)?;
            apply_gain(
                &mut time,
                band,
                ics.window_sequence,
                gain,
                &mut self.prev_g[band],
            );
            for i in 0..BAND {
                bands[band][i] = self.overlap[band][i] + time[i];
                self.overlap[band][i] = time[BAND + i];
            }
        }
        dst.resize(BANDS * BAND, 0.0);
        super::ssr_pqf::ipqf(&mut self.pqf_mem, &bands, dst);
        self.prev_shape = ics.window_shape;
        Ok(())
    }
}

pub(crate) fn read_gain(br: &mut BitReader<'_>, seq: WindowSequence) -> Result<GainControl> {
    let mode = GAIN_MODE[seq as usize];
    let max_band = br.read(2)? as u8;
    if max_band > 3 {
        return Err(Error::Format("SSR max_band"));
    }
    let mut adjust_num = [[0u8; 8]; 3];
    let mut alev = [[[0u8; MAX_ADJ]; 8]; 3];
    let mut aloc = [[[0u8; MAX_ADJ]; 8]; 3];
    for bd in 0..usize::from(max_band) {
        for wd in 0..usize::from(mode[0]) {
            let n = br.read(3)? as u8;
            adjust_num[bd][wd] = n;
            for ad in 0..usize::from(n) {
                if ad >= MAX_ADJ {
                    return Err(Error::Format("SSR adjust_num"));
                }
                alev[bd][wd][ad] = br.read(4)? as u8;
                let bits = if wd == 0 && mode[1] == 1 { 4 } else { mode[2] };
                aloc[bd][wd][ad] = br.read(u32::from(bits))? as u8;
            }
        }
    }
    Ok(GainControl {
        max_band,
        adjust_num,
        alev,
        aloc,
    })
}

fn window_band(
    freq: &[f32; BAND],
    time: &mut [f32; BAND * 2],
    ics: &IcsInfo,
    prev: WindowShape,
) -> Result<()> {
    let seq = ics.window_sequence;
    let wl = win256(ics.window_shape);
    let wp = win256(prev);
    let sl = win32(ics.window_shape);
    let sp = win32(prev);
    match seq {
        WindowSequence::OnlyLong => {
            imdct_into_f32(freq, time);
            for i in 0..BAND {
                time[i] *= wp[i];
                time[BAND + i] *= wl[BAND - 1 - i];
            }
        }
        WindowSequence::LongStart => {
            imdct_into_f32(freq, time);
            let nflat = (BAND - 32) / 2;
            for i in 0..BAND {
                time[i] *= wp[i];
            }
            for i in 0..32 {
                time[BAND + nflat + i] *= sl[31 - i];
            }
            for slot in time.iter_mut().skip(BAND + nflat + 32) {
                *slot = 0.0;
            }
        }
        WindowSequence::LongStop => {
            imdct_into_f32(freq, time);
            let nflat = (BAND - 32) / 2;
            for slot in time.iter_mut().take(nflat) {
                *slot = 0.0;
            }
            for i in 0..32 {
                time[nflat + i] *= sp[i];
            }
            for i in 0..BAND {
                time[BAND + i] *= wl[BAND - 1 - i];
            }
        }
        WindowSequence::EightShort => return Err(Error::Format("SSR eight-short")),
    }
    Ok(())
}

fn apply_gain(
    time: &mut [f32; BAND * 2],
    band: usize,
    seq: WindowSequence,
    gain: Option<&GainControl>,
    prev: &mut f32,
) {
    if band == 0 {
        return;
    }
    let Some(gc) = gain else {
        return;
    };
    if band > usize::from(gc.max_band) {
        return;
    }
    let bd = band - 1;
    let mut fn_g = [1.0f32; BAND * 2];
    let mut g = *prev;
    let mut cursor = 0usize;
    let mode = GAIN_MODE[seq as usize];
    let (lens, steps) = region_lens(seq);
    for wd in 0..usize::from(mode[0]) {
        let len = lens[wd];
        let step = steps[wd];
        let n = usize::from(gc.adjust_num[bd][wd]).min(MAX_ADJ);
        let end = cursor + len;
        for ad in 0..n {
            let pos = cursor + usize::from(gc.aloc[bd][wd][ad]) * step;
            let pos = pos.clamp(cursor, end);
            let dest = 2.0f32.powf(f32::from(gc.alev[bd][wd][ad]) * 0.25);
            interp(&mut fn_g, cursor, pos, g, dest);
            cursor = pos;
            g = dest;
        }
        if cursor < end {
            for slot in fn_g.iter_mut().take(end).skip(cursor) {
                *slot = g;
            }
            cursor = end;
        }
    }
    for (sample, &m) in time.iter_mut().zip(fn_g.iter()) {
        *sample *= m;
    }
    *prev = g;
}

fn region_lens(seq: WindowSequence) -> ([usize; 8], [usize; 8]) {
    let mut lens = [0usize; 8];
    let mut steps = [1usize; 8];
    match seq {
        WindowSequence::OnlyLong => {
            lens[0] = 512;
            steps[0] = 16;
        }
        WindowSequence::LongStart => {
            lens[0] = 256;
            steps[0] = 16;
            lens[1] = 256;
            steps[1] = 64;
        }
        WindowSequence::LongStop => {
            lens[0] = 256;
            steps[0] = 16;
            lens[1] = 256;
            steps[1] = 8;
        }
        WindowSequence::EightShort => {
            for w in 0..8 {
                lens[w] = 64;
                steps[w] = 16;
            }
        }
    }
    (lens, steps)
}

fn interp(dst: &mut [f32], from: usize, to: usize, g0: f32, g1: f32) {
    if to <= from {
        return;
    }
    let n = (to - from) as f32;
    let l0 = g0.max(1.0e-12).ln();
    let l1 = g1.max(1.0e-12).ln();
    for (k, slot) in dst.iter_mut().enumerate().take(to).skip(from) {
        let t = (k - from) as f32 / n;
        *slot = (l0 + (l1 - l0) * t).exp();
    }
}
