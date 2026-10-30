//! TASK-61: last-frame Copy metadata on [`super::StreamDecoder`].

use super::super::channel_map::PlaneMap;
use super::super::error::{Error, Result};
use super::StreamDecoder;
use crate::layout::{self, Channel, FrameMeta};

/// LC / LD / SBR / PS object types; `channel_configuration` 0–7 (8–15 are the
/// 6.1 / 7.1-back / 7.1-top / reserved arrangements, TASK-62).
pub(super) fn check_config(aot: u8, cfg: u8) -> Result<()> {
    if !matches!(aot, 1 | 2 | 3 | 4 | 5 | 23 | 29) {
        return Err(Error::UnsupportedAot(aot));
    }
    if cfg > 7 {
        return Err(Error::UnsupportedChannelConfiguration(cfg));
    }
    Ok(())
}

impl StreamDecoder {
    #[must_use]
    pub(crate) fn last_meta(&self) -> FrameMeta {
        self.last_meta
    }

    #[must_use]
    pub(crate) fn last_core_rate(&self) -> u32 {
        self.last_meta.core_rate
    }

    pub(super) fn store_meta(
        &mut self,
        cfg: u8,
        core: u32,
        out: u32,
        order: Option<&[PlaneMap]>,
        n: usize,
    ) {
        let n = n.min(layout::MAX_PLANES);
        let mut labs = [Channel::Other; layout::MAX_PLANES];
        match order {
            Some(o) => {
                for (d, s) in labs.iter_mut().zip(o.iter()).take(n) {
                    *d = s.label;
                }
            }
            None => {
                for (d, &s) in labs.iter_mut().zip(layout::mpeg_channels(cfg)).take(n) {
                    *d = s;
                }
            }
        }
        self.last_meta = layout::frame_meta(
            self.mix_down_mono,
            self.pce.is_some(),
            cfg,
            core,
            out,
            &labs[..n],
        );
    }
}
