//! Presentation-sample seeking on a seekable M4A (`elst` + codec preroll).

use std::io::{self, Read, Seek, SeekFrom};

use crate::engine::asc::AudioSpecificConfig;
use crate::engine::decode::StreamDecoder;
use crate::error::{AacError, Result};
use crate::isomp4::{self, AacTrack, TrackBoxes, load_track_boxes};
use crate::options::{ChannelMode, DecodeOptions};
use crate::stream::{Frame, StreamInfo};

use super::m4a::map_m4a_parse_err;

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    loop {
        match r.read_exact(buf) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(AacError::truncated_at(None));
            }
            Err(e) => return Err(AacError::from(e)),
        }
    }
}

/// Extra LC mono/stereo overlap AUs (measured). HE/PS and any
/// `channel_configuration` above stereo replay from AU 0 (see [`M4aSeek::seek`]).
#[must_use]
pub fn preroll_aus(sbr: bool) -> u64 {
    if sbr { 0 } else { 2 }
}

fn samples_per_au(asc: &AudioSpecificConfig) -> u64 {
    // LC is 1024 or 960; HE output is twice the core when the ASC rate is.
    // ER AAC LD is one 512- or 480-sample frame.
    let core = u64::from(asc.frame_samples.max(1));
    let sr = u64::from(asc.sample_rate.max(1));
    core.saturating_mul(u64::from(asc.output_sample_rate)) / sr
}

/// Seekable M4A source: presentation-sample `seek` with codec preroll.
///
/// ```
/// use std::io::Cursor;
/// use syom::{DecodeOptions, M4aSeek};
/// let m4a = include_bytes!("../goldens/sine441.m4a");
/// let mut src = M4aSeek::open(Cursor::new(m4a), DecodeOptions::speech())?;
/// let at = src.seek(1024)?;
/// assert_eq!(at, 1024);
/// let mut samples = 0usize;
/// let info = src.decode(|f| {
///     samples += f.samples;
///     Ok(())
/// })?;
/// assert_eq!(samples as u64, info.samples);
/// # Ok::<(), syom::AacError>(())
/// ```
pub struct M4aSeek<R> {
    reader: R,
    track: AacTrack,
    asc: AudioSpecificConfig,
    opts: DecodeOptions,
    dec: Box<StreamDecoder>,
    scratch: Vec<f32>,
    next_frame: usize,
    discard_left: usize,
    play_left: u64,
    pos: u64,
    skip: u64,
    play: u64,
    samples_per_au: u64,
    preroll: u64,
    locked: Option<(u32, usize)>,
    mono: bool,
    checkpoint_gapless_safe: bool,
    #[cfg(test)]
    last_seek_aus: u64,
}

impl<R: Read + Seek> M4aSeek<R> {
    /// Parse `moov` and sit at presentation 0 (elst skip still to apply).
    pub fn open(mut reader: R, opts: DecodeOptions) -> Result<Self> {
        opts.validate()?;
        let boxes = load_track_boxes(&mut reader, &opts.memory)?;
        let tagged = match &boxes {
            TrackBoxes::Flat { moov, .. } | TrackBoxes::Frag { moov, .. } => {
                crate::gapless::itunsmpb_in(moov).is_some()
            }
        };
        let track = match boxes {
            TrackBoxes::Flat { moov, file_len } => {
                isomp4::parse_aac_track_with_len(&moov, file_len, &opts.memory)
                    .map_err(map_m4a_parse_err)?
            }
            TrackBoxes::Frag {
                moov, moofs, mdats, ..
            } => isomp4::frag::track_from_parts(&moov, &moofs, &mdats, &opts.memory)?,
        };
        let (asc, _) = AudioSpecificConfig::parse(&track.asc).map_err(AacError::from)?;
        let out_rate = asc.output_sample_rate;
        if out_rate == 0 || out_rate > opts.max_sample_rate {
            return Err(AacError::sample_rate(out_rate, opts.max_sample_rate));
        }
        let spf = samples_per_au(&asc).max(1);
        let (skip, play_opt) = track.edit_window(out_rate)?;
        let skip = skip as u64;
        let coded = spf.saturating_mul(track.frames.len() as u64);
        let play = match play_opt {
            Some(p) => p as u64,
            None => coded.saturating_sub(skip),
        };
        let mono = matches!(opts.channel_mode, ChannelMode::Mono);
        let n_ch = if mono {
            1
        } else {
            u32::from(asc.channel_configuration).max(1)
        };
        opts.memory.check_output(n_ch, track.total_samples)?;
        // LD overlap is one 512-sample frame. LC mono/stereo needs two
        // (measured); HE/PS replays from the start because of in-band SBR.
        let preroll = if asc.aot == crate::engine::asc::AOT_LD {
            1
        } else {
            preroll_aus(asc.sbr_present)
        };
        let checkpoint_gapless_safe = track.has_elst || !tagged;
        let mut s = Self {
            reader,
            track,
            asc,
            opts,
            dec: StreamDecoder::new(),
            scratch: Vec::new(),
            next_frame: 0,
            discard_left: skip as usize,
            play_left: play,
            pos: 0,
            skip,
            play,
            samples_per_au: spf,
            preroll,
            locked: None,
            mono,
            checkpoint_gapless_safe,
            #[cfg(test)]
            last_seek_aus: 0,
        };
        s.reset_codec();
        Ok(s)
    }

