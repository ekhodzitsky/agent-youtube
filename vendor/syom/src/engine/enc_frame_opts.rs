//! Opt-in tool switches of [`LcEncoder`] (line cap): short TNS, short
//! grouping, band refine, PNS, intensity stereo. All default off.

use super::LcEncoder;

impl LcEncoder {
    pub(crate) fn with_short_tns(mut self, on: bool) -> Self {
        self.short_tns = on;
        self
    }

    pub(crate) fn with_short_group(mut self, on: bool) -> Self {
        self.short_group = on;
        self
    }

    pub(crate) fn with_band_refine(mut self, on: bool) -> Self {
        self.band_refine = on;
        self
    }

    pub(crate) fn with_pns(mut self, on: bool) -> Self {
        self.pns = on;
        self
    }

    pub(crate) fn with_intensity(mut self, on: bool) -> Self {
        self.intensity = on;
        self
    }
}
