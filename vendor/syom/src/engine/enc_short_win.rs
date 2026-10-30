//! Short-window analysis for [`super`] (line cap): the start / stop /
//! short KBD windows and the eight 256→128 MDCTs on the decoder's grid.

use super::super::enc_quant::MAX_GROUPS;
use super::super::filterbank::window_left;
use super::super::ics::WindowShape;
use super::super::mdct::mdct_into_f32;
use super::super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};

/// Block offset of the first short window (`(2048 − 256) / 4`) and the hop
/// between short windows — mirrors the decoder's `short_windowed`.
const SHORT_START: usize = (2 * LONG_WINDOW_LEN - 2 * SHORT_WINDOW_LEN) / 4;
const SHORT_HOP: usize = SHORT_WINDOW_LEN;

/// The encoder's analysis windows, pre-scaled by 32768 like the long
/// `LcEncoder::window` and matching the decoder's synthesis shapes exactly
/// (`filterbank::apply_long_window` / `short_windowed`).
pub struct ShortWindows {
    /// Long-start: long KBD left, flat 1.0, short KBD right, zeros.
    pub start: Box<[f32; 2 * LONG_WINDOW_LEN]>,
    /// Long-stop: zeros, short KBD left, flat 1.0, long KBD right.
    pub stop: Box<[f32; 2 * LONG_WINDOW_LEN]>,
    /// One 256-tap short KBD window.
    pub short: Box<[f32; 2 * SHORT_WINDOW_LEN]>,
}

impl ShortWindows {
    pub fn new() -> Self {
        let long = window_left(2 * LONG_WINDOW_LEN, WindowShape::Kbd);
        let short = window_left(2 * SHORT_WINDOW_LEN, WindowShape::Kbd);
        let mut start = crate::engine::heap::heap_array::<f32, { 2 * LONG_WINDOW_LEN }>(0.0);
        let mut stop = crate::engine::heap::heap_array::<f32, { 2 * LONG_WINDOW_LEN }>(0.0);
        for i in 0..LONG_WINDOW_LEN {
            start[i] = long[i] * 32768.0;
            stop[LONG_WINDOW_LEN + i] = long[LONG_WINDOW_LEN - 1 - i] * 32768.0;
        }
        let flat = SHORT_START; // 448 samples of flat 1.0 on each side
        for i in 0..flat {
            start[LONG_WINDOW_LEN + i] = 32768.0;
            stop[SHORT_START + SHORT_WINDOW_LEN + i] = 32768.0;
        }
        for m in 0..SHORT_WINDOW_LEN {
            // Descending short right half at 1472..1600 of the start window.
            start[LONG_WINDOW_LEN + flat + m] = short[SHORT_WINDOW_LEN - 1 - m] * 32768.0;
            // Ascending short left half at 448..576 of the stop window.
            stop[SHORT_START + m] = short[m] * 32768.0;
        }
        let mut sw = Box::new([0.0f32; 2 * SHORT_WINDOW_LEN]);
        for i in 0..SHORT_WINDOW_LEN {
            sw[i] = short[i] * 32768.0;
            sw[2 * SHORT_WINDOW_LEN - 1 - i] = short[i] * 32768.0;
        }
        Self {
            start,
            stop,
            short: sw,
        }
    }
}

/// Window-major short spectrum of one 2048-sample block: eight 256→128
/// MDCTs at offsets 448 + 128·w (mirrors the decoder's short IMDCT grid).
pub fn spectra(
    block: &[f32; 2 * LONG_WINDOW_LEN],
    windows: &ShortWindows,
    spec: &mut [f32; LONG_WINDOW_LEN],
) {
    let mut seg = [0.0f32; 2 * SHORT_WINDOW_LEN];
    for w in 0..MAX_GROUPS {
        let base = SHORT_START + w * SHORT_HOP;
        for (s, (&x, &win)) in seg.iter_mut().zip(
            block[base..base + 2 * SHORT_WINDOW_LEN]
                .iter()
                .zip(windows.short.iter()),
        ) {
            *s = x * win;
        }
        mdct_into_f32(
            &seg,
            &mut spec[w * SHORT_WINDOW_LEN..(w + 1) * SHORT_WINDOW_LEN],
        );
    }
}
