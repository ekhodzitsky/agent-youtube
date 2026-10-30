//! Per-frame rate control of [`LcEncoder`] (line cap): the ABR loop
//! (search the allowed-noise offset that fits `bitrate_bps` plus one
//! frame of credit, refine, pad — decision-8 / TASK-66) or quality VBR
//! (TASK-67: one fixed offset, no target rate, no padding, only the
//! 6144 bits/channel cap).

use super::rate::max_frame_bits;
use super::{LcEncoder, MAX_PAYLOAD_BYTES};
use crate::engine::enc_section;
use crate::engine::enc_short;
use crate::engine::error::{Error, Result};
use crate::engine::swb::LONG_WINDOW_LEN;

/// Allowed-noise offset steps (0.75 dB of energy each) per quality level:
/// level 5 is the psy target; every level above refines 6 dB, every level
/// below raises the water level 6 dB (quiet bands leave first).
const STEPS_PER_LEVEL: i32 = 8;
/// Highest quality level.
pub const QUALITY_MAX: u8 = 10;

impl LcEncoder {
    /// Per-frame bit budget from the target bitrate (no reservoir),
    /// never above the LC 6144 bits/channel cap or the ADTS byte ceiling.
    pub(super) fn budget_bits(&self) -> usize {
        let bits = u64::from(self.bitrate_bps) * 1024 / u64::from(self.sample_rate);
        (bits as usize)
            .min(max_frame_bits(self.channels))
            .min(MAX_PAYLOAD_BYTES * 8)
    }

    /// Quality VBR level 0..=10 (`None` = ABR). Level 5 codes at the psy
    /// model's noise-to-mask target; each level is 6 dB of allowed noise.
    #[must_use]
    pub(crate) fn with_quality(mut self, level: Option<u8>) -> Self {
        self.quality = level.map(|q| (5 - i32::from(q.min(QUALITY_MAX))) * STEPS_PER_LEVEL);
        self
    }

    /// `|x|^0.75` once per frame: every `build` re-quantizes these spectra.
    pub(super) fn cache_mags(&mut self, specs: &[[f32; LONG_WINDOW_LEN]; 2]) {
        for (ch, spec) in specs.iter().enumerate().take(self.channels) {
            if self.seq.is_eight_short() {
                crate::engine::enc_quant::cache_mags(spec, &mut self.chans_s[ch].mag);
            } else {
                crate::engine::enc_quant::cache_mags(spec, &mut self.chans[ch].mag);
            }
        }
    }

    /// Quantize the built spectra into `payload` under the active mode.
    pub(super) fn rate_control(&mut self, specs: &[[f32; LONG_WINDOW_LEN]; 2]) -> Result<()> {
        let cap = max_frame_bits(self.channels);
        self.cache_mags(specs);
        let mut payload = std::mem::take(&mut self.payload);
        if let Some(offset) = self.quality {
            // The level's offset, coarsened uniformly (never by dropping
            // top bands) when a frame would exceed the LC cap.
            let offset = if self.build(specs, offset) > cap {
                self.search_offset_from(offset, specs, cap)
            } else {
                offset
            };
            self.build(specs, offset);
            self.drop_bands_until(cap);
            self.emit_into(&mut payload);
            self.credit = 0;
        } else if self.cbr.is_some() {
            // Opt-in reservoir. The ABR arm below is the default path.
            self.rate_control_cbr(specs, &mut payload)?;
        } else {
            let budget = self.budget_bits();
            let spend = budget + (self.credit.min(budget as i64 / 2)) as usize;
            let offset = self.search_offset(specs, spend);
            self.build(specs, offset);
            self.refine_bands(specs, spend);
            let coded = self.fit_budget(spend.min(cap), &mut payload);
            self.credit = (self.credit + budget as i64 - coded as i64).clamp(0, budget as i64);
        }
        self.payload = payload;
        if self.payload.len().saturating_mul(8) > cap {
            return Err(Error::Format("LC encoder: frame exceeds 6144 bits/channel"));
        }
        self.prev_seq = self.seq;
        Ok(())
    }

