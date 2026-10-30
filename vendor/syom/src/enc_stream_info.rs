//! [`EncodeInfo`] — the finish tallies of the push [`super::Encoder`].

/// Tallies from a finished streaming encode.
///
/// Input / bitstream tallies plus the AAC timeline (Apple QA1636-style).
/// A one-shot ADTS encode stores these counts in a leading `iTunSMPB`
/// tag. [`super::Encoder`] feed callbacks stay raw access units. M4A
/// writes [`Self::priming`] as `elst.media_time`, [`Self::samples`] as
/// the `elst`/`mvhd` presentation duration, and [`Self::remainder`] as
/// the unplayed `mdhd` tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodeInfo {
    /// Input sample rate.
    pub sample_rate: u32,
    /// Channels per frame: 1–2, or surround 3, 4, 5, 6, or 8.
    pub channels: usize,
    /// AAC frames emitted, including the overlap-drain frame of zeros.
    pub aac_frames: u64,
    /// Input samples per channel consumed (source length).
    pub samples: u64,
    /// Bytes handed to the callback (ADTS headers included).
    pub bytes: u64,
    /// Encoder delay in decoded samples: 1024 (LC), 512 (AAC-LD), or 3018
    /// (HE v1 and v2, output rate). Skip this many decoded samples for
    /// valid audio.
    pub priming: u64,
    /// Unplayed decoded tail after the source. LC and AAC-LD pad up to the
    /// frame (1024 or 512); HE is `coded − priming − samples`.
    pub remainder: u64,
    /// Decoded length before container trim: `aac_frames` times 1024 (LC),
    /// 512 (AAC-LD), or 2048 (HE, output rate).
    pub coded_samples: u64,
    /// MPEG layout of the coded planes (`channel_configuration` 1–7).
    pub layout: crate::Layout,
}

impl super::Encoder {
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.aac_frames
    }

    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }

    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.life == super::Life::Failed
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.life == super::Life::Finished
    }
}
