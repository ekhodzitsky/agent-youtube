//! [`DecodeOptions`] validation and frame budget (line cap).

use super::DecodeOptions;

impl DecodeOptions {
    /// Reject limits that would disable protection or are not a duration.
    ///
    /// [`f64::INFINITY`] is the explicit unlimited contract used by
    /// [`Self::unbounded`]. NaN, negative infinity and negative durations
    /// error. `max_sample_rate == 0` and a zero decode-rate cap with a
    /// finite duration also error.
    pub fn validate(&self) -> crate::Result<()> {
        let d = self.max_duration_secs;
        if d.is_nan() {
            return Err(crate::AacError::invalid_limits("max_duration_secs is NaN"));
        }
        if d.is_infinite() && d < 0.0 {
            return Err(crate::AacError::invalid_limits("max_duration_secs is -inf"));
        }
        if d.is_finite() && d < 0.0 {
            return Err(crate::AacError::invalid_limits(
                "max_duration_secs is negative",
            ));
        }
        if self.max_sample_rate == 0 {
            return Err(crate::AacError::invalid_limits("max_sample_rate is 0"));
        }
        if self.max_decode_sample_rate == 0 && d.is_finite() {
            return Err(crate::AacError::invalid_limits(
                "max_decode_sample_rate is 0",
            ));
        }
        self.memory
            .validate()
            .map_err(|e| crate::AacError::invalid_limits(format!("{} budget is 0", e.kind)))?;
        Ok(())
    }

    /// Maximum decoded samples per channel at `sample_rate` under these options.
    ///
    /// Call [`Self::validate`] before decoding. Invalid durations yield 0
    /// here so a missed check cannot open an unlimited cap.
    #[inline]
    pub fn max_frames(&self, sample_rate: u32) -> usize {
        let d = self.max_duration_secs;
        if d.is_infinite() && d > 0.0 {
            return usize::MAX;
        }
        if !d.is_finite() || d <= 0.0 {
            return 0;
        }
        let cap = self.max_decode_sample_rate.max(1);
        let rate = sample_rate.min(cap) as f64;
        let frames = d * rate;
        if !frames.is_finite() || frames >= usize::MAX as f64 {
            return usize::MAX;
        }
        frames as usize
    }
}