    /// Hard cap: drop the highest coded bands until the frame fits
    /// `limit_bits` (the bit budget, never above 6144 bits/channel or
    /// the ADTS payload limit). Quality collapse is legal, an oversize
    /// frame is not.
    pub(super) fn drop_bands_until(&mut self, limit_bits: usize) {
        let cap = limit_bits
            .min(max_frame_bits(self.channels))
            .min(MAX_PAYLOAD_BYTES * 8);
        loop {
            let mut tmp = std::mem::take(&mut self.payload);
            self.emit_into(&mut tmp);
            let bits = tmp.len().saturating_mul(8);
            self.payload = tmp;
            if bits <= cap {
                return;
            }
            if self.seq.is_eight_short() {
                let mut hit = None;
                for (ch, q) in self.chans_s.iter().enumerate().take(self.channels) {
                    if let Some(b) = q.coded.iter().rposition(|&c| c) {
                        hit = Some((ch, b));
                        break;
                    }
                }
                let Some((ch, b)) = hit else { return };
                self.chans_s[ch].coded[b] = false;
                self.books_s[ch] = enc_short::plan_books_short(&self.chans_s[ch]);
            } else {
                let mut hit = None;
                for (ch, q) in self.chans.iter().enumerate().take(self.channels) {
                    if let Some(b) = q.coded[..q.n_bands].iter().rposition(|&c| c) {
                        hit = Some((ch, b));
                        break;
                    }
                }
                let Some((ch, b)) = hit else { return };
                self.chans[ch].coded[b] = false;
                self.chans[ch].pns[b] = false;
                self.chans[ch].intensity[b] = false;
                self.books[ch][b] = 0;
                self.books[ch] = enc_section::plan_books(&self.chans[ch]);
            }
        }
    }

    /// Pad an undersized frame with EXT_FILL `fill_element()`s before
    /// `ID_END` so payload bits sit in `[97%, 100%]` of `limit` (zero
    /// bytes after `ID_END` are rejected by fdk-aac — TASK-121). Returns
    /// the size before padding — one-frame `credit` must use that, or
    /// stuffing would starve the next frame's rate loop. All-zero spectra
    /// are not padded (silence exception). Not CBR.
    pub(super) fn fit_budget(&mut self, limit: usize, out: &mut Vec<u8>) -> usize {
        let limit = limit.min(max_frame_bits(self.channels));
        self.drop_bands_until(limit);
        let end_at = self.emit_into(out);
        let coded = out.len().saturating_mul(8);
        if self.quant_all_zero() {
            return coded;
        }
        // Pad to the ABR ceiling, minus stuffing debt from earlier frames
        // that spent credit above `budget`. Search/drop still use credit;
        // this only changes EXT_FILL payload before ID_END.
        let budget = self.budget_bits().min(limit);
        let pad_to = if self.pad_debt > 0 {
            budget.saturating_sub(self.pad_debt as usize)
        } else {
            budget
        };
        if coded < pad_to.saturating_mul(97) / 100 {
            let target = (pad_to / 8).min(MAX_PAYLOAD_BYTES);
            let room = target.saturating_mul(8).saturating_sub(end_at + 3);
            let pad = crate::engine::enc_pad::pad_fill_for_room(room);
            if pad > 0 {
                self.pad_fill = pad;
                self.emit_into(out);
                self.pad_fill = 0;
            }
        }
        let emitted = out.len().saturating_mul(8) as i64;
        self.pad_debt = (self.pad_debt + emitted - budget as i64).max(0);
        coded
    }

    pub(super) fn quant_all_zero(&self) -> bool {
        if self.seq.is_eight_short() {
            self.chans_s[..self.channels]
                .iter()
                .all(|q| q.quant.iter().all(|&x| x == 0))
        } else {
            self.chans[..self.channels]
                .iter()
                .all(|q| q.quant.iter().all(|&x| x == 0))
        }
    }
}
