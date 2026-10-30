//! Bit emission for [`super`] (line cap): `ics_info`, `section_data`,
//! `scale_factor_data` and the Huffman spectral walk.

use super::super::bits::BitWriter;
use super::super::enc_huff::{sf_emit_delta, spectral_emit};
use super::super::enc_quant::{MAX_BANDS, QuantChannel};
use super::super::ics::WindowSequence;
use super::super::section::{has_spectral, is_intensity, is_noise};
use super::super::sf::{NOISE_OFFSET, NOISE_PCM_BITS};

/// `ics_info()` for an LC stream (Table 4.6, KBD window — the encoder's
/// analysis window — no predictor). Short frames carry `max_sfb` on 4 bits
/// plus 7-bit `scale_factor_grouping` (`grouping = 0` → 8 groups of 1).
pub fn emit_ics_info(w: &mut BitWriter, seq: WindowSequence, max_sfb: u8, grouping: u8) {
    w.write_bit(false); // ics_res
    w.write(u32::from(seq as u8), 2);
    w.write_bit(true); // window_shape: KBD
    if seq.is_eight_short() {
        w.write(u32::from(max_sfb), 4);
        w.write(u32::from(grouping), 7);
    } else {
        w.write(u32::from(max_sfb), 6);
        w.write_bit(false); // predictor_data_present
    }
}

/// `section_data()` for one group (Table 4.5 long-window branch).
pub fn emit_section_data(w: &mut BitWriter, sfb_cb: &[u8; MAX_BANDS], max_sfb: usize) {
    let mut k = 0usize;
    while k < max_sfb {
        let cb = sfb_cb[k];
        let mut len = 1usize;
        while k + len < max_sfb && sfb_cb[k + len] == cb {
            len += 1;
        }
        w.write(u32::from(cb), 4);
        let mut rem = len;
        while rem >= 31 {
            w.write(31, 5);
            rem -= 31;
        }
        w.write(rem as u32, 5);
        k += len;
    }
}

/// `scale_factor_data()` (Table 4.53): DPCM over spectral, PNS, and IS.
pub fn emit_scale_factors(
    w: &mut BitWriter,
    sfb_cb: &[u8; MAX_BANDS],
    q: &QuantChannel,
    global_gain: u8,
) {
    let mut prev_sf = i32::from(global_gain);
    let mut prev_nrg = i32::from(global_gain) - NOISE_OFFSET - 256;
    let mut prev_is = 0i32;
    let mut noise_first = true;
    let bands = sfb_cb
        .iter()
        .zip(q.sf.iter())
        .zip(q.noise_nrg.iter())
        .zip(q.is_pos.iter())
        .take(q.n_bands);
    for (((&cb, &sf), &nrg), &pos) in bands {
        if is_noise(cb) {
            if noise_first {
                w.write((nrg - prev_nrg).clamp(0, 511) as u32, NOISE_PCM_BITS);
                noise_first = false;
            } else {
                sf_emit_delta(w, nrg - prev_nrg);
            }
            prev_nrg = nrg;
        } else if is_intensity(cb) {
            sf_emit_delta(w, pos - prev_is);
            prev_is = pos;
        } else if has_spectral(cb) {
            sf_emit_delta(w, sf - prev_sf);
            prev_sf = sf;
        }
    }
}

/// Huffman spectral data, mirroring the `parse_quant_into` long-window walk.
pub fn emit_spectral(
    w: &mut BitWriter,
    offsets: &[u16],
    sfb_cb: &[u8; MAX_BANDS],
    q: &QuantChannel,
) {
    let bands = sfb_cb.iter().zip(offsets.windows(2)).take(q.n_bands);
    for (&cb, win) in bands {
        if !has_spectral(cb) {
            continue;
        }
        let lo = usize::from(win[0]);
        let hi = usize::from(win[1]);
        let step = if cb <= 4 { 4 } else { 2 };
        let mut i = lo;
        while i < hi {
            spectral_emit(cb, &q.quant[i..i + step], w);
            i += step;
        }
    }
}
