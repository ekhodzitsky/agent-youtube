//! AAC-LD builder of [`super::EncodeOptions`].

impl super::EncodeOptions {
    /// ER AAC-LD: 512-sample frames, mono or stereo, LOAS or M4A.
    /// Off by default, so [`crate::encode`] stays AAC-LC. ADTS is an
    /// error (the header cannot signal AOT 23). Priming is one frame.
    /// HE, lookahead, quality VBR and the LC-only tools must stay off.
    ///
    /// ```
    /// use syom::{DecodeOptions, EncodeContainer, EncodeOptions, decode_with, encode_with};
    /// let pcm = vec![0.2f32; 2048];
    /// let opts = EncodeOptions::default()
    ///     .with_container(EncodeContainer::Latm)
    ///     .with_ld(true)
    ///     .with_bitrate_bps(64_000);
    /// let loas = encode_with(&[&pcm[..]], 48_000, &opts)?;
    /// let dec = decode_with(&loas, &DecodeOptions::unbounded())?;
    /// assert_eq!((dec.sample_rate, dec.channels[0].len()), (48_000, 2560));
    /// # Ok::<(), syom::AacError>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn with_ld(mut self, on: bool) -> Self {
        self.ld = on;
        self
    }
}
