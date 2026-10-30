//! [`EncodeContainer`] — the encode output framing (line cap).

/// `#[non_exhaustive]`: further transports may follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EncodeContainer {
    /// ADTS elementary stream.
    #[default]
    Adts,
    /// LATM/LOAS (`AudioSyncStream`, one access unit per frame,
    /// `StreamMuxConfig` in every frame): the broadcast / RTP framing; the
    /// push [`crate::Encoder`] streams it (TASK-94).
    Latm,
    /// M4A / ISOBMFF (`ftyp` + `mdat` + `moov`).
    M4a,
    /// Raw `raw_data_block` access units (push [`crate::Encoder`] only).
    /// One-shot [`crate::encode_with`] rejects this: muxers need per-AU
    /// sizes. Pair with [`crate::Encoder::asc`] and
    /// [`crate::wrap_adts_au`] / [`crate::mux_raw_lc_m4a`].
    Raw,
}
