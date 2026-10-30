//! Short-window TNS (TASK-72). Child of `enc_tns` so it reuses Levinson /
//! quantize / analysis_filter. Decoder syntax: 1-bit n_filt, 4-bit length,
//! 3-bit order, max order 7 (Table 4.54 / 4.103).

use super::super::bits::BitWriter;
use super::super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};
use super::{
    EncTns, K_MIN, MAX_ORDER, PG_MIN, analysis_filter, autocorrelate, dequantize_coef, levinson,
    quantize_coef, step_up,
};

/// Table 4.103 without PQF, short windows — mirror of `tns.rs`.
const TNS_MAX_BANDS_SHORT: [u8; 12] = [9, 9, 10, 14, 14, 14, 14, 14, 14, 14, 14, 14];
const MAX_ORDER_SHORT: usize = 7;
const N_WIN: usize = 8;

/// Per-window short TNS (order 0 = `n_filt = 0` for that window).
#[derive(Debug, Clone, Copy)]
pub struct EncTnsShort {
    on: bool,
    order: [u8; N_WIN],
    length: [u8; N_WIN],
    coef: [[u8; MAX_ORDER_SHORT]; N_WIN],
}

impl EncTnsShort {
    pub fn off() -> Self {
        Self {
            on: false,
            order: [0; N_WIN],
            length: [0; N_WIN],
            coef: [[0; MAX_ORDER_SHORT]; N_WIN],
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Side-information bits including `tns_data_present`.
    pub fn bits(&self) -> usize {
        if !self.on {
            return 1;
        }
        let mut n = 1; // present
        for w in 0..N_WIN {
            n += 1; // n_filt
            if self.order[w] > 0 {
                n += 1 + 4 + 3 + 1 + 1 + 4 * self.order[w] as usize;
            }
        }
        n
    }

    pub fn emit(&self, w: &mut BitWriter) {
        w.write_bit(self.on);
        if !self.on {
            return;
        }
        for w_i in 0..N_WIN {
            if self.order[w_i] == 0 {
                w.write(0, 1);
                continue;
            }
            w.write(1, 1); // n_filt
            w.write_bit(true); // coef_res 4-bit
            w.write(u32::from(self.length[w_i]), 4);
            w.write(u32::from(self.order[w_i]), 3);
            w.write_bit(false); // direction up
            w.write_bit(false); // coef_compress
            for &c in self.coef[w_i].iter().take(self.order[w_i] as usize) {
                w.write(u32::from(c), 4);
            }
        }
    }

    pub fn window_on(&self, w: usize) -> bool {
        w < N_WIN && self.order[w] > 0
    }
}

/// LPC + gate one 128-bin short window. Filters `spec[base..base+128]` in place.
fn decide_window(
    spec: &mut [f32],
    offsets: &[u16],
    fs_index: u8,
    n_swb: usize,
) -> (u8, u8, [u8; MAX_ORDER_SHORT]) {
    let zero = (0u8, 0u8, [0u8; MAX_ORDER_SHORT]);
    let Some(&max_bands) = TNS_MAX_BANDS_SHORT.get(fs_index as usize) else {
        return zero;
    };
    let max_bands = n_swb.min(usize::from(max_bands));
    if max_bands == 0 {
        return zero;
    }
    let lo = usize::from(offsets[0]);
    let hi = usize::from(offsets[max_bands]).min(spec.len());
    if hi.saturating_sub(lo) <= MAX_ORDER_SHORT {
        return zero;
    }
    let r = autocorrelate(&spec[lo..hi]);
    if r[0] <= 0.0 {
        return zero;
    }
    let Some(k) = levinson(&r) else {
        return zero;
    };
    let Some(order) = (1..=MAX_ORDER_SHORT)
        .rev()
        .find(|&m| k[m - 1].abs() >= K_MIN)
    else {
        return zero;
    };
    let mut coef = [0u8; MAX_ORDER_SHORT];
    let mut kq = [0.0f32; MAX_ORDER];
    for i in 0..order {
        coef[i] = quantize_coef(k[i]);
        kq[i] = dequantize_coef(coef[i]);
    }
    let a = step_up(&kq[..order]);
    let mut filtered = [0.0f32; SHORT_WINDOW_LEN];
    let n = hi - lo;
    filtered[..n].copy_from_slice(&spec[lo..hi]);
    analysis_filter(&mut filtered[..n], &spec[lo..hi], &a[..order]);
    let (mut e_in, mut e_out) = (0.0f64, 0.0f64);
    for i in 0..n {
        e_in += f64::from(spec[lo + i]) * f64::from(spec[lo + i]);
        e_out += f64::from(filtered[i]) * f64::from(filtered[i]);
    }
    if e_in < PG_MIN * e_out {
        return zero;
    }
    spec[lo..hi].copy_from_slice(&filtered[..n]);
    // Decoder walks down from num_swb; one filter of length num_swb starts
    // at 0 and is clamped to tns_max_bands.
    (order as u8, n_swb as u8, coef)
}

/// Per-channel short TNS on a window-major 1024-bin spectrum.
pub fn decide_short(
    spec: &mut [f32; LONG_WINDOW_LEN],
    offsets: &[u16],
    fs_index: u8,
) -> EncTnsShort {
    let n_swb = offsets.len() - 1;
    let mut out = EncTnsShort::off();
    for w in 0..N_WIN {
        let base = w * SHORT_WINDOW_LEN;
        let (order, length, coef) = decide_window(
            &mut spec[base..base + SHORT_WINDOW_LEN],
            offsets,
            fs_index,
            n_swb,
        );
        out.order[w] = order;
        out.length[w] = length;
        out.coef[w] = coef;
        if order > 0 {
            out.on = true;
        }
    }
    out
}

impl EncTns {
    pub(crate) fn set_short(&mut self, s: EncTnsShort) {
        self.short = s;
        if s.is_on() {
            self.on = false; // long payload unused; present bit comes from short.emit
        }
    }

    pub(crate) fn short(&self) -> EncTnsShort {
        self.short
    }
}
