//! Streaming encode: a resumable push [`Encoder`] for planar f32 PCM →
//! AAC-LC, HE-AAC, or AAC-LD in ADTS, LOAS, or raw access units.
//!
//! Feed PCM in chunks of any size; the callback fires once per completed
//! frame (1024 LC, 2048 HE input samples, or 512 AAC-LD). ADTS or LOAS
//! wraps the unit; Raw emits the `raw_data_block` plus [`Encoder::asc`].
//! Leftover samples are buffered across feeds; [`Encoder::finish`] encodes
//! the zero-padded tail frame, exactly like one-shot [`crate::encode_with`]
//! — same frame sequence, same bytes. M4A is rejected at construction
//! (`stco` / sample sizes need the finished totals; use
//! [`crate::encode_with`] with [`EncodeContainer::M4a`]). The push decoder
//! accepts fragmented MP4 and still rejects flat M4A.
//!
//! With [`EncodeOptions::lookahead`] on, the callback trails the input by
//! one frame (the attack detector sees the next frame before a frame is
//! encoded) and `finish` flushes the held frame; byte-parity with one-shot
//! holds for the same options.
//!
//! Lifecycle: **open** → `feed` / `finish`; a PCM, rate, callback, or
//! encode error moves to **failed**; a successful `finish` moves to
//! **finished**. Further `feed` / `finish` error until [`Encoder::reset`].
//!
//! ```
//! use syom::{DecodeOptions, EncodeOptions, Encoder};
//! let mut enc = Encoder::new(48_000, 1, &EncodeOptions::adts())?;
//! let pcm = vec![0.0f32; 3000];
//! let mut adts = Vec::new();
//! for chunk in pcm.chunks(777) {
//!     enc.feed(&[chunk], |f| {
//!         adts.extend_from_slice(f.au);
//!         Ok(())
//!     })?;
//! }
//! let info = enc.finish(|f| {
//!     adts.extend_from_slice(f.au);
//!     Ok(())
//! })?;
//! assert_eq!(info.samples, 3000);
//! assert_eq!(info.aac_frames, 4); // padded tail + overlap drain
//! assert_eq!(info.priming, 1024);
//! assert_eq!(info.remainder, 72); // 4*1024 - priming - 3000
//! assert_eq!(info.bytes as usize, adts.len());
//! let dec = syom::decode_with(&adts, &DecodeOptions::unbounded())?;
//! assert_eq!(dec.channels[0].len(), 4 * 1024);
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! Raw AUs for an external muxer: [`EncodeOptions::raw`], [`Encoder::asc`],
//! [`crate::wrap_adts_au`].

use crate::engine::adts::ADTS_SAMPLE_RATES_HZ;
use crate::engine::swb::LONG_WINDOW_LEN as FRAME;
use crate::error::{AacError, Result};
use crate::options::{EncodeContainer, EncodeOptions};

/// One encoded AAC frame.
///
/// The bytes borrow encoder scratch and are valid **only for the duration
/// of the callback** — copy them out to keep them.
#[non_exhaustive]
pub struct EncodedFrame<'a> {
    /// Input samples this frame carries: 1024 (LC), 2048 (HE), or 512
    /// (AAC-LD). The [`Encoder::finish`] tail is the remainder, zero-padded
    /// to that frame length in the bitstream.
    pub samples: usize,
    /// Container bytes: an ADTS or LOAS frame, or the raw `raw_data_block`
    /// when the encoder was built with [`EncodeContainer::Raw`].
    pub au: &'a [u8],
    /// Elementary `raw_data_block` (no ADTS header). Same lifetime as `au`.
    pub payload: &'a [u8],
}

