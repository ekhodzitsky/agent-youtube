//! TNS — ISO/IEC 14496-3 §4.6.9 Tables 4.54 / 4.102 / 4.103.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::ics::{IcsInfo, WindowSequence};

/// Table 4.103 without PQF, long windows, fs 0..=11.
const MAX_BANDS_LONG: [u8; 12] = [31, 31, 34, 40, 42, 51, 46, 46, 42, 42, 42, 39];
/// Table 4.103 without PQF, short windows.
const MAX_BANDS_SHORT: [u8; 12] = [9, 9, 10, 14, 14, 14, 14, 14, 14, 14, 14, 14];
/// ER AAC LD (512-line long windows), fs 0..=11.
const MAX_BANDS_LD: [u8; 12] = [31, 31, 31, 31, 32, 37, 31, 31, 31, 31, 31, 31];

/// Long-window order ceiling (Table 4.54).
pub const MAX_TNS_ORDER: usize = 12;
/// Long-window `n_filt` is 2 bits (0..=3).
pub const MAX_TNS_FILTERS: usize = 3;
/// Eight short windows, or one long window.
pub const MAX_TNS_WINDOWS: usize = 8;

/// One TNS filter (Table 4.54).
#[derive(Debug, Clone, Copy, Default)]
pub struct TnsFilter {
    /// Scalefactor bands covered, counted down from the previous top.
    pub length: u8,
    /// All-pole order (clamped later by TNS_MAX_ORDER).
    pub order: u8,
    /// `true` = downward (inc = −1).
    pub direction: bool,
    /// Shrinks `coef` field by one bit.
    pub coef_compress: bool,
    /// Unsigned wire coefficients (`order` live entries).
    pub coef: [u8; MAX_TNS_ORDER],
}

/// Per-window TNS payload.
#[derive(Debug, Clone, Copy, Default)]
pub struct TnsWindow {
    /// `coef_res`: false → 3-bit, true → 4-bit.
    pub coef_res: bool,
    /// Filters in wire order (top of spectrum first).
    pub n_filt: u8,
    pub filters: [TnsFilter; MAX_TNS_FILTERS],
}

/// Parsed `tns_data()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TnsData {
    /// One slot per transform window.
    pub n_windows: u8,
    pub windows: [TnsWindow; MAX_TNS_WINDOWS],
}

impl TnsData {
    /// Table 4.54.
    pub fn parse(br: &mut BitReader<'_>, ics: &IcsInfo) -> Result<Self> {
        let short = ics.window_sequence.is_eight_short();
        let n_filt_bits = if short { 1 } else { 2 };
        let len_bits = if short { 4 } else { 6 };
        let order_bits = if short { 3 } else { 5 };
        let n_windows = ics.num_windows.min(MAX_TNS_WINDOWS as u8);
        let mut windows = [TnsWindow::default(); MAX_TNS_WINDOWS];
        for win in windows.iter_mut().take(n_windows as usize) {
            let n_filt = (br.read(n_filt_bits)? as usize).min(MAX_TNS_FILTERS);
            let mut coef_res = false;
            if n_filt > 0 {
                coef_res = br.read_bit()?;
            }
            let mut filters = [TnsFilter::default(); MAX_TNS_FILTERS];
            for filt in filters.iter_mut().take(n_filt) {
                let length = br.read(len_bits)? as u8;
                let order = br.read(order_bits)? as u8;
                let mut direction = false;
                let mut coef_compress = false;
                let mut coef = [0u8; MAX_TNS_ORDER];
                if order > 0 {
                    direction = br.read_bit()?;
                    coef_compress = br.read_bit()?;
                    let coef_res_bits = if coef_res { 4 } else { 3 };
                    let coef_bits = coef_res_bits - u32::from(coef_compress);
                    let n_coef = (order as usize).min(MAX_TNS_ORDER);
                    for c in coef.iter_mut().take(n_coef) {
                        *c = br.read(coef_bits)? as u8;
                    }
                }
                *filt = TnsFilter {
                    length,
                    order,
                    direction,
                    coef_compress,
                    coef,
                };
            }
            *win = TnsWindow {
                coef_res,
                n_filt: n_filt as u8,
                filters,
            };
        }
        Ok(TnsData { n_windows, windows })
    }
}

