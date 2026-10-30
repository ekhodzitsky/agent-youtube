//! Split-phase hooks on [`LcEncoder`] for the surround encoder
//! (TASK-115): analyze a frame, price it at an allowed-noise offset, then
//! emit it at the offset the whole frame agreed on. A child module of
//! `enc_frame`, like `rate`; mono/stereo encodes never touch these.

use super::LcEncoder;
use super::rate::max_frame_bits;
use crate::engine::error::Result;
use crate::engine::swb::LONG_WINDOW_LEN;

/// One element's analyzed frame.
pub(crate) type Specs = [[f32; LONG_WINDOW_LEN]; 2];

impl LcEncoder {
    /// Quality VBR is active (a fixed offset; no shared rate search).
    pub(crate) fn is_quality(&self) -> bool {
        self.quality.is_some()
    }

    /// Analysis up to (not including) rate control.
    pub(crate) fn mc_analyze(&mut self, pcm: &[&[f32]]) -> Result<Specs> {
        self.check_shape(pcm)?;
        let attack = self.detect(pcm);
        let specs = self.analyze(pcm, attack);
        self.cache_mags(&specs);
        Ok(specs)
    }

    /// Element bits at `offset` (includes this element's own END + pad).
    pub(crate) fn mc_bits(&mut self, specs: &Specs, offset: i32) -> usize {
        self.build(specs, offset)
    }

    /// Emit at `offset` into the element payload, dropping top bands
    /// until it fits `limit_bits` and the LC cap; no stuffing. Returns
    /// `(coded bits, all spectra quantized to zero)`.
    pub(crate) fn mc_emit(
        &mut self,
        specs: &Specs,
        offset: i32,
        limit_bits: usize,
    ) -> (usize, bool) {
        self.build(specs, offset);
        self.drop_bands_until(limit_bits.min(max_frame_bits(self.channels)));
        let mut payload = std::mem::take(&mut self.payload);
        self.emit_into(&mut payload);
        self.payload = payload;
        self.prev_seq = self.seq;
        (self.payload.len() * 8, self.quant_all_zero())
    }
}