/// Resumable push encoder: planar f32 chunks in, ADTS, LOAS, or raw units out.
///
/// Byte-exact with one-shot [`crate::encode_with`] on the same PCM, for any
/// feed chunking. Peak RAM is one frame of PCM plus the current access
/// unit. Validation matches one-shot encode: the ADTS sample-rate table,
/// 1–2 channels or surround 3 / 4 / 5 / 6 / 8 planes (LC, no lookahead),
/// finite samples in `[-1, 1]`, equal plane lengths.
/// `|x| > 1` and non-finite samples are [`crate::AacError::InvalidPcm`] (no clip).
pub struct Encoder {
    enc: Core,
    sample_rate: u32,
    channels: usize,
    /// ADTS, LATM/LOAS or raw framing of each delivered unit.
    framing: EncodeContainer,
    /// `AudioSpecificConfig` (2-byte LC, 3-byte AAC-LD, or 4-byte HE) and its bit count.
    asc: Vec<u8>,
    asc_bits: u32,
    /// Raw access unit the callback's `payload` borrows when the frame
    /// bytes are not a byte-aligned superset of it (LATM).
    au_scratch: Vec<u8>,
    /// Samples buffered across feeds, one plane per channel.
    pending: [Vec<f32>; 2],
    pending_len: usize,
    /// Reused access-unit scratch the callback borrows.
    scratch: Vec<u8>,
    samples: u64,
    aac_frames: u64,
    bytes: u64,
    life: Life,
    /// HE: access units collected from the engine before delivery
    /// (reused; `he_n` are live), and source samples already attributed
    /// to delivered frames.
    he_aus: Vec<Vec<u8>>,
    he_n: usize,
    attributed: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Life {
    Open,
    Finished,
    Failed,
}

impl Encoder {
    /// New encoder for `channels` planes at `sample_rate` under `opts`.
    ///
    /// Errors [`AacError::Encode`] on an unsupported rate, channel count,
    /// or zero bitrate. [`EncodeContainer::M4a`] is [`AacError::Unsupported`]
    /// (use [`crate::encode_with`]; M4A needs finish-time sizes).
    pub fn new(sample_rate: u32, channels: usize, opts: &EncodeOptions) -> Result<Self> {
        crate::encode::check_mode(opts)?;
        if !ADTS_SAMPLE_RATES_HZ.contains(&sample_rate) {
            return Err(AacError::encode(format!(
                "encode: unsupported sample rate {sample_rate}Hz (not in the AAC table)"
            )));
        }
        crate::encode::check_planes(channels, opts)?;
        if opts.bitrate_bps == 0 {
            return Err(AacError::encode("encode: bitrate must be > 0"));
        }
        let max_bps = if opts.ld {
            crate::encode::max_ld_bitrate(sample_rate, channels)
        } else {
            crate::engine::enc_frame::max_bitrate_bps(sample_rate, channels)
        };
        if opts.bitrate_bps > max_bps {
            let profile = if opts.ld { "AAC-LD" } else { "AAC-LC" };
            return Err(AacError::encode(format!(
                "encode: bitrate {} bps exceeds {profile} 6144 bits/channel (max {max_bps} bps)",
                opts.bitrate_bps
            )));
        }
        if opts.container == EncodeContainer::M4a {
            return Err(AacError::Unsupported(
                crate::UnsupportedFeature::EncodeM4aStreaming,
            ));
        }
        let framing = opts.container;
        let (enc, asc) = if channels > 2 {
            let mc = mc::McCore::new(sample_rate, channels, opts)?;
            let asc =
                crate::engine::asc::write_lc(mc.enc.fs_index(), mc.enc.channel_configuration());
            (Core::Mc(Box::new(mc)), asc)
        } else if opts.he {
            let he = crate::encode::new_he(sample_rate, channels, opts)?;
            let asc = crate::encode::he_asc(&he, sample_rate, channels)?;
            (Core::He(Box::new(he)), asc)
        } else if opts.ld {
            let mut ld = if channels == 1 {
                crate::engine::enc_ld::LdEncoder::new(sample_rate)
            } else {
                crate::engine::enc_ld::LdEncoder::stereo(sample_rate)
            }
            .map_err(AacError::from)?;
            ld.set_bitrate(opts.bitrate_bps);
            let asc = ld.asc();
            (Core::Ld(Box::new(ld)), asc)
        } else {
            let lc = crate::encode::new_lc(sample_rate, channels, opts)?;
            let asc = crate::engine::asc::write_lc(lc.fs_index(), channels as u8);
            (Core::Lc(Box::new(lc)), asc)
        };
        let asc_bits = crate::engine::latm_write::asc_bit_len(&asc)?;
        Ok(Self {
            enc,
            sample_rate,
            channels,
            framing,
            asc,
            asc_bits,
            au_scratch: Vec::new(),
            pending: [Vec::new(), Vec::new()],
            pending_len: 0,
            scratch: Vec::new(),
            samples: 0,
            aac_frames: 0,
            bytes: 0,
            life: Life::Open,
            he_aus: Vec::new(),
            he_n: 0,
            attributed: 0,
        })
    }

    /// `AudioSpecificConfig` for this encoder (stable; not input-sized):
    /// 2-byte LC, 3-byte AAC-LD, or a 4-byte explicit HE v1 or v2 config.
    #[must_use]
    pub fn asc(&self) -> &[u8] {
        &self.asc
    }

    fn ensure_open(&self) -> Result<()> {
        match self.life {
            Life::Open => Ok(()),
            Life::Finished => Err(AacError::Lifecycle {
                state: crate::LifecycleState::Finished,
            }),
            Life::Failed => Err(AacError::Lifecycle {
                state: crate::LifecycleState::Failed,
            }),
        }
    }

    fn fail<T>(&mut self, e: AacError) -> Result<T> {
        self.life = Life::Failed;
        Err(e)
    }

    /// Drop overlap, rate credit, pending PCM, and counters. Prepared KBD
    /// windows, psy spreading, and vector capacity are kept. Rate, channels,
    /// and [`EncodeOptions`] are unchanged. No global cache.
    pub fn reset(&mut self) -> Result<()> {
        self.enc.reset();
        self.he_n = 0;
        self.attributed = 0;
        for p in &mut self.pending {
            p.clear();
        }
        self.pending_len = 0;
        self.scratch.clear();
        self.samples = 0;
        self.aac_frames = 0;
        self.bytes = 0;
        self.life = Life::Open;
        Ok(())
    }

