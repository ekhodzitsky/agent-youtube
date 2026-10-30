//! Public channel layout and per-frame Copy metadata (TASK-61).
//!
//! No heap: a stable format is an 8-slot `[Option<Channel>; 8]`. Priming
//! and remainder live on the decode tallies.

/// Product plane ceiling ([`crate::DEFAULT_MAX_CHANNELS`]).
pub const MAX_PLANES: usize = 8;

/// One output plane's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Channel {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    BackLeft,
    BackRight,
    BackCenter,
    SideLeft,
    SideRight,
    /// PCE leftover, speech downmix, or an unlabeled extra element.
    #[default]
    Other,
}

/// How the plane vector was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Layout {
    /// ISO Table 4.1 `channel_configuration` 1–7.
    Mpeg(u8),
    /// PCE declaration order (front, side, back, LFE).
    Pce,
    /// [`crate::ChannelMode::Mono`]: mean of non-LFE planes.
    SpeechMono,
    /// Config 0 without PCE, or extra elements appended.
    #[default]
    Unspecified,
}

/// Copy metadata for one decoded frame or a finished stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FrameMeta {
    pub layout: Layout,
    /// AAC core rate (before SBR upsample). `0` if nothing was decoded.
    pub core_rate: u32,
    /// Presentation rate (2× core for HE). Matches [`super::Frame::sample_rate`].
    pub output_rate: u32,
    /// Per-plane labels; unused slots are `None`.
    pub channels: [Option<Channel>; MAX_PLANES],
}

impl FrameMeta {
    pub const EMPTY: Self = Self {
        layout: Layout::Unspecified,
        core_rate: 0,
        output_rate: 0,
        channels: [None; MAX_PLANES],
    };

    #[must_use]
    pub fn from_labels(
        layout: Layout,
        core_rate: u32,
        output_rate: u32,
        labels: &[Channel],
    ) -> Self {
        let mut channels = [None; MAX_PLANES];
        for (slot, &c) in channels.iter_mut().zip(labels.iter()) {
            *slot = Some(c);
        }
        Self {
            layout,
            core_rate,
            output_rate,
            channels,
        }
    }

    #[must_use]
    pub fn speech_mono(core_rate: u32, output_rate: u32) -> Self {
        Self::from_labels(
            Layout::SpeechMono,
            core_rate,
            output_rate,
            &[Channel::Other],
        )
    }

    /// Labels of live planes, in output order.
    pub fn labels(&self) -> impl Iterator<Item = Channel> + '_ {
        self.channels.iter().copied().flatten()
    }

    /// Index of the LFE plane, if any.
    #[must_use]
    pub fn lfe_index(&self) -> Option<usize> {
        self.channels.iter().position(|&c| c == Some(Channel::Lfe))
    }
}

/// Per-frame metadata from the decode-side facts.
#[must_use]
pub(crate) fn frame_meta(
    speech: bool,
    pce: bool,
    cfg: u8,
    core_rate: u32,
    output_rate: u32,
    labels: &[Channel],
) -> FrameMeta {
    if speech {
        return FrameMeta::speech_mono(core_rate, output_rate);
    }
    let layout = if pce { Layout::Pce } else { mpeg_layout(cfg) };
    FrameMeta::from_labels(layout, core_rate, output_rate, labels)
}

/// [`Layout::Mpeg`] for the default configurations 1–7, else
/// [`Layout::Unspecified`].
#[must_use]
pub(crate) fn mpeg_layout(cfg: u8) -> Layout {
    if (1..=7).contains(&cfg) {
        Layout::Mpeg(cfg)
    } else {
        Layout::Unspecified
    }
}

/// MPEG default-layout labels (lavc / Table 4.1 order). Empty if `cfg` is
/// not 1–7. Config 7 (7.1) is FL FR FC LFE BL BR SL SR: ISO calls the
/// third CPE "left/right outside front"; lavc presents it as the side
/// pair, and so does syom.
#[must_use]
pub fn mpeg_channels(cfg: u8) -> &'static [Channel] {
    use Channel::{
        BackCenter, BackLeft, BackRight, FrontCenter, FrontLeft, FrontRight, Lfe, SideLeft,
        SideRight,
    };
    match cfg {
        1 => &[FrontCenter],
        2 => &[FrontLeft, FrontRight],
        3 => &[FrontLeft, FrontRight, FrontCenter],
        4 => &[FrontLeft, FrontRight, FrontCenter, BackCenter],
        5 => &[FrontLeft, FrontRight, FrontCenter, BackLeft, BackRight],
        6 => &[FrontLeft, FrontRight, FrontCenter, Lfe, BackLeft, BackRight],
        7 => &[
            FrontLeft,
            FrontRight,
            FrontCenter,
            Lfe,
            BackLeft,
            BackRight,
            SideLeft,
            SideRight,
        ],
        _ => &[],
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;
