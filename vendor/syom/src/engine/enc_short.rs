//! Short-window (`EIGHT_SHORT`) section planning and element emission for
//! the LC encoder — mirrors the short branches of [`super::section`]
//! (Table 4.5), [`super::ics`] (Table 4.6), [`super::sf`] (Table 4.53), and
//! the grouped spectral walk of [`super::spectrum::parse_quant_into`].
//!
//! Default grouping is 8×1 (`scale_factor_grouping = 0`). Opt-in grouping
//! shares scale factors / sections across similar consecutive windows.
//! Scalefactor DPCM still runs continuously across groups.

use super::bits::BitWriter;
use super::enc_group::{self, Grouping};
use super::enc_huff::{sf_delta_bits, sf_emit_delta};
use super::enc_ms::MsBands;
use super::enc_psy::Psy;
use super::enc_quant::{self, MAX_FLAT_SHORT, MAX_GROUPS, QuantShort};
use super::enc_section::{emit_ics_info, plan_books_into};
use super::enc_tns::EncTns;
use super::ics::WindowSequence;
use super::section::has_spectral;
use super::swb::{LONG_WINDOW_LEN, SHORT_WINDOW_LEN};

#[path = "enc_short_win.rs"]
mod win;
pub use win::{ShortWindows, spectra};

/// Quantize + plan one short channel at `offset`; returns its body bits.
/// Psy scores each window; [`Grouping`] then folds windows into groups.
#[allow(clippy::too_many_arguments)]
pub fn channel_build(
    psy: &mut Psy,
    offsets: &[u16],
    spec: &[f32; LONG_WINDOW_LEN],
    offset: i32,
    q: &mut QuantShort,
    tq: &mut [f32; MAX_FLAT_SHORT],
    books: &mut [u8; MAX_FLAT_SHORT],
    gain: &mut u8,
    standalone: bool,
    tns: &EncTns,
    grouping: Grouping,
) -> usize {
    let n_sfb = q.n_sfb;
    let mut win_coded = [false; MAX_FLAT_SHORT];
    let mut win_tq = [0.0f32; MAX_FLAT_SHORT];
    let mut win_forced = [false; MAX_FLAT_SHORT];
    // Per-window energy / allowed noise / Σ√|x| for the noise targets.
    let mut win_e = [0.0f32; MAX_FLAT_SHORT];
    let mut win_n = [0.0f32; MAX_FLAT_SHORT];
    let mut win_s = [0.0f32; MAX_FLAT_SHORT];
    for w in 0..MAX_GROUPS {
        let (lo, hi) = (w * n_sfb, (w + 1) * n_sfb);
        psy.analyze(
            &spec[w * SHORT_WINDOW_LEN..(w + 1) * SHORT_WINDOW_LEN],
            offsets,
            crate::engine::enc_frame::TARGET_Q,
            &mut win_coded[lo..hi],
            &mut win_tq[lo..hi],
        );
        let (e, n, s) = psy.bands();
        win_e[lo..hi].copy_from_slice(&e[..n_sfb]);
        win_n[lo..hi].copy_from_slice(&n[..n_sfb]);
        win_s[lo..hi].copy_from_slice(&s[..n_sfb]);
        if tns.short().window_on(w) {
            for b in 0..n_sfb.min(14) {
                win_forced[lo + b] = !win_coded[lo + b];
                win_coded[lo + b] = true;
                win_tq[lo + b] = crate::engine::enc_frame::TARGET_Q;
            }
        }
    }
    let mut peaks = [0.0f32; MAX_FLAT_SHORT];
    enc_quant::band_peaks_short(spec, offsets, n_sfb, &mut peaks);
    q.set_grouping(grouping);
    let mut gpeaks = [0.0f32; MAX_FLAT_SHORT];
    enc_group::fold_windows(
        grouping,
        n_sfb,
        &win_coded,
        &win_tq,
        &peaks,
        &mut q.coded,
        tq,
        &mut gpeaks,
    );
    // Fold energy / noise / Σ√|x| (sums) and the TNS forcing (any) per group.
    let n = q.n_groups * n_sfb;
    let (mut ge, mut gn, mut gs) = (
        [0.0f32; MAX_FLAT_SHORT],
        [0.0f32; MAX_FLAT_SHORT],
        [0.0f32; MAX_FLAT_SHORT],
    );
    let mut gw = [0.0f32; MAX_FLAT_SHORT];
    let mut gforced = [false; MAX_FLAT_SHORT];
    let mut wbase = 0usize;
    for g in 0..q.n_groups {
        for k in 0..q.group_len[g] as usize {
            for b in 0..n_sfb {
                let (o, i) = (g * n_sfb + b, (wbase + k) * n_sfb + b);
                ge[o] += win_e[i];
                gn[o] += win_n[i];
                gs[o] += win_s[i];
                gw[o] += f32::from(offsets[b + 1] - offsets[b]);
                gforced[o] |= win_forced[i];
            }
        }
        wbase += q.group_len[g] as usize;
    }
    let cap = *tq;
    crate::engine::enc_alloc::noise_targets(
        &gpeaks,
        &ge,
        &gs,
        &gn,
        &gw,
        &cap,
        offset,
        n,
        &mut q.coded,
        tq,
    );
    for o in 0..n {
        if gforced[o] && !q.coded[o] && gpeaks[o] > 0.0 {
            q.coded[o] = true;
            tq[o] = 1.0;
        }
    }
    enc_quant::raw_scalefactors_short(&gpeaks, tq, 0, q);
    *gain = enc_quant::normalize_sf_short(q);
    enc_quant::quantize_short_cached(spec, offsets, q);
    *books = plan_books_short(q);
    channel_body_bits_short(books, q, *gain, standalone, tns)
}

