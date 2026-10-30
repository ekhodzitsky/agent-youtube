//! Surround path of the push [`Encoder`] (TASK-116): 3, 4, 5, 6 or 8
//! planes in the public decode order (3.0 FL FR FC, 4.0 + BC, 5.0 FL FR
//! FC BL BR, 5.1 FL FR FC LFE BL BR, 7.1 FL FR FC LFE BL BR SL SR),
//! `channel_configuration` 3–7, AAC-LC ABR or quality VBR. One access
//! unit per 1024 samples per plane, then one drain unit at finish.

use super::{EncodeInfo, EncodedFrame, Encoder, FRAME};
use crate::engine::enc_mc::McEncoder;
use crate::error::{AacError, Result};
use crate::options::EncodeOptions;

pub(super) struct McCore {
    pub(super) enc: McEncoder,
    /// Samples buffered across feeds, one vector per plane (< 1024 each).
    pending: Vec<Vec<f32>>,
    au: Vec<u8>,
}

impl McCore {
    pub(super) fn new(sample_rate: u32, planes: usize, opts: &EncodeOptions) -> Result<Self> {
        let enc = McEncoder::new_with(sample_rate, planes, opts.bitrate_bps, |rate, ch, bps| {
            let share = opts.clone().with_bitrate_bps(bps);
            crate::encode::new_lc(rate, ch, &share)
                .map_err(|_| crate::engine::error::Error::Format("LC encoder: element setup"))
        })?;
        Ok(Self {
            enc,
            pending: vec![Vec::with_capacity(FRAME); planes],
            au: Vec::new(),
        })
    }

    pub(super) fn reset(&mut self) {
        self.enc.reset();
        self.pending.iter_mut().for_each(Vec::clear);
    }
}

impl Encoder {
    fn mc(&mut self) -> Result<&mut McCore> {
        match &mut self.enc {
            super::Core::Mc(m) => Ok(m),
            _ => Err(AacError::Lifecycle {
                state: crate::LifecycleState::Failed,
            }),
        }
    }

    /// Encode the pending frame (zero-padded to 1024) and deliver it.
    fn emit_mc<F>(&mut self, samples: usize, cb: &mut F) -> Result<()>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let m = self.mc()?;
        let mut au = std::mem::take(&mut m.au);
        for p in &mut m.pending {
            p.resize(FRAME, 0.0);
        }
        let planes: Vec<&[f32]> = m.pending.iter().map(Vec::as_slice).collect();
        let r = m.enc.encode_into(&planes, &mut au);
        m.pending.iter_mut().for_each(Vec::clear);
        let mut scratch = std::mem::take(&mut self.scratch);
        let r = r
            .map_err(AacError::from)
            .and_then(|()| self.deliver(&au, samples, &mut scratch, cb));
        self.scratch = scratch;
        self.mc()?.au = au;
        r
    }

    pub(super) fn feed_mc<F>(&mut self, planes: &[&[f32]], mut cb: F) -> Result<usize>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let n = planes.first().map_or(0, |p| p.len());
        let mut off = 0usize;
        while off < n {
            let m = self.mc()?;
            let take = (FRAME - m.pending[0].len()).min(n - off);
            for (pend, plane) in m.pending.iter_mut().zip(planes.iter()) {
                pend.extend_from_slice(&plane[off..off + take]);
            }
            off += take;
            if m.pending[0].len() == FRAME
                && let Err(e) = self.emit_mc(FRAME, &mut cb)
            {
                self.samples += off as u64;
                return self.fail(e);
            }
        }
        self.samples += n as u64;
        Ok(n)
    }

    pub(super) fn finish_mc<F>(&mut self, mut cb: F) -> Result<EncodeInfo>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let tail = self.mc()?.pending[0].len();
        if tail > 0
            && let Err(e) = self.emit_mc(tail, &mut cb)
        {
            return self.fail(e);
        }
        // Drain: one block of zeros so the last samples overlap-add.
        if let Err(e) = self.emit_mc(0, &mut cb) {
            return self.fail(e);
        }
        let block = FRAME as u64;
        let config = self.mc()?.enc.channel_configuration();
        self.life = super::Life::Finished;
        Ok(EncodeInfo {
            sample_rate: self.sample_rate,
            channels: self.channels,
            aac_frames: self.aac_frames,
            samples: self.samples,
            bytes: self.bytes,
            priming: block,
            remainder: (block - (self.samples % block)) % block,
            coded_samples: self.aac_frames * block,
            layout: crate::Layout::Mpeg(config),
        })
    }
}
