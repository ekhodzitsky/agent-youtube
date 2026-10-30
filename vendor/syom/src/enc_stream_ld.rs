//! AAC-LD path of the push [`Encoder`]: 512-sample frames, one trailing
//! zero frame so the last source frame overlap-adds, same bytes as one-shot.

use super::he::Core;
use super::{EncodeInfo, EncodedFrame, Encoder};
use crate::error::{AacError, Result};

const FRAME: usize = 512;

impl Encoder {
    pub(super) fn feed_ld<F>(&mut self, planes: &[&[f32]], mut on_frame: F) -> Result<usize>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let n = planes.first().map_or(0, |p| p.len());
        let mut off = 0usize;
        while self.pending_len + (n - off) >= FRAME {
            let take = FRAME - self.pending_len;
            let mut bufs = [[0.0f32; FRAME]; 2];
            for (ch, buf) in bufs.iter_mut().enumerate().take(self.channels) {
                buf[..self.pending_len].copy_from_slice(&self.pending[ch][..self.pending_len]);
                buf[self.pending_len..].copy_from_slice(&planes[ch][off..off + take]);
            }
            if let Err(e) = self.emit_ld(&bufs, FRAME, &mut on_frame) {
                self.pending_len = 0;
                self.samples += (off + take) as u64;
                return self.fail(e);
            }
            off += take;
            self.pending_len = 0;
        }
        if off < n {
            if self.pending_len == 0 {
                for (ch, plane) in planes.iter().enumerate() {
                    self.pending[ch].clear();
                    self.pending[ch].extend_from_slice(&plane[off..]);
                }
                self.pending_len = n - off;
            } else {
                for (ch, plane) in planes.iter().enumerate() {
                    self.pending[ch].extend_from_slice(&plane[off..]);
                }
                self.pending_len += n - off;
            }
        }
        self.samples += n as u64;
        Ok(n)
    }

    pub(super) fn finish_ld<F>(&mut self, mut on_frame: F) -> Result<EncodeInfo>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        if self.pending_len > 0 {
            let tail = self.pending_len;
            let mut bufs = [[0.0f32; FRAME]; 2];
            for (ch, buf) in bufs.iter_mut().enumerate().take(self.channels) {
                buf[..tail].copy_from_slice(&self.pending[ch][..tail]);
            }
            if let Err(e) = self.emit_ld(&bufs, tail, &mut on_frame) {
                return self.fail(e);
            }
            self.pending_len = 0;
        }
        let zeros = [[0.0f32; FRAME]; 2];
        if let Err(e) = self.emit_ld(&zeros, 0, &mut on_frame) {
            return self.fail(e);
        }
        let n = self.samples;
        let block = FRAME as u64;
        let remainder = (block - (n % block)) % block;
        self.life = super::Life::Finished;
        Ok(EncodeInfo {
            sample_rate: self.sample_rate,
            channels: self.channels,
            aac_frames: self.aac_frames,
            samples: n,
            bytes: self.bytes,
            priming: block,
            remainder,
            coded_samples: self.aac_frames * block,
            layout: crate::Layout::Mpeg(self.channels as u8),
        })
    }

    fn emit_ld<F>(&mut self, bufs: &[[f32; FRAME]; 2], samples: usize, cb: &mut F) -> Result<()>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let mut slots: [&[f32]; 2] = [&[], &[]];
        for (i, buf) in bufs.iter().enumerate().take(self.channels) {
            slots[i] = &buf[..];
        }
        let au = {
            let Core::Ld(ld) = &mut self.enc else {
                return Err(AacError::Lifecycle {
                    state: crate::LifecycleState::Failed,
                });
            };
            ld.push(&slots[..self.channels]).map_err(AacError::from)?
        };
        let mut scratch = std::mem::take(&mut self.scratch);
        let result = self.deliver(&au, samples, &mut scratch, cb);
        self.scratch = scratch;
        result
    }
}