fn max_order(seq: WindowSequence) -> u8 {
    if seq.is_eight_short() { 7 } else { 12 }
}

pub(crate) fn max_bands(fs_index: u8, seq: WindowSequence, ld: bool) -> Result<u8> {
    let idx = fs_index as usize;
    let table = if seq.is_eight_short() {
        &MAX_BANDS_SHORT
    } else if ld {
        &MAX_BANDS_LD
    } else {
        &MAX_BANDS_LONG
    };
    table
        .get(idx)
        .copied()
        .ok_or(Error::UnsupportedSampleRateIndex(fs_index))
}

fn sign_extend(coef: u8, bits: u32) -> i32 {
    let s_mask = 1u8 << (bits - 1);
    if coef & s_mask != 0 {
        (coef as i32) | !((1i32 << bits) - 1)
    } else {
        i32::from(coef)
    }
}

fn decode_lpc(
    order: usize,
    coef_res_bits: u32,
    compress: bool,
    coef: &[u8],
) -> Result<[f64; MAX_TNS_ORDER + 1]> {
    let coef_res2 = coef_res_bits - u32::from(compress);
    if !(2..=4).contains(&coef_res2) {
        return Err(Error::SpectrumInvalid);
    }
    let iqfac = ((1u32 << (coef_res_bits - 1)) as f64 - 0.5) / (std::f64::consts::PI / 2.0);
    let iqfac_m = ((1u32 << (coef_res_bits - 1)) as f64 + 0.5) / (std::f64::consts::PI / 2.0);
    let mut tmp2 = [0.0f64; MAX_TNS_ORDER];
    if coef.len() < order {
        return Err(Error::SpectrumInvalid);
    }
    for (slot, &c) in tmp2.iter_mut().take(order).zip(coef.iter().take(order)) {
        let t = sign_extend(c, coef_res2);
        let ang = t as f64 / if t >= 0 { iqfac } else { iqfac_m };
        *slot = super::det_math::sin_f64(ang);
    }
    #[cfg(test)]
    if tns_lavc_tests::single_precision() {
        return Ok(tns_lavc_tests::step_up_f32(&tmp2[..order]));
    }
    let mut a = [0.0f64; MAX_TNS_ORDER + 1];
    a[0] = 1.0;
    for m in 1..=order {
        let mut b = [0.0f64; MAX_TNS_ORDER];
        for i in 1..m {
            b[i] = a[i] + tmp2[m - 1] * a[m - i];
        }
        a[1..m].copy_from_slice(&b[1..m]);
        a[m] = tmp2[m - 1];
    }
    Ok(a)
}

/// All-pole synthesis in f64: a high-gain filter amplifies rounding, and
/// single precision (libavcodec) drifts up to 5 LSB from this result on
/// `goldens/tns_gain.adts` (TASK-117, `tns_lavc_tests`).
fn ar_filter(spec: &mut [f32], start: usize, size: usize, inc: i32, lpc: &[f64]) {
    let order = lpc.len().saturating_sub(1);
    if size == 0 || order == 0 {
        return;
    }
    #[cfg(test)]
    if tns_lavc_tests::single_precision() {
        return tns_lavc_tests::ar_filter_f32(spec, start, size, inc, lpc);
    }
    let mut hist = [0.0f64; 12];
    let mut pos = 0usize;
    let mut idx = start as isize;
    for _ in 0..size {
        let x = f64::from(spec[idx as usize]);
        let mut y = x;
        let mut hidx = pos;
        for &coeff in lpc.iter().take(order + 1).skip(1) {
            if hidx == 0 {
                hidx = order;
            }
            hidx -= 1;
            y -= coeff * hist[hidx];
        }
        hist[pos] = y;
        spec[idx as usize] = y as f32;
        pos += 1;
        if pos == order {
            pos = 0;
        }
        idx += inc as isize;
    }
}