    fn reset_codec(&mut self) {
        self.dec.reset();
        self.dec.mix_down_mono = self.mono;
        if let Some(pce) = self.asc.pce.clone() {
            self.dec.set_config_pce(pce);
        }
        self.dec.set_he_config(
            self.asc.sbr_present,
            self.asc.ps_present,
            self.asc.output_sample_rate,
        );
        self.dec.set_frame_samples(self.asc.frame_samples);
        self.locked = None;
        self.scratch.clear();
    }

    /// Presentation length in output samples (`elst` when present).
    #[must_use]
    pub fn presentation_len(&self) -> u64 {
        self.play
    }

    /// Current presentation position.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Profile preroll in AUs (see [`preroll_aus`]).
    #[must_use]
    pub fn preroll_aus(&self) -> u64 {
        self.preroll
    }

    #[cfg(test)]
    pub(crate) fn last_seek_aus(&self) -> u64 {
        self.last_seek_aus
    }

    /// Seek to a presentation sample. `0` is the first sample after `elst`
    /// skip. Negative is before-start; `> presentation_len` is beyond-end.
    pub fn seek(&mut self, presentation_sample: i64) -> Result<u64> {
        if presentation_sample < 0 {
            return Err(AacError::invalid_limits(
                "m4a seek before presentation start",
            ));
        }
        let pres = presentation_sample as u64;
        if pres > self.play {
            let rate = self.asc.output_sample_rate.max(1);
            return Err(AacError::too_long(
                pres as f64 / f64::from(rate),
                self.play as f64 / f64::from(rate),
            ));
        }
        self.reset_codec();
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
        let coded = self.skip.saturating_add(pres);
        let spf = self.samples_per_au.max(1);
        let target = coded / spf;
        let intra = coded % spf;
        // HE/PS: SBR headers live in-band (TASK-39). More than stereo:
        // overlap was not shown to settle in 2 AUs (measured on mc51).
        // LC mono/stereo: 2 AUs (measured). LD: 1 AU, the low-delay overlap.
        let start = if self.asc.sbr_present || self.asc.channel_configuration > 2 {
            0
        } else {
            target.saturating_sub(self.preroll)
        };
        for i in start..target {
            let idx = usize::try_from(i).unwrap_or(usize::MAX);
            if idx >= self.track.frames.len() {
                break;
            }
            self.decode_au(idx)?;
            #[cfg(test)]
            {
                self.last_seek_aus += 1;
            }
        }
        self.next_frame = usize::try_from(target).unwrap_or(usize::MAX);
        self.discard_left = usize::try_from(intra).unwrap_or(usize::MAX);
        Ok(pres)
    }

