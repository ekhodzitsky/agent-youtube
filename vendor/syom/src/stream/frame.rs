//! Streaming frame types: the borrowed [`Frame`] delivered to the callback
//! and the [`StreamInfo`] tallies returned at end of stream.

use crate::layout::{FrameMeta, Layout};

/// One decoded AAC frame: planar f32 in [-1, 1], one plane per channel.
///
/// The planes borrow decoder scratch and are valid **only for the duration
/// of the frame callback** — copy them out to keep them. [`Self::meta`] is
/// `Copy` (no per-frame heap).
#[non_exhaustive]
pub struct Frame<'a> {
    /// Native sample rate after SBR (2× the core rate for HE-AAC).
    pub sample_rate: u32,
    /// Per-channel sample count in this frame.
    pub samples: usize,
    /// Planes in the same channel order as [`crate::DecodedAac`].
    pub planar: &'a [&'a [f32]],
    /// Channel labels, core/output rate, layout (TASK-61).
    pub meta: FrameMeta,
}

/// Tallies from a finished stream decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamInfo {
    /// Native sample rate of the stream (0 if nothing decodable was seen).
    pub sample_rate: u32,
    /// Core AAC rate (before SBR). `0` if nothing decodable was seen.
    pub core_rate: u32,
    /// Channels per emitted frame (0 if nothing decodable was seen).
    pub channels: usize,
    /// How the planes were produced.
    pub layout: Layout,
    /// Payload frames decoded, including edit-list-skipped ones.
    pub aac_frames: u64,
    /// Output samples per channel after an edit-list or `iTunSMPB` trim.
    pub samples: u64,
    /// Encoder delay skipped at the start, when an edit list or a matching
    /// `iTunSMPB` tag says so. A push ADTS feed applies a modest tag as
    /// frames arrive. These stay [`None`] when the tag is absent or the
    /// decoded count does not match it.
    pub priming: Option<u64>,
    /// Unplayed coded tail, under the same rule as [`Self::priming`].
    pub remainder: Option<u64>,
}
