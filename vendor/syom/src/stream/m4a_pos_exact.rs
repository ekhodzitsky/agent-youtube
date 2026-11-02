//! Conservative exact interval planning; no imported state or bounded-history claim.
use super::*;
use crate::{
    UnsupportedFeature,
    engine::asc::{AOT_LC, AOT_LD},
};
use std::ops::Range;

/// Opaque logical-position checkpoint bound to one source instance. This does
/// **not** contain codec history or authorize skipping bytes. Restoring replays
/// the prefix under the same budget as [`M4aSeek::seek_exact`]. It cannot be
/// deserialized, edited or transferred even to a new reader of identical bytes.
#[derive(Debug, Clone)]
pub struct ExactSeekPosition {
    source: std::sync::Arc<()>,
    sample: u64,
}
impl ExactSeekPosition {
    /// Absolute native presentation sample saved in this logical checkpoint.
    pub fn presentation_sample(&self) -> u64 {
        self.sample
    }
}

/// History required by an [`ExactSeekPlan`]. No bounded, history-independent
/// checkpoint/preroll path is currently qualified by this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExactSeekHistory {
    /// The request is empty: no access-unit payload is needed.
    NoMedia,
    /// Output begins in coded AU 0, with the initial decoder state.
    StreamStart,
    /// Replay from coded AU 0, even on a previously decoded source.
    PrefixReplay,
}

/// Read requirements for `[start, end)` in absolute native presentation samples.
/// Sample 0 follows the supported edit/priming trim. This borrows the same
/// source's parsed index; it is not an executable or transferable checkpoint.
///
/// Ranges cover full AAC payloads, including prefix history, **after open**.
/// Opening still needs container headers/index metadata. Ranges are in AU order,
/// may be disjoint/overlapping, and are not HTTP ranges or SIDX segment IDs.
/// No media is read or allocated by planning. Source bytes must remain immutable
/// throughout this source's lifetime. Actual frame geometry is checked during
/// [`M4aSeek::seek_exact`] replay and subsequent [`M4aSeek::decode`].
#[derive(Debug)]
pub struct ExactSeekPlan<'a> {
    start: u64,
    end: u64,
    rate: u32,
    replay: u64,
    frames: &'a [(u64, u32)],
}

impl ExactSeekPlan<'_> {
    /// Absolute native presentation coordinates; no resampling is performed.
    pub fn presentation_samples(&self) -> Range<u64> {
        self.start..self.end
    }
    /// Declared native rate for the supported LC/LD configuration. In-band HE
    /// or inconsistent geometry is rejected when encountered, not approximated.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// Decoder-history requirement, independent of prior seeks or warm state.
    pub fn history(&self) -> ExactSeekHistory {
        if self.frames.is_empty() {
            ExactSeekHistory::NoMedia
        } else if self.replay == 0 {
            ExactSeekHistory::StreamStart
        } else {
            ExactSeekHistory::PrefixReplay
        }
    }
    /// AUs decoded and discarded before the AU containing `start`.
    pub fn replay_access_units(&self) -> u64 {
        self.replay
    }
    /// All required coded AUs, including the AUs delivering requested samples.
    pub fn access_units(&self) -> Range<u64> {
        0..self.frames.len() as u64
    }
    /// Exact payload byte ranges in decode order, excluding container metadata.
    pub fn media_ranges(&self) -> impl ExactSizeIterator<Item = Range<u64>> + '_ {
        self.frames.iter().map(|&(at, len)| at..at + u64::from(len))
    }
}

impl<R: Read + Seek> M4aSeek<R> {
    fn exact_scope(&self) -> Result<()> {
        if !matches!(self.asc.aot, AOT_LC | AOT_LD)
            || !matches!(self.asc.channel_configuration, 1 | 2)
            || self.asc.sbr_present
            || self.dec.sbr_active
            || self.asc.ps_present
            || self.asc.output_sample_rate != self.asc.sample_rate
        {
            return Err(AacError::Unsupported(UnsupportedFeature::ExactM4aSeek));
        }
        Ok(())
    }

    /// Save a logical position, not a reusable decoder-state snapshot.
    pub fn save_exact_position(&self) -> Result<ExactSeekPosition> {
        self.exact_scope()?;
        if self.exact_failed {
            return Err(AacError::Lifecycle {
                state: crate::LifecycleState::Failed,
            });
        }
        Ok(ExactSeekPosition {
            source: self.source_identity.clone(),
            sample: self.pos,
        })
    }

