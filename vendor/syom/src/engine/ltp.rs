//! AAC LTP — ISO/IEC 14496-3 long term prediction.
//!
//! The delay line holds the previous reconstructed PCM and the filterbank
//! overlap (the half that has not been emitted yet), plus a zero tail so a
//! short lag reads silence instead of walking off the buffer. The predicted
//! time signal is windowed with the same shapes as synthesis and analysed
//! with the forward MDCT. TNS, when present, is applied in the encoder
//! direction before the estimate is added on the bands the frame marked.

use super::bits::BitReader;
use super::error::Result;
use super::filterbank::window_left;
use super::ics::{IcsInfo, WindowSequence, WindowShape};
use super::mdct::mdct_into_f32;
use super::tns::{self, TnsData};

/// Highest scalefactor band LTP may correct on a long window.
const MAX_LTP_SFB: usize = 40;
/// `frame_len * 4`: two emitted frames, one overlap half, one zero tail.
const STATE_LEN: usize = 4096;
const FRAME: usize = 1024;

const COEF: [f32; 8] = [
    0.570829, 0.696616, 0.813004, 0.911304, 0.984900, 1.067894, 1.194601, 1.369533,
];

/// Per-channel LTP delay line. Empty until the first LTP frame.
#[derive(Clone, Debug)]
pub(crate) struct LtpChan {
    /// Reconstructed samples. Index `2048 + i - lag` is the estimate input.
    state: Vec<f32>,
    prev_shape: WindowShape,
}

impl Default for LtpChan {
    fn default() -> Self {
        Self {
            state: Vec::new(),
            prev_shape: WindowShape::Sine,
        }
    }
}

impl LtpChan {
    fn ensure(&mut self) {
        if self.state.is_empty() {
            self.state.resize(STATE_LEN, 0.0);
            self.prev_shape = WindowShape::Sine;
        }
    }

    /// Add the LTP estimate. Eight-short frames do not use LTP.
    pub(crate) fn apply(
        &mut self,
        spec: &mut [f32],
        ics: &IcsInfo,
        fs_index: u8,
        tns: Option<&TnsData>,
    ) -> Result<()> {
        if !ics.ltp_present || ics.window_sequence.is_eight_short() {
            return Ok(());
        }
        self.ensure();
        let coef = COEF
            .get(usize::from(ics.ltp_coef_idx))
            .copied()
            .unwrap_or(0.0);
        let lag = usize::from(ics.ltp_lag);
        let mut time = [0.0f32; FRAME * 2];
        for (i, slot) in time.iter_mut().enumerate() {
            let idx = FRAME * 2 + i;
            let sample = idx
                .checked_sub(lag)
                .and_then(|p| self.state.get(p).copied())
                .unwrap_or(0.0);
            *slot = sample * coef;
        }
        let mut est = [0.0f32; FRAME];
        analyse(
            &time,
            &mut est,
            ics.window_sequence,
            ics.window_shape,
            self.prev_shape,
        );
        if let Some(t) = tns {
            tns::apply_ma(&mut est, t, ics, fs_index)?;
        }
        let offsets = ics.swb_offsets(fs_index)?;
        let nsfb = usize::from(ics.max_sfb)
            .min(MAX_LTP_SFB)
            .min(offsets.len().saturating_sub(1));
        for sfb in 0..nsfb {
            if (ics.ltp_used >> sfb) & 1 == 0 {
                continue;
            }
            let lo = offsets[sfb] as usize;
            let hi = offsets[sfb + 1] as usize;
            for k in lo..hi {
                if let (Some(dst), Some(&add)) = (spec.get_mut(k), est.get(k)) {
                    *dst += add;
                }
            }
        }
        Ok(())
    }

    /// Shift the delay line. `pcm` and `overlap` are filterbank-scale samples.
    pub(crate) fn update(&mut self, pcm: &[f32], overlap: &[f32], shape: WindowShape) {
        self.ensure();
        let n = FRAME.min(pcm.len()).min(overlap.len());
        for i in 0..n {
            self.state[i] = self.state[FRAME + i];
        }
        for i in 0..n {
            self.state[FRAME + i] = pcm[i];
            self.state[FRAME * 2 + i] = overlap[i];
        }
        self.prev_shape = shape;
    }
}

/// `ltp_data_present` and, when set, lag / coef / `ltp_long_used`.
pub(crate) fn read_present(br: &mut BitReader<'_>, max_sfb: u8) -> Result<(bool, u16, u8, u64)> {
    if !br.read_bit()? {
        return Ok((false, 0, 0, 0));
    }
    let (lag, coef, used) = read_body(br, max_sfb)?;
    Ok((true, lag, coef, used))
}

pub(crate) fn read_body(br: &mut BitReader<'_>, max_sfb: u8) -> Result<(u16, u8, u64)> {
    let lag = br.read(11)? as u16;
    let coef = br.read(3)? as u8;
    let n = usize::from(max_sfb).min(MAX_LTP_SFB);
    let mut used = 0u64;
    for sfb in 0..n {
        if br.read_bit()? {
            used |= 1u64 << sfb;
        }
    }
    Ok((lag, coef, used))
}

fn analyse(
    time: &[f32; FRAME * 2],
    spec: &mut [f32; FRAME],
    seq: WindowSequence,
    shape: WindowShape,
    prev: WindowShape,
) {
    let long = window_left(2048, shape);
    let long_prev = window_left(2048, prev);
    let short = window_left(256, shape);
    let short_prev = window_left(256, prev);
    let mut win = [0.0f32; FRAME * 2];
    let nflat = (FRAME - 128) / 2; // 448
    match seq {
        WindowSequence::OnlyLong => {
            for i in 0..FRAME {
                win[i] = time[i] * long_prev[i];
                win[FRAME + i] = time[FRAME + i] * long[FRAME - 1 - i];
            }
        }
        WindowSequence::LongStart => {
            for i in 0..FRAME {
                win[i] = time[i] * long_prev[i];
            }
            win[FRAME..FRAME + nflat].copy_from_slice(&time[FRAME..FRAME + nflat]);
            for i in 0..128 {
                win[FRAME + nflat + i] = time[FRAME + nflat + i] * short[127 - i];
            }
        }
        WindowSequence::LongStop => {
            for i in 0..128 {
                win[nflat + i] = time[nflat + i] * short_prev[i];
            }
            for i in 0..nflat {
                win[nflat + 128 + i] = time[nflat + 128 + i];
            }
            for i in 0..FRAME {
                win[FRAME + i] = time[FRAME + i] * long[FRAME - 1 - i];
            }
        }
        WindowSequence::EightShort => return,
    }
    mdct_into_f32(&win, spec);
}