/// §4.6.9.3 `tns_decode_frame` on a window-major spectrum.
/// Encoder-direction TNS used by LTP on the predicted spectrum.
pub(crate) fn apply_ma(spec: &mut [f32], tns: &TnsData, ics: &IcsInfo, fs_index: u8) -> Result<()> {
    apply_dir(spec, tns, ics, fs_index, false)
}

pub fn apply(spec: &mut [f32], tns: &TnsData, ics: &IcsInfo, fs_index: u8) -> Result<()> {
    apply_dir(spec, tns, ics, fs_index, true)
}

fn apply_dir(
    spec: &mut [f32],
    tns: &TnsData,
    ics: &IcsInfo,
    fs_index: u8,
    decode: bool,
) -> Result<()> {
    let win_len = ics.window_len();
    let offsets = ics.swb_offsets(fs_index)?;
    let tns_max_order = max_order(ics.window_sequence) as usize;
    let tns_max_bands = max_bands(fs_index, ics.window_sequence, ics.ld)? as usize;
    let max_sfb = ics.max_sfb as usize;
    for (w, win) in tns.windows.iter().take(tns.n_windows as usize).enumerate() {
        let mut bottom = ics.num_swb as usize;
        for filt in win.filters.iter().take(win.n_filt as usize) {
            let top = bottom;
            bottom = top.saturating_sub(filt.length as usize);
            let order = (filt.order as usize).min(tns_max_order);
            if order == 0 {
                continue;
            }
            let coef_res_bits = if win.coef_res { 4 } else { 3 };
            let lpc = decode_lpc(order, coef_res_bits, filt.coef_compress, &filt.coef)?;
            let start_sfb = bottom.min(tns_max_bands).min(max_sfb);
            let end_sfb = top.min(tns_max_bands).min(max_sfb);
            let start = *offsets.get(start_sfb).unwrap_or(&0) as usize;
            let end = *offsets.get(end_sfb).unwrap_or(&0) as usize;
            if end <= start {
                continue;
            }
            let size = end - start;
            let base = w * win_len;
            let (inc, origin) = if filt.direction {
                (-1i32, base + end - 1)
            } else {
                (1i32, base + start)
            };
            if decode {
                ar_filter(spec, origin, size, inc, &lpc[..=order]);
            } else {
                ma_filter(spec, origin, size, inc, &lpc[..=order]);
            }
        }
    }
    Ok(())
}

/// All-zero analysis: `y[n] = x[n] + Σ a[i] x[n−i]`, the inverse of `ar_filter`.
fn ma_filter(spec: &mut [f32], start: usize, size: usize, inc: i32, lpc: &[f64]) {
    let order = lpc.len().saturating_sub(1);
    if size == 0 || order == 0 {
        return;
    }
    let mut hist = [0.0f64; MAX_TNS_ORDER];
    let mut pos = 0usize;
    let mut idx = start as isize;
    for _ in 0..size {
        let x = f64::from(spec[idx as usize]);
        let mut y = x;
        let mut hidx = pos;
        for &coeff in lpc.iter().take(order + 1).skip(1) {
            if hidx == 0 {
                hidx = order;
            }
            hidx -= 1;
            y += coeff * hist[hidx];
        }
        hist[pos] = x;
        spec[idx as usize] = y as f32;
        pos += 1;
        if pos == order {
            pos = 0;
        }
        idx += inc as isize;
    }
}

#[cfg(test)]
#[path = "tns_lavc_tests.rs"]
mod tns_lavc_tests;
