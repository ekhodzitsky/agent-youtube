//! Opt-in CBR builder of [`super::EncodeOptions`] (line cap).

impl super::EncodeOptions {
    /// Constant bitrate with a bit reservoir. Off by default, so ABR
    /// bytes and ADTS `adts_buffer_fullness = 0x7FF` stay unchanged.
    ///
    /// LC mono or stereo. A later frame may spend bits saved earlier,
    /// and no frame exceeds 6144 bits per channel. ADTS fullness is that
    /// reservoir in 32-bit words per channel, never the VBR marker
    /// `0x7FF`. Rejected together with HE, AAC-LD, quality VBR, or
    /// surround.
    ///
    /// ```
    /// use syom::{DecodeOptions, EncodeOptions, decode_with, encode_with};
    /// let pcm: Vec<f32> = (0..4096).map(|i| 0.2 * (i as f32 * 0.07).sin()).collect();
    /// let abr = encode_with(&[&pcm], 48_000, &EncodeOptions::adts())?;
    /// let cbr = encode_with(&[&pcm], 48_000, &EncodeOptions::adts().with_cbr(true))?;
    /// assert_ne!(abr, cbr);
    /// let dec = decode_with(&cbr, &DecodeOptions::unbounded())?;
    /// assert_eq!(dec.channels[0].len(), 4096);
    /// # Ok::<(), syom::AacError>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn with_cbr(mut self, on: bool) -> Self {
        self.cbr = on;
        self
    }
}