    fn decode_au(&mut self, idx: usize) -> Result<(u32, usize)> {
        let &(off, len) = self
            .track
            .frames
            .get(idx)
            .ok_or_else(|| AacError::format("aac: sample index past table"))?;
        self.opts.memory.check_declared_au(u64::from(len))?;
        let n = usize::try_from(len).map_err(|_| AacError::format("aac: sample range overflow"))?;
        self.reader
            .seek(SeekFrom::Start(off))
            .map_err(AacError::from)?;
        let mut payload = vec![0u8; n];
        read_exact(&mut self.reader, &mut payload)?;
        let rate = if self.mono {
            self.scratch.clear();
            self.dec
                .decode_raw_mono_f32(
                    self.asc.aot,
                    self.asc.sampling_frequency_index,
                    self.asc.sample_rate,
                    self.asc.channel_configuration,
                    1,
                    &payload,
                    &mut self.scratch,
                )
                .map_err(AacError::from)?
        } else {
            self.dec
                .decode_frame_scaled(
                    self.asc.aot,
                    self.asc.sampling_frequency_index,
                    self.asc.sample_rate,
                    self.asc.channel_configuration,
                    &payload,
                )
                .map_err(AacError::from)?
        };
        let n_samp = if self.mono {
            self.scratch.len()
        } else {
            self.dec.frame_planes().first().map_or(0, Vec::len)
        };
        Ok((rate, n_samp))
    }

    /// Decode from the current position to the presentation end.
    pub fn decode<F>(&mut self, mut on_frame: F) -> Result<StreamInfo>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        let mut aac_frames = 0u64;
        let mut emitted = 0u64;
        let mut samples_out = 0u64;
        while self.next_frame < self.track.frames.len() && self.play_left > 0 {
            let (rate, n) = self.decode_au(self.next_frame)?;
            self.next_frame += 1;
            aac_frames += 1;
            if n == 0 {
                continue;
            }
            let n_ch = if self.mono {
                1
            } else {
                self.dec.frame_planes().len()
            };
            match self.locked {
                None => {
                    if rate == 0 || rate > self.opts.max_sample_rate {
                        return Err(AacError::sample_rate(rate, self.opts.max_sample_rate));
                    }
                    self.locked = Some((rate, n_ch));
                }
                Some((r, c)) => {
                    if rate != r || c != n_ch {
                        return Err(AacError::decode(
                            "aac: sample rate or channel count changed mid-stream",
                        ));
                    }
                }
            }
            if self.discard_left >= n {
                self.discard_left -= n;
                continue;
            }
            let start = self.discard_left;
            self.discard_left = 0;
            let mut take = n - start;
            take = take.min(self.play_left as usize);
            self.play_left -= take as u64;
            if take == 0 {
                continue;
            }
            emitted += 1;
            samples_out += take as u64;
            self.pos += take as u64;
            let end = start + take;
            if self.mono {
                let plane: [&[f32]; 1] = [&self.scratch[start..end]];
                on_frame(Frame {
                    sample_rate: rate,
                    samples: take,
                    planar: &plane,
                    meta: self.dec.last_meta(),
                })?;
            } else {
                let mut slots: [&[f32]; crate::layout::MAX_PLANES] =
                    [&[]; crate::layout::MAX_PLANES];
                let src = self.dec.frame_planes();
                let n = src.len().min(crate::layout::MAX_PLANES);
                for (i, ch) in src.iter().enumerate().take(n) {
                    slots[i] = &ch[start..end];
                }
                on_frame(Frame {
                    sample_rate: rate,
                    samples: take,
                    planar: &slots[..n],
                    meta: self.dec.last_meta(),
                })?;
            }
        }
        if emitted == 0 && samples_out == 0 && self.play > 0 && self.pos == 0 {
            return Err(AacError::NotAac);
        }
        let (sample_rate, channels) = self
            .locked
            .unwrap_or((self.asc.output_sample_rate, if self.mono { 1 } else { 0 }));
        Ok(StreamInfo {
            sample_rate,
            core_rate: self.dec.last_core_rate(),
            channels,
            layout: self.dec.last_meta().layout,
            aac_frames,
            samples: samples_out,
            priming: self.track.has_elst.then_some(self.skip),
            remainder: self.track.unplayed_tail(self.skip, samples_out),
        })
    }
}