    /// Feed PCM planes (any length, equal lengths per channel). `on_frame`
    /// fires per completed AAC frame. Returns samples consumed per channel.
    pub fn feed<F>(&mut self, planes: &[&[f32]], on_frame: F) -> Result<usize>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        self.ensure_open()?;
        if planes.len() != self.channels {
            return Err(AacError::InvalidPcm(crate::PcmReject::ChannelCount));
        }
        let n = planes.first().map_or(0, |p| p.len());
        if planes.iter().any(|p| p.len() != n) {
            return Err(AacError::InvalidPcm(crate::PcmReject::PlaneLength));
        }
        if let Err(e) = crate::encode::check_pcm_samples(planes.iter().copied()) {
            return self.fail(e);
        }
        match self.enc {
            Core::He(_) => return self.feed_he(planes, on_frame),
            Core::Mc(_) => return self.feed_mc(planes, on_frame),
            Core::Ld(_) => return self.feed_ld(planes, on_frame),
            Core::Lc(_) => {}
        }
        let mut cb = on_frame;
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut off = 0usize;
        while self.pending_len + (n - off) >= FRAME {
            let take = FRAME - self.pending_len;
            let mut bufs = [[0.0f32; FRAME]; 2];
            for (ch, buf) in bufs.iter_mut().enumerate().take(self.channels) {
                buf[..self.pending_len].copy_from_slice(&self.pending[ch][..self.pending_len]);
                buf[self.pending_len..].copy_from_slice(&planes[ch][off..off + take]);
            }
            if let Err(e) = self.emit(&bufs, FRAME, &mut scratch, &mut cb) {
                self.pending_len = 0;
                self.samples += (off + take) as u64;
                self.scratch = scratch;
                return self.fail(e);
            }
            off += take;
            self.pending_len = 0;
        }
        self.scratch = scratch;
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

    /// End of input: encode the zero-padded tail (if any) and report tallies.
    /// Lookahead flushes the held frame first. Empty input is [`AacError::InvalidPcm`].
    pub fn finish<F>(&mut self, on_frame: F) -> Result<EncodeInfo>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        self.ensure_open()?;
        if self.samples == 0 {
            return self.fail(AacError::InvalidPcm(crate::PcmReject::Empty));
        }
        match self.enc {
            Core::He(_) => return self.finish_he(on_frame),
            Core::Mc(_) => return self.finish_mc(on_frame),
            Core::Ld(_) => return self.finish_ld(on_frame),
            Core::Lc(_) => {}
        }
        let mut cb = on_frame;
        let mut scratch = std::mem::take(&mut self.scratch);
        let r = self.flush_end(&mut scratch, &mut cb);
        self.scratch = scratch;
        if let Err(e) = r {
            return self.fail(e);
        }
        let n = self.samples;
        let block = FRAME as u64;
        let remainder = (block - (n % block)) % block;
        self.life = Life::Finished;
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

    fn flush_end<F>(&mut self, scratch: &mut Vec<u8>, cb: &mut F) -> Result<()>
    where
        F: FnMut(EncodedFrame<'_>) -> Result<()>,
    {
        let tail = self.pending_len;
        if tail > 0 {
            let mut bufs = [[0.0f32; FRAME]; 2];
            for (ch, buf) in bufs.iter_mut().enumerate().take(self.channels) {
                buf[..tail].copy_from_slice(&self.pending[ch][..tail]);
            }
            if self.lc()?.lookahead_enabled() {
                let planes: Vec<&[f32]> = bufs[..self.channels].iter().map(|b| &b[..]).collect();
                if let Some(au) = self.lc()?.push_frame(&planes)? {
                    self.deliver(&au, FRAME, scratch, cb)?;
                }
            } else {
                self.emit(&bufs, tail, scratch, cb)?;
            }
        }
        // One-shot codes the last content frame while looking at the silent
        // drain (`attack(last) || attack(zeros)`). Flushing that frame first
        // drops the step into silence and picks a different window.
        let lookahead = self.lc()?.lookahead_enabled();
        let aus = self.lc()?.drain_overlap()?;
        let content = if tail > 0 { tail } else { FRAME };
        for (i, au) in aus.iter().enumerate() {
            let n = if lookahead && aus.len() > 1 && i == 0 {
                content
            } else {
                0
            };
            self.deliver(au, n, scratch, cb)?;
        }
        Ok(())
    }
}

#[path = "enc_stream_emit.rs"]
mod emit;

#[path = "enc_stream_he.rs"]
mod he;
#[path = "enc_stream_ld.rs"]
mod ld_stream;
use he::Core;

#[path = "enc_stream_mc.rs"]
mod mc;

#[path = "enc_stream_info.rs"]
mod info;
pub use info::EncodeInfo;

#[cfg(test)]
#[path = "enc_stream_tests.rs"]
mod enc_stream_tests;
