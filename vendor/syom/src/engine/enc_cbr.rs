//! Opt-in CBR bit reservoir for [`super::LcEncoder`].
//!
//! Absent means ABR: one frame of credit and ADTS fullness `0x7FF`.
//! Present, unused bits accumulate up to `6144·channels − average`, and a
//! later frame may spend them without crossing that cap. ADTS fullness is
//! the saved bits divided by the channel count and by 32 (ISO/IEC 13818-7),
//! never the VBR marker `0x7FF`.

use super::{LcEncoder, MAX_PAYLOAD_BYTES};
use crate::engine::error::Result;
use crate::engine::swb::LONG_WINDOW_LEN;

/// Saved bits and the ADTS fullness of frames not yet wrapped.
pub(super) struct Reservoir {
    bits: u32,
    log: Vec<u16>,
    at: usize,
}

impl Reservoir {
    pub(super) fn new() -> Self {
        Self {
            bits: 0,
            log: Vec::new(),
            at: 0,
        }
    }

    pub(super) fn clear(&mut self) {
        self.bits = 0;
        self.log.clear();
        self.at = 0;
    }
}

/// `adts_buffer_fullness` for `saved` reservoir bits. Never `0x7FF`.
pub(super) fn fullness_words(saved: u32, channels: usize) -> u16 {
    let ncc = channels.max(1) as u32;
    let words = (saved / ncc / 32).min(0x7FE);
    words as u16
}

impl LcEncoder {
    /// `on` replaces ABR credit with a reservoir. `off` restores ABR.
    #[must_use]
    pub(crate) fn with_cbr(mut self, on: bool) -> Self {
        self.cbr = if on { Some(Reservoir::new()) } else { None };
        self
    }

    /// Fullness of the next unwrapped CBR frame, if this encoder is CBR.
    /// Quality VBR ignores the reservoir so its ADTS marker stays `0x7FF`.
    pub(crate) fn take_cbr_fullness(&mut self) -> Option<u16> {
        if self.quality.is_some() {
            return None;
        }
        let resv = self.cbr.as_mut()?;
        if resv.at >= resv.log.len() {
            return Some(0);
        }
        let v = resv.log[resv.at];
        resv.at += 1;
        if resv.at == resv.log.len() {
            resv.log.clear();
            resv.at = 0;
        }
        Some(v)
    }

    /// Remaining CBR fullness values in encode order. `None` on ABR.
    pub(crate) fn take_cbr_log(&mut self) -> Option<Vec<u16>> {
        if self.quality.is_some() {
            return None;
        }
        let resv = self.cbr.as_mut()?;
        let at = resv.at.min(resv.log.len());
        let rest = resv.log.split_off(at);
        resv.log.clear();
        resv.at = 0;
        Some(rest)
    }

    /// Rate loop that may spend [`Self::budget_bits`] plus the reservoir,
    /// never above the LC frame cap. Does not touch ABR `credit`.
    pub(super) fn rate_control_cbr(
        &mut self,
        specs: &[[f32; LONG_WINDOW_LEN]; 2],
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let cap = super::rate::max_frame_bits(self.channels).min(MAX_PAYLOAD_BYTES * 8);
        let avg = self.budget_bits().min(cap);
        let max_res = cap.saturating_sub(avg);
        let saved = match self.cbr.as_ref() {
            Some(resv) => resv.bits,
            None => return Ok(()),
        };
        let spend = avg.saturating_add(saved as usize).min(cap);
        self.pad_fill = 0;
        let offset = self.search_offset(specs, spend);
        self.build(specs, offset);
        self.refine_bands(specs, spend);
        self.drop_bands_until(spend.min(cap));
        let end_at = self.emit_into(out);
        if !self.quant_all_zero() {
            let floor = (saved as usize).saturating_add(avg).saturating_sub(max_res);
            self.pad_toward(out, end_at, floor.min(cap));
        }
        let used = out.len().saturating_mul(8);
        let next = (i64::from(saved) + avg as i64 - used as i64).clamp(0, max_res as i64);
        let next = next as u32;
        let full = fullness_words(next, self.channels);
        if let Some(resv) = self.cbr.as_mut() {
            resv.bits = next;
            resv.log.push(full);
        }
        Ok(())
    }

    /// EXT_FILL before `ID_END` so a non-silent frame reaches `target_bits`
    /// when the reservoir cannot absorb more (fdk-aac rejects trailing zeros).
    fn pad_toward(&mut self, out: &mut Vec<u8>, end_at: usize, target_bits: usize) {
        let cap_bytes = MAX_PAYLOAD_BYTES.min(super::rate::max_frame_bits(self.channels) / 8);
        let target = (target_bits / 8).min(cap_bytes);
        if target == 0 || out.len() >= target {
            return;
        }
        let room = target
            .saturating_mul(8)
            .saturating_sub(end_at.saturating_add(3));
        let pad = crate::engine::enc_pad::pad_fill_for_room(room);
        if pad == 0 {
            return;
        }
        self.pad_fill = pad;
        self.emit_into(out);
        self.pad_fill = 0;
    }
}
