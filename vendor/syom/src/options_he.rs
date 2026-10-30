//! HE-AAC v2 builder of [`super::EncodeOptions`] (line cap, TASK-93).

impl super::EncodeOptions {
    /// HE-AAC v1 (SBR on an LC core at half the input rate). Off by
    /// default. One-shot and push encode honor it identically.
    ///
    /// ```
    /// use syom::{DecodeOptions, EncodeOptions, decode_with, encode_with, probe};
    /// let pcm = vec![vec![0.0f32; 48_000]; 2];
    /// let opts = EncodeOptions::adts().with_bitrate_bps(48_000).with_he(true);
    /// let adts = encode_with(&pcm, 48_000, &opts)?;
    /// assert_eq!(probe(&adts)?.meta.core_rate, 24_000); // ADTS header: LC at the core rate
    /// let dec = decode_with(&adts, &DecodeOptions::audio())?;
    /// assert_eq!((dec.sample_rate, dec.core_rate), (48_000, 24_000));
    /// # Ok::<(), syom::AacError>(())
    /// ```
    #[inline]
    pub fn with_he(mut self, on: bool) -> Self {
        self.he = on;
        self
    }

    /// HE-AAC v2 (TASK-93): **stereo** input is coded as one mono HE v1
    /// stream (LC core at half rate + SBR) plus a parametric-stereo
    /// payload — 20 bands of level difference and coherence per access
    /// unit. Meant for about 16–40 kbps whole-stream; above that HE v1
    /// stereo or LC keeps the real channels. Turns [`Self::he`] on as well;
    /// `false` clears both.
    ///
    /// Limits: the image is parametric (no phase cues; exact anti-phase
    /// content cancels in the mono core), spatial changes are followed
    /// within about one access unit, quality VBR does not apply. ADTS
    /// signals SBR and PS implicitly (mono header at the core rate); M4A,
    /// LATM and [`crate::Encoder::asc`] carry the explicit AOT 29 config.
    /// Timeline is HE v1's (3018 output samples of priming).
    ///
    /// ```
    /// use syom::{EncodeOptions, decode_with, DecodeOptions, encode_with};
    /// let l: Vec<f32> = (0..16_384).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
    /// let r: Vec<f32> = l.iter().map(|x| 0.25 * x).collect();
    /// let opts = EncodeOptions::adts().with_he_v2(true).with_bitrate_bps(32_000);
    /// let adts = encode_with(&[l, r], 48_000, &opts)?;
    /// let pcm = decode_with(&adts, &DecodeOptions::audio())?;
    /// assert_eq!((pcm.channels.len(), pcm.sample_rate), (2, 48_000));
    /// # Ok::<(), syom::AacError>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn with_he_v2(mut self, on: bool) -> Self {
        self.he = on;
        self.ps = on;
        self
    }

    /// TASK-134 measurement. Does not change [`Self::with_he_v2`]: `false`
    /// (the default) writes coarse `iid_mode` 1. `true` writes 20-band
    /// fine IID (`iid_mode` 4) on an HE v2 encode.
    #[doc(hidden)]
    #[inline]
    #[must_use]
    pub fn with_ps_iid_fine(mut self, on: bool) -> Self {
        self.ps_iid_fine = on;
        self
    }
}