/// Emit the short-frame `raw_data_block` from the built state.
/// Returns the bit length before `END` (ABR padding sizes against it).
#[allow(clippy::too_many_arguments)]
pub fn emit_frame(
    offsets: &[u16],
    chans: &[QuantShort],
    books: &[[u8; MAX_FLAT_SHORT]],
    gains: &[u8],
    ms: &MsBands,
    tns: &[EncTns],
    channels: usize,
    fill: &[u8],
    pad_fill: usize,
    out: &mut Vec<u8>,
) -> usize {
    let mut w = BitWriter::from_vec(std::mem::take(out));
    if channels == 1 {
        w.write(0, 3); // SCE
        w.write(0, 4); // tag
        emit_channel_body_short(
            &mut w, offsets, &books[0], &chans[0], gains[0], true, &tns[0],
        );
    } else {
        w.write(1, 3); // CPE
        w.write(0, 4); // tag
        w.write_bit(true); // common_window
        emit_ics_info(
            &mut w,
            WindowSequence::EightShort,
            chans[0].n_sfb as u8,
            chans[0].grouping_bits(),
        );
        ms.emit(&mut w); // ms_mask_present + optional per-group ms_used bits
        for ch in 0..channels {
            emit_channel_body_short(
                &mut w, offsets, &books[ch], &chans[ch], gains[ch], false, &tns[ch],
            );
        }
    }
    if !fill.is_empty() {
        let _ = super::enc_sbr_bits::write_fill_element(&mut w, fill); // HE FIL
    }
    super::enc_pad::write_pad_fill(&mut w, pad_fill); // ABR stuffing
    let end_at = w.bit_len() as usize;
    w.write(7, 3); // END
    *out = w.finish();
    end_at
}

/// Short-window section-header cost (4-bit cb + 3-bit increments, escape 7).
fn section_header_bits_short(len: usize) -> usize {
    4 + 3 * (len / 7 + 1)
}

/// Choose `sfb_cb` per (window, band): planning is per group (window), the
/// result is flattened `g * n_sfb + b`.
pub fn plan_books_short(q: &QuantShort) -> [u8; MAX_FLAT_SHORT] {
    let mut sfb_cb = [0u8; MAX_FLAT_SHORT];
    let n = q.n_groups * q.n_sfb;
    // ZERO_HCB for all-zero (group, band)s when the flat sf chain stays
    // DPCM-valid (TASK-113), as `plan_books` does for long frames.
    let mut eff = q.coded;
    for (e, &z) in eff.iter_mut().zip(q.zero.iter()).take(n) {
        *e &= !z;
    }
    let coded = if super::enc_quant::dpcm_ok(&q.sf, &eff, n) {
        eff
    } else {
        q.coded
    };
    for g in 0..q.n_groups {
        let lo = g * q.n_sfb;
        plan_books_into(
            &coded[lo..lo + q.n_sfb],
            &q.bits[lo..lo + q.n_sfb],
            q.n_sfb,
            &mut sfb_cb[lo..lo + q.n_sfb],
            section_header_bits_short,
        );
    }
    sfb_cb
}