    /// Restore this instance's saved position with full, budgeted prefix replay.
    /// Foreign checkpoints are rejected before I/O or state changes. No codec
    /// checkpoint import/serialization or bounded-history restore is provided.
    pub fn restore_exact_position(
        &mut self,
        position: &ExactSeekPosition,
        max_replay_aus: u64,
    ) -> Result<u64> {
        if !std::sync::Arc::ptr_eq(&self.source_identity, &position.source) {
            return Err(AacError::ForeignSeekPosition);
        }
        let at = i64::try_from(position.sample).map_err(|_| {
            AacError::invalid_limits("saved position exceeds exact seek coordinates")
        })?;
        self.seek_exact(at, max_replay_aus)
    }

    /// Plan an exact interval without reading media or changing position.
    ///
    /// Only configured LC (1024/960) and LD (512/480), mono/stereo, are in
    /// scope. Every nonempty plan includes the prefix from AU 0; metadata,
    /// SIDX, PNS-off encoder options or a prior successful seek do not prove a
    /// bounded independent window. HE/PS, prediction profiles and unqualified
    /// layouts return [`UnsupportedFeature::ExactM4aSeek`]. A decoded in-band
    /// extension or format inconsistency also fails explicitly during execution.
    pub fn exact_seek_plan(&self, start: i64, end: i64) -> Result<ExactSeekPlan<'_>> {
        self.exact_scope()?;
        if start < 0 || end < start || end as u64 > self.play {
            return Err(AacError::invalid_limits(
                "invalid exact M4A presentation interval",
            ));
        }
        let (start, end) = (start as u64, end as u64);
        let coded_start = self
            .skip
            .checked_add(start)
            .ok_or_else(|| AacError::format("exact seek sample overflow"))?;
        let coded_end = self
            .skip
            .checked_add(end)
            .ok_or_else(|| AacError::format("exact seek sample overflow"))?;
        let stop = if start == end {
            0
        } else {
            coded_end.div_ceil(self.samples_per_au)
        };
        let stop =
            usize::try_from(stop).map_err(|_| AacError::format("exact seek index overflow"))?;
        let frames = self
            .track
            .frames
            .get(..stop)
            .ok_or_else(|| AacError::format("exact interval exceeds coded samples"))?;
        if frames
            .iter()
            .any(|&(at, len)| at.checked_add(u64::from(len)).is_none())
        {
            return Err(AacError::format("exact seek byte range overflow"));
        }
        Ok(ExactSeekPlan {
            start,
            end,
            rate: self.asc.output_sample_rate,
            replay: if start == end {
                0
            } else {
                coded_start / self.samples_per_au
            },
            frames,
        })
    }

    /// Seek with exact initial-state replay, enforcing the discarded-AU budget
    /// **before any I/O or state change**. Use `u64::MAX` to permit full replay.
    /// This budget excludes the target/output AUs; use [`Self::exact_seek_plan`]
    /// to learn all payload ranges for an interval.
    ///
    /// A refused budget returns [`AacError::SeekReplayRequired`]. Missing prefix
    /// bytes return the reader's I/O error, never short-preroll approximation.
    /// There is no codec-state import/restore API: reopening at a saved position
    /// needs replay again. After a codec/read error, call a successful seek to
    /// reset before decoding again. Callback errors may stop an interval normally.
    pub fn seek_exact(&mut self, presentation_sample: i64, max_replay_aus: u64) -> Result<u64> {
        let end = if presentation_sample >= 0 && presentation_sample as u64 == self.play {
            presentation_sample
        } else {
            presentation_sample.checked_add(1).ok_or_else(|| {
                AacError::invalid_limits("exact seek position cannot form an interval")
            })?
        };
        let plan = self.exact_seek_plan(presentation_sample, end)?;
        let pres = plan.start;
        let target = plan.replay;
        if target > max_replay_aus {
            return Err(AacError::SeekReplayRequired {
                required_aus: target,
                max_aus: max_replay_aus,
            });
        }
        self.reset_codec();
        self.exact_mode = true;
        self.pos = pres;
        self.play_left = self.play - pres;
        #[cfg(test)]
        {
            self.last_seek_aus = 0;
        }
        if pres == self.play {
            self.next_frame = self.track.frames.len();
            self.discard_left = 0;
            return Ok(pres);
        }
        for idx in 0..target as usize {
            self.decode_au(idx)?;
            #[cfg(test)]
            {
                self.last_seek_aus += 1;
            }
        }
        self.next_frame = target as usize;
        self.discard_left = ((self.skip + pres) % self.samples_per_au) as usize;
        Ok(pres)
    }

    pub(super) fn check_exact_frame(&self, rate: u32, n: usize) -> Result<()> {
        if self.dec.sbr_active
            || rate != self.asc.output_sample_rate
            || n as u64 != self.samples_per_au
            || (!self.mono
                && self.dec.frame_planes().len() != usize::from(self.asc.channel_configuration))
        {
            return Err(AacError::Unsupported(UnsupportedFeature::ExactM4aSeek));
        }
        Ok(())
    }
}
