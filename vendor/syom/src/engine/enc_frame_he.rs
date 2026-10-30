//! HE hooks on [`LcEncoder`] (TASK-89): the SBR `extension_payload`
//! bytes emitted as a FIL before `END`, counted in the rate loop, and
//! the core band-limit at the SBR crossover. A child module of
//! `enc_frame`, like `rate`; LC-only encodes never touch these.

use super::LcEncoder;
use crate::engine::enc_sbr_bits::FILL_MAX_BYTES;
use crate::engine::error::{Error, Result};
use crate::engine::swb::LONG_WINDOW_LEN;

#[cfg_attr(not(test), allow(dead_code))]
impl LcEncoder {
    /// Attach the SBR `extension_payload` bytes for the next emitted
    /// frame (empty clears). Length is bounded by `fill_element` syntax.
    pub(crate) fn set_fill(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > FILL_MAX_BYTES {
            return Err(Error::SbrGridInvalid);
        }
        self.fill.clear();
        self.fill.extend_from_slice(payload);
        Ok(())
    }

    /// Bits the FIL element adds to the frame: `ID_FIL` + count (+ escape)
    /// + payload. Zero when no fill is attached.
    pub(crate) fn fill_bits(&self) -> usize {
        match self.fill.len() {
            0 => 0,
            n if n < 15 => 3 + 4 + 8 * n,
            n => 3 + 4 + 8 + 8 * n,
        }
    }

    /// LFE element policy (TASK-114): long windows only, no TNS, band
    /// limited to 120 Hz.
    pub(crate) fn set_lfe(&mut self) {
        self.block_switching = false;
        self.tns_enabled = false;
        self.set_cutoff_hz(Some(120));
    }

    /// Stop coding core bands at `hz` (the SBR crossover `k0`); `None`
    /// restores full band. Long and short psy share the rule.
    pub(crate) fn set_cutoff_hz(&mut self, hz: Option<u32>) {
        let bin = hz.map_or(0, |hz| {
            (u64::from(hz) * 2 * LONG_WINDOW_LEN as u64 / u64::from(self.sample_rate)) as usize
        });
        self.psy.set_cutoff_bin(bin);
        self.psy_short.set_cutoff_bin(bin / 8);
    }
}