/// `section_data()` for a short frame (Table 4.5 short-window branch).
pub fn emit_section_data_short(
    w: &mut BitWriter,
    sfb_cb: &[u8; MAX_FLAT_SHORT],
    n_sfb: usize,
    n_groups: usize,
) {
    for g in 0..n_groups {
        let row = &sfb_cb[g * n_sfb..(g + 1) * n_sfb];
        let mut k = 0usize;
        while k < n_sfb {
            let cb = row[k];
            let mut len = 1usize;
            while k + len < n_sfb && row[k + len] == cb {
                len += 1;
            }
            w.write(u32::from(cb), 4);
            let mut rem = len;
            while rem >= 7 {
                w.write(7, 3);
                rem -= 7;
            }
            w.write(rem as u32, 3);
            k += len;
        }
    }
}

/// `scale_factor_data()` for a short frame: DPCM over the flattened
/// (group, band) order — the decoder's `last_sf` continues across groups.
pub fn emit_scale_factors_short(
    w: &mut BitWriter,
    sfb_cb: &[u8; MAX_FLAT_SHORT],
    q: &QuantShort,
    global_gain: u8,
) {
    let mut prev = i32::from(global_gain);
    let n = q.n_groups * q.n_sfb;
    let bands = sfb_cb.iter().zip(q.sf.iter()).take(n);
    for (&cb, &sf) in bands {
        if !has_spectral(cb) {
            continue;
        }
        sf_emit_delta(w, sf - prev);
        prev = sf;
    }
}

/// Total bits of one short-window channel body.
pub fn channel_body_bits_short(
    sfb_cb: &[u8; MAX_FLAT_SHORT],
    q: &QuantShort,
    global_gain: u8,
    standalone: bool,
    tns: &EncTns,
) -> usize {
    let mut bits = 8; // global_gain
    if standalone {
        bits += 15; // ics_info (short)
    }
    // section_data, per group
    for g in 0..q.n_groups {
        let row = &sfb_cb[g * q.n_sfb..(g + 1) * q.n_sfb];
        let mut k = 0usize;
        while k < q.n_sfb {
            let mut len = 1usize;
            while k + len < q.n_sfb && row[k + len] == row[k] {
                len += 1;
            }
            bits += section_header_bits_short(len);
            k += len;
        }
    }
    // scale_factor_data + spectral data, flattened (group, band) order
    let n = q.n_groups * q.n_sfb;
    let mut prev = i32::from(global_gain);
    let bands = sfb_cb.iter().zip(q.sf.iter()).zip(q.bits.iter()).take(n);
    for ((&cb, &sf), row) in bands {
        if !has_spectral(cb) {
            continue;
        }
        bits += sf_delta_bits(sf - prev);
        prev = sf;
        bits += row[usize::from(cb)] as usize;
    }
    bits += 2 + tns.bits(); // pulse + gain, tns flag + payload
    bits
}

/// Emit one short-window channel body (`individual_channel_stream`):
/// global_gain, then `ics_info` when `standalone`, then section_data +
/// scale_factor_data + flags + spectral data.
pub fn emit_channel_body_short(
    w: &mut BitWriter,
    offsets: &[u16],
    sfb_cb: &[u8; MAX_FLAT_SHORT],
    q: &QuantShort,
    global_gain: u8,
    standalone: bool,
    tns: &EncTns,
) {
    w.write(u32::from(global_gain), 8);
    if standalone {
        emit_ics_info(
            w,
            WindowSequence::EightShort,
            q.n_sfb as u8,
            q.grouping_bits(),
        );
    }
    emit_section_data_short(w, sfb_cb, q.n_sfb, q.n_groups);
    emit_scale_factors_short(w, sfb_cb, q, global_gain);
    w.write_bit(false); // pulse_data_present
    tns.emit(w);
    w.write_bit(false); // gain_control_data_present
    enc_group::emit_spectral_short(w, offsets, sfb_cb, q);
}

#[cfg(test)]
#[path = "enc_short_tests.rs"]
mod enc_short_tests;