// Versioned LC-only state. Spectral/IMDCT scratch is overwritten by the next AU.
// HE, profile prediction, PCE and coupling histories are deliberately excluded.
impl<R: Read + Seek> M4aSeek<R> {
    pub fn supports_checkpoint(&self) -> bool {
        self.checkpoint_gapless_safe
            && self.asc.frame_samples == 1024
            && matches!(
                self.asc.output_sample_rate,
                8000 | 16000 | 22050 | 24000 | 32000 | 44100 | 48000
            )
            && self.asc.aot == 2
            && !self.asc.sbr_present
            && !self.asc.ps_present
            && matches!(self.asc.channel_configuration, 1 | 2)
            && matches!(self.opts.channel_mode, ChannelMode::Mono)
    }
    pub fn checkpoint_compatible(&self) -> bool {
        self.supports_checkpoint()
            && !self.dec.sbr_active
            && self.dec.pce.is_none()
            && self.dec.fb_pool.is_empty()
            && self.dec.tools.aot == 2
    }
    pub fn bounded_presentation(&self) -> bool {
        self.track.has_elst
    }
    pub fn output_sample_rate(&self) -> u32 {
        self.asc.output_sample_rate
    }
    pub fn remaining_samples(&self) -> u64 {
        self.play_left
    }
    /// Validate EOF without padding a rounded movie duration. A nonzero tail
    /// is admissible only after every indexed AU and the priming skip completed.
    pub fn presentation_complete(&self) -> bool {
        self.discard_left == 0 && (self.play_left == 0 || (
            self.next_frame == self.track.frames.len()
                && self.track.rounded_presentation_tail(self.play_left, self.asc.output_sample_rate)
        ))
    }
    pub fn checkpoint(&self) -> Option<Vec<u8>> {
        if !self.checkpoint_compatible()
            || self.play_left == 0
            || self.discard_left != 0
            || (self.next_frame as u64).checked_mul(self.samples_per_au)
                != self.skip.checked_add(self.pos)
        {
            return None;
        }
        let mut out = b"YTLC0001".to_vec();
        for value in [
            self.next_frame as u64,
            self.pos,
            self.discard_left as u64,
            self.play_left,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.dec.rng.checkpoint().to_le_bytes());
        self.dec.fb_l.checkpoint(&mut out);
        self.dec.fb_r.checkpoint(&mut out);
        Some(out)
    }
    pub fn restore_checkpoint(&mut self, bytes: &[u8]) -> Result<()> {
        let invalid = || AacError::format("invalid LC decoder checkpoint");
        if !self.supports_checkpoint() || bytes.len() != 8238 || &bytes[..8] != b"YTLC0001" {
            return Err(invalid());
        }
        let number = |offset| -> Result<u64> {
            Ok(u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .map_err(|_| invalid())?,
            ))
        };
        let next = number(8)?;
        let pos = number(16)?;
        let discard = number(24)?;
        let left = number(32)?;
        if next > self.track.frames.len() as u64
            || discard != 0
            || pos == 0
            || pos >= self.play
            || left != self.play - pos
            || next.checked_mul(self.samples_per_au) != self.skip.checked_add(pos)
        {
            return Err(invalid());
        }
        self.reset_codec();
        self.dec.rng = crate::engine::pns::Lcg::restore_checkpoint(u32::from_le_bytes(
            bytes[40..44].try_into().map_err(|_| invalid())?,
        ));
        self.dec
            .fb_l
            .restore_checkpoint(&bytes[44..4141])
            .ok_or_else(invalid)?;
        self.dec
            .fb_r
            .restore_checkpoint(&bytes[4141..8238])
            .ok_or_else(invalid)?;
        self.next_frame = usize::try_from(next).map_err(|_| invalid())?;
        self.pos = pos;
        self.discard_left = 0;
        self.play_left = left;
        self.locked = Some((self.asc.output_sample_rate, 1));
        Ok(())
    }
}
