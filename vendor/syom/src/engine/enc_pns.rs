//! Opt-in perceptual noise substitution (TASK-75).
//!
//! After long-window quantize, replace noise-like HF bands with `NOISE_HCB`
//! and `noise_nrg`. Decoder fills them with the lavc LCG (`engine/pns`).
//! Default off. Short frames skip; TNS-span bands skip.

use super::det_math;
use super::enc_ms::MsBands;
use super::enc_psy;
use super::enc_quant::{MAX_BANDS, QuantChannel, UNREPRESENTABLE};
use super::enc_section;
use super::enc_tns::EncTns;
use super::sf::{NOISE_OFFSET, NOISE_PCM_BITS};
use super::swb::LONG_WINDOW_LEN;

/// Johnston SFM below this is noise-like (same 0.25 the tonality knob uses).
const TONE_MAX: f32 = 0.25;
/// Skip bands whose center is below this (PNS on bass is a tonal smear).
const MIN_HZ: f32 = 4_000.0;
/// Do not substitute a band whose cheapest spectral book is already cheap.
const MIN_SPEC_BITS: u32 = 16;
/// 2048-point MDCT bin Hz scale: `sample_rate / transform`.
const TRANSFORM: f32 = (2 * LONG_WINDOW_LEN) as f32;

/// Map band energy `Σx²` to ISO `noise_nrg`: decoder L2 = `2^(nrg/4)`.
#[must_use]
pub fn nrg_from_energy(energy: f32) -> i32 {
    if energy <= 1e-20 {
        0
    } else {
        (2.0 * det_math::log2(energy)).round() as i32
    }
}

fn first_pcm(nrg: i32, global_gain: u8) -> Option<i32> {
    let pcm = nrg - (i32::from(global_gain) - NOISE_OFFSET - 256);
    (0..=((1 << NOISE_PCM_BITS) - 1))
        .contains(&pcm)
        .then_some(pcm)
}

/// Mark PNS bands on a built long-window frame; re-plan sections.
#[allow(clippy::too_many_arguments)]
pub fn apply_frame(
    specs: &[[f32; LONG_WINDOW_LEN]; 2],
    offsets: &[u16],
    sample_rate: u32,
    channels: usize,
    chans: &mut [QuantChannel],
    books: &mut [[u8; MAX_BANDS]],
    gains: &[u8],
    ms: &MsBands,
    tns: &[EncTns],
) {
    let n_bands = chans.first().map(|q| q.n_bands).unwrap_or(0);
    let bin_hz = sample_rate as f32 / TRANSFORM;
    let mut want = [[false; MAX_BANDS]; 2];
    for ch in 0..channels {
        chans[ch].pns[..n_bands].fill(false);
        let tns_span = tns.get(ch).and_then(|t| t.band_range());
        let spec = &specs[ch];
        for b in 0..n_bands {
            if !chans[ch].coded[b] {
                continue;
            }
            if tns_span.is_some_and(|(lo, hi)| b >= lo && b < hi) {
                continue;
            }
            let lo = usize::from(offsets[b]);
            let hi = usize::from(offsets[b + 1]);
            let hz = 0.5 * (f32::from(offsets[b]) + f32::from(offsets[b + 1])) * bin_hz;
            if hz < MIN_HZ || enc_psy::band_tonality(&spec[lo..hi]) >= TONE_MAX {
                continue;
            }
            let cb = usize::from(books[ch][b]);
            let spec_bits = chans[ch].bits[b]
                .get(cb)
                .copied()
                .unwrap_or(UNREPRESENTABLE);
            if spec_bits < MIN_SPEC_BITS || spec_bits == UNREPRESENTABLE {
                continue;
            }
            let e: f32 = spec[lo..hi].iter().map(|&x| x * x).sum();
            let nrg = nrg_from_energy(e);
            if first_pcm(nrg, gains[ch]).is_none() {
                continue;
            }
            want[ch][b] = true;
            chans[ch].noise_nrg[b] = nrg;
        }
    }
    both_or_neither(channels, n_bands, ms, &mut want);
    for (ch, want_ch) in want.iter_mut().enumerate().take(channels) {
        let mut last = i32::from(gains[ch]) - NOISE_OFFSET - 256;
        let mut first = true;
        for (b, slot) in want_ch.iter_mut().enumerate().take(n_bands) {
            if !*slot {
                continue;
            }
            let nrg = chans[ch].noise_nrg[b];
            if first {
                if first_pcm(nrg, gains[ch]).is_none() {
                    *slot = false;
                    continue;
                }
                first = false;
                last = nrg;
            } else if (nrg - last).abs() > 60 {
                *slot = false;
            } else {
                last = nrg;
            }
        }
    }
    both_or_neither(channels, n_bands, ms, &mut want);
    for ch in 0..channels {
        chans[ch].pns[..n_bands].copy_from_slice(&want[ch][..n_bands]);
        if want[ch][..n_bands].iter().any(|&w| w) {
            books[ch] = enc_section::plan_books(&chans[ch]);
        }
    }
}

fn both_or_neither(
    channels: usize,
    n_bands: usize,
    ms: &MsBands,
    want: &mut [[bool; MAX_BANDS]; 2],
) {
    if channels != 2 {
        return;
    }
    let (left, right) = want.split_at_mut(1);
    for (b, (l, r)) in left[0]
        .iter_mut()
        .zip(right[0].iter_mut())
        .enumerate()
        .take(n_bands)
    {
        if ms.used_at(b) && *l != *r {
            *l = false;
            *r = false;
        }
    }
}

#[cfg(test)]
#[path = "enc_pns_tests.rs"]
mod enc_pns_tests;
