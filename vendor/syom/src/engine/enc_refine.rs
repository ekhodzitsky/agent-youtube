//! Bounded bandwise scalefactor refinement (TASK-74).
//!
//! After the global offset is chosen, spend leftover budget on `sf[b] -= 1`
//! for the underfunded coded band (max energy / (qmax+1), lowest index
//! wins ties). Long frames only. At most [`MAX_SUCCESS`] keeps and
//! [`MAX_TRIES`] attempts. Masking and the greedy section planner stay
//! fixed. Default off.

use super::super::enc_quant::{self, MAX_BANDS, QUANT_MAX};
use super::super::enc_section;
use super::super::swb::LONG_WINDOW_LEN;
use super::{LcEncoder, TARGET_Q};

/// Successful `sf -= 1` steps per frame.
pub const MAX_SUCCESS: u32 = 16;
/// Attempts including rejected DPCM / over-budget tries.
pub const MAX_TRIES: u32 = 48;

impl LcEncoder {
    /// Spend leftover bits on long-window bands. No-op on EightShort.
    pub(super) fn refine_bands(
        &mut self,
        specs: &[[f32; LONG_WINDOW_LEN]; 2],
        limit_bits: usize,
    ) -> u32 {
        if !self.band_refine || self.seq.is_eight_short() {
            return 0;
        }
        let limit = limit_bits.min(super::rate::max_frame_bits(self.channels));
        let mut skip = [[false; MAX_BANDS]; 2];
        let mut ok = 0u32;
        let mut tries = 0u32;
        while ok < MAX_SUCCESS && tries < MAX_TRIES {
            tries += 1;
            let Some((ch, b)) = pick_band(self, specs, &skip) else {
                break;
            };
            if step_band(self, specs, ch, b, limit) {
                ok += 1;
            } else {
                skip[ch][b] = true;
            }
        }
        ok
    }
}

fn pick_band(
    enc: &LcEncoder,
    specs: &[[f32; LONG_WINDOW_LEN]; 2],
    skip: &[[bool; MAX_BANDS]; 2],
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for ch in 0..enc.channels {
        let q = &enc.chans[ch];
        let spec = &specs[ch];
        for (b, ((&sk, &coded), &sf)) in skip[ch]
            .iter()
            .zip(q.coded.iter())
            .zip(q.sf.iter())
            .enumerate()
            .take(q.n_bands)
        {
            if sk || !coded || sf <= 0 || q.pns[b] || q.intensity[b] {
                continue;
            }
            let lo = usize::from(enc.offsets[b]);
            let hi = usize::from(enc.offsets[b + 1]);
            let mut e = 0.0f32;
            let mut qmax = 0u32;
            for (&x, &qv) in spec[lo..hi].iter().zip(q.quant[lo..hi].iter()) {
                e += x * x;
                qmax = qmax.max(qv.unsigned_abs());
            }
            if qmax >= QUANT_MAX as u32 || qmax as f32 >= TARGET_Q {
                continue;
            }
            let score = e / (qmax as f32 + 1.0);
            let take = match best {
                None => true,
                Some((_, _, s)) => score > s,
            };
            if take {
                best = Some((ch, b, score));
            }
        }
    }
    best.map(|(ch, b, _)| (ch, b))
}

fn step_band(
    enc: &mut LcEncoder,
    specs: &[[f32; LONG_WINDOW_LEN]; 2],
    ch: usize,
    b: usize,
    limit: usize,
) -> bool {
    let q = &mut enc.chans[ch];
    let old_sf = q.sf[b];
    q.sf[b] = old_sf - 1;
    if !enc_quant::dpcm_ok(&q.sf, &q.coded, q.n_bands) {
        q.sf[b] = old_sf;
        return false;
    }
    // Snapshot quant+bits for revert.
    let lo = usize::from(enc.offsets[b]);
    let hi = usize::from(enc.offsets[b + 1]);
    let mut old_q = [0i32; 128];
    let n = hi - lo;
    old_q[..n].copy_from_slice(&q.quant[lo..hi]);
    let old_bits = q.bits[b];
    let old_books = enc.books[ch];
    let old_gain = enc.gains[ch];
    enc_quant::requant_band(&specs[ch], enc.offsets, q, b);
    let clipped = q.quant[lo..hi]
        .iter()
        .any(|v| v.unsigned_abs() >= QUANT_MAX as u32);
    if clipped {
        q.sf[b] = old_sf;
        q.quant[lo..hi].copy_from_slice(&old_q[..n]);
        q.bits[b] = old_bits;
        return false;
    }
    enc.books[ch] = enc_section::plan_books(q);
    if let Some(first) = q.coded.iter().position(|&c| c) {
        enc.gains[ch] = q.sf[first] as u8;
    }
    let mut tmp = std::mem::take(&mut enc.payload);
    enc.emit_into(&mut tmp);
    let bits = tmp.len().saturating_mul(8);
    enc.payload = tmp;
    if bits <= limit {
        return true;
    }
    let q = &mut enc.chans[ch];
    q.sf[b] = old_sf;
    q.quant[lo..hi].copy_from_slice(&old_q[..n]);
    q.bits[b] = old_bits;
    enc.books[ch] = old_books;
    enc.gains[ch] = old_gain;
    false
}
