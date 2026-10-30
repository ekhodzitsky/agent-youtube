//! ADTS `iTunSMPB` window. The tag is peeled before the one-byte resync.
//! One-shot decode arms the window only when the decoded length equals
//! priming + source + remainder. A push feed arms a modest tag before
//! that check and ignores an edge above [`gapless::MAX_PUSH_GAP`].
//! LATM never sets this state.

use crate::engine::adts::AdtsHeader;
use crate::engine::error::Error as EngineError;
use crate::error::{AacError, Result};
use crate::gapless::{self, Delay, Id3At};

use super::Decoder;

/// Skip / play cursors for one ADTS stream, plus the tag that armed them.
#[derive(Clone, Debug, Default)]
pub(super) struct AdtsGap {
    id3_done: bool,
    /// Counted on a final flush; `None` until then (or after the window arms).
    predict: Option<u64>,
    candidate: Option<Delay>,
    windowed: bool,
    skip_left: usize,
    play_left: Option<usize>,
    tag_priming: u64,
    tag_remainder: u64,
    tag_source: u64,
}

impl AdtsGap {
    /// `true` means the caller should wait for more bytes of the tag.
    /// A declared tag larger than `tag_budget` is [`AacError::Limit`]
    /// and is not allocated. A complete tag does not hold the ADTS
    /// frames behind it.
    pub(super) fn hold(
        &mut self,
        buf: &[u8],
        pos: &mut usize,
        final_flush: bool,
        tag_budget: u64,
    ) -> Result<bool> {
        if !self.id3_done {
            let avail = &buf[*pos..];
            match gapless::id3_at(avail, tag_budget)? {
                Id3At::Need if !final_flush => return Ok(true),
                Id3At::Need if avail.len() >= 3 && avail.starts_with(b"ID3") => {
                    return Err(AacError::truncated_at(Some(*pos as u64)));
                }
                Id3At::Need => self.id3_done = true,
                Id3At::Ready { len, delay } => {
                    *pos += len;
                    self.id3_done = true;
                    self.candidate = delay;
                }
                Id3At::Absent => self.id3_done = true,
            }
        }
        if self.candidate.is_some() && !self.windowed && self.predict.is_none() {
            if final_flush {
                // The whole buffer is here: arm only when the tag sums to
                // the access units that follow.
                self.predict = Some(count_frames(&buf[*pos..]));
                return Ok(false);
            }
            // Push feed. A modest tag is applied as frames arrive. An
            // oversized edge is dropped; holding the file to check it
            // would grow the resident buffer to the input cap.
            let modest = self.candidate.is_some_and(|d| {
                d.priming <= gapless::MAX_PUSH_GAP && d.remainder <= gapless::MAX_PUSH_GAP
            });
            if modest {
                self.trust();
            } else {
                self.candidate = None;
            }
        }
        Ok(false)
    }

    /// Arm from the first decoded frame's sample count. A mismatch leaves
    /// the window off so the padded PCM is returned.
    pub(super) fn arm(&mut self, spf: usize) {
        if self.windowed {
            return;
        }
        let (Some(frames), Some(d)) = (self.predict.take(), self.candidate.take()) else {
            return;
        };
        let coded = frames.saturating_mul(spf as u64);
        let sum = d
            .priming
            .saturating_add(d.source)
            .saturating_add(d.remainder);
        if spf == 0 || coded != sum {
            return;
        }
        self.install(d);
    }

    /// `(start, end)` inside a frame of `n` samples. `(0, n)` when no tag
    /// was accepted. `(0, 0)` drops the frame; the caller still counts `n`.
    pub(super) fn take(&mut self, n: usize) -> (usize, usize) {
        if !self.windowed {
            return (0, n);
        }
        if self.skip_left >= n {
            self.skip_left -= n;
            return (0, 0);
        }
        let start = self.skip_left;
        self.skip_left = 0;
        let mut take = n - start;
        if let Some(left) = self.play_left.as_mut() {
            if *left == 0 {
                return (0, 0);
            }
            take = take.min(*left);
            *left -= take;
        }
        (start, start + take)
    }

    /// Priming and remainder once `samples_decoded` matches the tag.
    pub(super) fn report(&self, samples_decoded: u64) -> (Option<u64>, Option<u64>) {
        if !self.windowed {
            return (None, None);
        }
        let sum = self
            .tag_priming
            .saturating_add(self.tag_source)
            .saturating_add(self.tag_remainder);
        if samples_decoded == sum {
            (Some(self.tag_priming), Some(self.tag_remainder))
        } else {
            (None, None)
        }
    }

    fn trust(&mut self) {
        let Some(d) = self.candidate.take() else {
            return;
        };
        self.install(d);
    }

    fn install(&mut self, d: Delay) {
        let (Ok(skip), Ok(play)) = (usize::try_from(d.priming), usize::try_from(d.source)) else {
            return;
        };
        self.skip_left = skip;
        self.play_left = Some(play);
        self.tag_priming = d.priming;
        self.tag_remainder = d.remainder;
        self.tag_source = d.source;
        self.windowed = true;
    }
}

fn count_frames(data: &[u8]) -> u64 {
    let mut pos = 0usize;
    let mut n = 0u64;
    while pos < data.len() {
        match AdtsHeader::parse(&data[pos..]) {
            Ok((hdr, _)) => {
                let len = usize::from(hdr.aac_frame_length);
                if len == 0 || pos.saturating_add(len) > data.len() {
                    break;
                }
                pos += len;
                n += 1;
            }
            Err(EngineError::AdtsSyncNotFound) => pos += 1,
            Err(_) => break,
        }
    }
    n
}

impl Decoder {
    /// One-shot mono: decode into `dst`, then keep only the gapless window.
    pub(super) fn adts_into(
        &mut self,
        dst: &mut Vec<f32>,
        hdr: &AdtsHeader,
        payload: &[u8],
        est_frames: Option<usize>,
    ) -> Result<u32> {
        self.check_sink_growth(dst.len())?;
        let before = dst.len();
        let rate = self
            .dec
            .decode_adts_header_payload(hdr, payload, Some(dst))
            .map_err(AacError::from)?;
        let n = dst.len() - before;
        self.tally_decoded(rate, 1, n)?;
        if self.aac_frames == 0 {
            self.gap.arm(n);
            self.reserve_hint(dst, est_frames, n)?;
        }
        let (start, end) = self.gap.take(n);
        self.sink_frame_samples = n;
        if end > start {
            let take = end - start;
            if start != 0 {
                dst.copy_within(before + start..before + end, before);
            }
            dst.truncate(before + take);
            self.samples_out += take as u64;
            self.emitted += 1;
        } else {
            dst.truncate(before);
        }
        self.opts.memory.check_output(1, dst.len() as u64)?;
        Ok(rate)
    }
}
