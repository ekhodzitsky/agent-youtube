//! HE v1 path of the push [`Encoder`] (TASK-90): the engine buffers PCM
//! itself and hands back one access unit per 2048 input samples; this
//! file collects them, attributes source samples, and reports the
//! output-rate timeline (priming / remainder) in [`EncodeInfo`].

use super::{EncodeInfo, EncodedFrame, Encoder};
use crate::engine::enc_frame::LcEncoder;
use crate::engine::enc_he::OUT_SAMPLES_PER_AU;
use crate::engine::enc_he_ps::AnyHe;
use crate::error::{AacError, Result};

/// The engine behind the push encoder.
pub(super) enum Core {
    Lc(Box<LcEncoder>),
    He(Box<AnyHe>),
    /// Surround LC (TASK-116): the engine plus PCM pending across feeds.
    Mc(Box<super::mc::McCore>),
    /// AAC-LD, 512-sample frames.
    Ld(Box<crate::engine::enc_ld::LdEncoder>),
}

impl Core {
    pub(super) fn fs_index(&self) -> u8 {
        match self {
            Core::Lc(e) => e.fs_index(),
            Core::He(e) => e.fs_index(),
            Core::Mc(m) => m.enc.fs_index(),
            Core::Ld(e) => e.fs_index(),
        }
    }

    /// `channel_configuration` for `channels` input planes.
    pub(super) fn channel_config(&self, channels: usize) -> usize {
        match self {
            Core::Mc(m) => usize::from(m.enc.channel_configuration()),
            Core::He(e) => e.core_channels(channels),
            Core::Lc(_) | Core::Ld(_) => channels,
        }
    }

    pub(super) fn reset(&mut self) {
        match self {
            Core::Lc(e) => e.reset(),
            Core::He(e) => e.reset(),
            Core::Mc(m) => m.reset(),
            Core::Ld(e) => e.reset(),
        }
    }
}

impl Encoder {
    /// The LC engine; the HE engine has its own path.
    pub(super) fn lc(&mut self) -> Result<&mut LcEncoder> {
        match &mut self.enc {
            Core::Lc(e) => Ok(e),
            Core::He(_) | Core::Mc(_) | Core::Ld(_) => Err(AacError::Lifecycle {
                state: crate::LifecycleState::Failed,
            }),
        }
    }

    /// Collect the engine's access units into the reusable list.
    fn collect_he(
        &mut self,
        planes: Option<&[&[f32]]>,
    ) -> Result<Option<crate::engine::enc_he::HeInfo>> {
        let (aus, mut n) = (&mut self.he_aus, 0usize);
        let sink = |au: &[u8]| {
            if n == aus.len() {
                aus.push(Vec::new());
            }
            aus[n].clear();
            aus[n].extend_from_slice(au);
            n += 1;
            Ok(())
        };
        let Core::He(he) = &mut self.enc else {
            return Err(AacError::Lifecycle {
                state: crate::LifecycleState::Failed,
            });
        };
        let info = match planes {
            Some(p) => {
                he.push(p, sink)?;
                None
            }
            None => Some(he.finish(sink)?),
        };
        self.he_n = n;
        Ok(info)
    }

    /// Deliver the collected units; each carries up to 2048 not yet
    /// attributed source samples (0 for drain units).
    fn deliver_he<F>(&mut self, cb: &mut F) -> Result<()>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let mut scratch = std::mem::take(&mut self.scratch);
        let aus = std::mem::take(&mut self.he_aus);
        let mut r = Ok(());
        for au in &aus[..self.he_n] {
            let left = self.samples.saturating_sub(self.attributed);
            let samples = left.min(OUT_SAMPLES_PER_AU) as usize;
            self.attributed += samples as u64;
            r = self.deliver(au, samples, &mut scratch, cb);
            if r.is_err() {
                break;
            }
        }
        self.he_aus = aus;
        self.he_n = 0;
        self.scratch = scratch;
        r
    }

    pub(super) fn feed_he<F>(&mut self, planes: &[&[f32]], mut cb: F) -> Result<usize>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let n = planes.first().map_or(0, |p| p.len());
        self.samples += n as u64;
        if let Err(e) = self.collect_he(Some(planes)) {
            return self.fail(e);
        }
        if let Err(e) = self.deliver_he(&mut cb) {
            return self.fail(e);
        }
        Ok(n)
    }

    pub(super) fn finish_he<F>(&mut self, mut cb: F) -> Result<EncodeInfo>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let info = match self.collect_he(None) {
            Ok(Some(i)) => i,
            Ok(None) => {
                return self.fail(AacError::Lifecycle {
                    state: crate::LifecycleState::Failed,
                });
            }
            Err(e) => return self.fail(e),
        };
        if let Err(e) = self.deliver_he(&mut cb) {
            return self.fail(e);
        }
        self.life = super::Life::Finished;
        Ok(EncodeInfo {
            sample_rate: self.sample_rate,
            channels: self.channels,
            aac_frames: self.aac_frames,
            samples: self.samples,
            bytes: self.bytes,
            priming: info.priming_out,
            remainder: info.remainder_out,
            coded_samples: info.aus * OUT_SAMPLES_PER_AU,
            layout: crate::Layout::Mpeg(self.channels as u8),
        })
    }
}
