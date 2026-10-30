//! Opt-in intensity stereo (TASK-76).
//!
//! After TNS, before M/S, mark long-window HF bands whose L/R envelopes
//! match (`|ρ| ≥ 0.85`). Those bands stay L/R. After quantize, the right
//! channel emits `INTENSITY_HCB` / `HCB2` and `is_pos` instead of Huffman.
//! Default off. Short frames and PNS bands skip.

use super::det_math;
use super::enc_quant::{MAX_BANDS, QuantChannel};
use super::enc_section;
use super::section::{INTENSITY_HCB, INTENSITY_HCB2};
use super::swb::LONG_WINDOW_LEN;

/// Skip bass/mid — IS is an HF ILD tool.
const MIN_HZ: f32 = 6_000.0;
/// |ρ| below this is ambience, not a panned image.
const RHO_MIN: f64 = 0.85;
const TRANSFORM: f32 = (2 * LONG_WINDOW_LEN) as f32;
const EPS: f64 = 1e-20;

/// Mark candidate IS bands on post-TNS L/R (before the M/S transform).
pub fn plan(
    specs: &[[f32; LONG_WINDOW_LEN]; 2],
    offsets: &[u16],
    sample_rate: u32,
    band: &mut [bool],
) {
    band.fill(false);
    let n = offsets.len().saturating_sub(1).min(band.len());
    let bin_hz = sample_rate as f32 / TRANSFORM;
    let mut loudest = 0.0f64;
    let mut stats = [(0.0f64, 0.0f64, 0.0f64); MAX_BANDS];
    for (b, st) in stats.iter_mut().enumerate().take(n) {
        *st = band_stats(&specs[0], &specs[1], offsets, b);
        loudest = loudest.max(st.1 + st.2);
    }
    let floor = loudest * 1e-4;
    for (b, slot) in band.iter_mut().enumerate().take(n) {
        let hz = 0.5 * (f32::from(offsets[b]) + f32::from(offsets[b + 1])) * bin_hz;
        if hz < MIN_HZ {
            continue;
        }
        let (rho, el, er) = stats[b];
        *slot = el > EPS && er > EPS && el + er >= floor && rho.abs() >= RHO_MIN;
    }
}

/// Stamp right-channel intensity books. `band` is the pre-M/S plan.
pub fn stamp(
    specs: &[[f32; LONG_WINDOW_LEN]; 2],
    offsets: &[u16],
    band: &[bool],
    chans: &mut [QuantChannel],
    books: &mut [[u8; MAX_BANDS]],
) {
    if chans.len() < 2 || books.len() < 2 {
        return;
    }
    let n = chans[1].n_bands;
    chans[0].intensity[..n].fill(false);
    chans[1].intensity[..n].fill(false);
    let mut prev = 0i32;
    let mut any = false;
    for b in 0..n {
        if !band.get(b).copied().unwrap_or(false)
            || chans[0].pns[b]
            || chans[1].pns[b]
            || !chans[0].coded[b]
        {
            continue;
        }
        let (rho, el, er) = band_stats(&specs[0], &specs[1], offsets, b);
        if el <= EPS || rho.abs() < RHO_MIN {
            continue;
        }
        let ratio = ((er / el).sqrt() as f32).max(1e-6);
        let pos = (-4.0 * det_math::log2(ratio)).round() as i32;
        if (pos - prev).abs() > 60 {
            continue;
        }
        chans[1].intensity[b] = true;
        chans[1].is_pos[b] = pos;
        chans[1].is_hcb[b] = if rho < 0.0 {
            INTENSITY_HCB2
        } else {
            INTENSITY_HCB
        };
        chans[1].coded[b] = true;
        prev = pos;
        any = true;
    }
    if any {
        books[1] = enc_section::plan_books(&chans[1]);
    }
}

fn band_stats(l: &[f32], r: &[f32], offsets: &[u16], b: usize) -> (f64, f64, f64) {
    let lo = usize::from(offsets[b]);
    let hi = usize::from(offsets[b + 1]);
    let mut el = 0.0f64;
    let mut er = 0.0f64;
    let mut lr = 0.0f64;
    for (&a, &c) in l[lo..hi].iter().zip(r[lo..hi].iter()) {
        let x = f64::from(a);
        let y = f64::from(c);
        el += x * x;
        er += y * y;
        lr += x * y;
    }
    let den = (el * er).sqrt();
    let rho = if den > EPS { lr / den } else { 0.0 };
    (rho, el, er)
}

#[cfg(test)]
#[path = "enc_is_tests.rs"]
mod enc_is_tests;
