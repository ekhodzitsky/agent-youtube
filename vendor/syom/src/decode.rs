//! One-shot decode: a collecting frame callback over the streaming core
//! (`stream` module owns the container state machines and caps).

use std::io::{Read, Seek, SeekFrom};

use crate::budgets::{BudgetExceeded, BudgetKind, pcm_bytes};
use crate::engine::adts::AdtsHeader;
use crate::error::{AacError, Result};
use crate::isomp4::sniff_is_isobmff;
use crate::options::{ChannelMode, DEFAULT_MAX_INPUT_BYTES, DecodeOptions};
use crate::stream::{
    decode_read_streaming, decode_seek_streaming, decode_streaming, decode_streaming_mono_into,
};

/// Decoded AAC at native sample rate (planar f32, mono-mixed or split).
///
/// Split channel order: mono/stereo as-is; multichannel AAC-LC
/// (`channel_configuration` 3–7) follows the libavcodec layout order
/// (5.1 = FL FR FC LFE BL BR, 7.1 adds SL SR); PCE streams follow PCE declaration order
/// (front, side, back, LFE). Speech mono is the mean of the non-LFE planes.
/// PCM is presentation-valid: M4A honours `elst`. A one-shot ADTS decode
/// honours a leading `iTunSMPB` tag when its counts match the decoded
/// length. Channel labels and decode-side priming/remainder are TASK-61;
/// `#[non_exhaustive]` leaves room.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodedAac {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    /// Core AAC rate (before SBR). Same as [`Self::sample_rate`] for LC.
    pub core_rate: u32,
    /// Channel layout of [`Self::channels`].
    pub layout: crate::Layout,
    /// Encoder delay skipped at the start, when the container says so.
    pub priming: Option<u64>,
    /// Unplayed coded tail, when the container says so.
    pub remainder: Option<u64>,
}

impl DecodedAac {
    pub(crate) fn from_info(info: crate::StreamInfo, channels: Vec<Vec<f32>>) -> Self {
        Self {
            sample_rate: info.sample_rate,
            channels,
            core_rate: info.core_rate,
            layout: info.layout,
            priming: info.priming,
            remainder: info.remainder,
        }
    }
}

/// Decode ADTS, M4A, LATM/LOAS, or bounded fragmented MP4 with speech defaults.
#[inline]
pub fn decode(data: &[u8]) -> Result<DecodedAac> {
    decode_with(data, &DecodeOptions::speech())
}

/// Alias of [`decode`].
#[inline]
pub fn decode_bytes(data: &[u8]) -> Result<DecodedAac> {
    decode(data)
}

/// Read a file and [`decode`] it. M4A uses seekable sample access (no
/// whole-`mdat` load); ADTS/LOAS still cap the file at
/// [`DEFAULT_MAX_INPUT_BYTES`].
pub fn read(path: impl AsRef<std::path::Path>) -> Result<DecodedAac> {
    read_with(path, &DecodeOptions::speech())
}

/// Read a file and [`decode_with`] it.
pub fn read_with(path: impl AsRef<std::path::Path>, opts: &DecodeOptions) -> Result<DecodedAac> {
    let path = path.as_ref();
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 12];
    let n = f.read(&mut head)?;
    if sniff_is_isobmff(&head[..n]) {
        f.seek(SeekFrom::Start(0))?;
        return decode_seek_with(f, opts);
    }
    drop(f);
    decode_with(&read_file_capped(path, DEFAULT_MAX_INPUT_BYTES)?, opts)
}

/// Decode seekable M4A with speech-ingest defaults. Peak compressed RAM is
/// `moov` plus one access unit.
#[inline]
pub fn decode_seek<R: Read + Seek>(reader: R) -> Result<DecodedAac> {
    decode_seek_with(reader, &DecodeOptions::speech())
}

/// Decode seekable M4A under `opts`.
pub fn decode_seek_with<R: Read + Seek>(reader: R, opts: &DecodeOptions) -> Result<DecodedAac> {
    let mut tracks: Vec<Vec<f32>> = Vec::new();
    let info = decode_seek_streaming(reader, opts, |f| {
        let n_ch = u32::try_from(f.planar.len()).unwrap_or(u32::MAX);
        if tracks.is_empty() {
            opts.memory.check_channels(n_ch)?;
            tracks.resize_with(f.planar.len(), Vec::new);
        }
        let next = tracks.first().map(|t| t.len() as u64).unwrap_or(0) + f.samples as u64;
        opts.memory.check_output(n_ch, next)?;
        for (dst, src) in tracks.iter_mut().zip(f.planar.iter()) {
            dst.extend_from_slice(src);
        }
        Ok(())
    })?;
    Ok(DecodedAac::from_info(info, tracks))
}

/// Decode ADTS, LOAS, or bounded fMP4 from a generic [`Read`] with speech defaults.
/// Does not materialize the whole stream. Flat M4A is [`AacError::Unsupported`]
/// (it needs [`std::io::Seek`]).
#[inline]
pub fn decode_read<R: Read>(reader: R) -> Result<DecodedAac> {
    decode_read_with(reader, &DecodeOptions::speech())
}

/// Decode ADTS, LOAS, or bounded fMP4 from `reader` under `opts`. Peak compressed RAM is the
/// streaming resident cap, not the stream duration.
pub fn decode_read_with<R: Read>(reader: R, opts: &DecodeOptions) -> Result<DecodedAac> {
    let mut tracks: Vec<Vec<f32>> = Vec::new();
    let info = decode_read_streaming(reader, opts, |f| {
        let n_ch = u32::try_from(f.planar.len()).unwrap_or(u32::MAX);
        if tracks.is_empty() {
            opts.memory.check_channels(n_ch)?;
            tracks.resize_with(f.planar.len(), Vec::new);
        }
        let next = tracks.first().map(|t| t.len() as u64).unwrap_or(0) + f.samples as u64;
        opts.memory.check_output(n_ch, next)?;
        for (dst, src) in tracks.iter_mut().zip(f.planar.iter()) {
            dst.extend_from_slice(src);
        }
        Ok(())
    })?;
    Ok(DecodedAac::from_info(info, tracks))
}

/// Load at most `limit` compressed bytes. Metadata larger than `limit` is
/// rejected before allocating that size; a file that grows after `stat` is
/// still stopped at `limit + 1` and rejected.
pub(crate) fn read_file_capped(path: impl AsRef<std::path::Path>, limit: u64) -> Result<Vec<u8>> {
    let path = path.as_ref();
    let meta = std::fs::metadata(path)?;
    if meta.len() > limit {
        return Err(AacError::too_long(meta.len() as f64 / 40_000.0, 0.0));
    }
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(AacError::too_long(buf.len() as f64 / 40_000.0, 0.0));
    }
    Ok(buf)
}

/// Decode ADTS, M4A, LATM/LOAS, or bounded fragmented MP4 under `opts`.
pub fn decode_with(data: &[u8], opts: &DecodeOptions) -> Result<DecodedAac> {
    let est_frames = adts_frame_estimate(data);
    if matches!(opts.channel_mode, ChannelMode::Mono) && !sniff_is_isobmff(data) {
        // Mono fast path: frames decode straight into the output plane, no
        // per-frame scratch or callback copy.
        let mut track: Vec<f32> = Vec::new();
        let info = decode_streaming_mono_into(data, opts, est_frames, &mut track)?;
        return Ok(DecodedAac::from_info(info, vec![track]));
    }
    let mut tracks: Vec<Vec<f32>> = Vec::new();
    let info = decode_streaming(data, opts, |f| {
        let n_ch = u32::try_from(f.planar.len()).unwrap_or(u32::MAX);
        if tracks.is_empty() {
            opts.memory.check_channels(n_ch)?;
            tracks.resize_with(f.planar.len(), Vec::new);
            // Pre-size once from the frame-count estimate instead of growing
            // geometrically per frame (each growth reallocs + recopies).
            if let Some(frames) = est_frames {
                let per = (frames * f.samples).min(opts.max_frames(f.sample_rate));
                if opts.memory.check_output(n_ch, per as u64).is_ok() {
                    for t in &mut tracks {
                        t.try_reserve(per).map_err(|_| {
                            AacError::from(BudgetExceeded {
                                kind: BudgetKind::Output,
                                observed: pcm_bytes(n_ch, per as u64).unwrap_or(u64::MAX),
                                max: opts.memory.effective_output_bytes(),
                            })
                        })?;
                    }
                }
            }
        }
        let next = tracks.first().map(|t| t.len() as u64).unwrap_or(0) + f.samples as u64;
        opts.memory.check_output(n_ch, next)?;
        for (dst, src) in tracks.iter_mut().zip(f.planar.iter()) {
            dst.extend_from_slice(src);
        }
        Ok(())
    })?;
    Ok(DecodedAac::from_info(info, tracks))
}

/// ADTS frame-count estimate for output pre-sizing: walk the frame headers
/// (payloads are skipped, so this is far cheaper than decoding). Stops at
/// the first unparseable or partial header — a hint only, never an error.
/// `None` when the input does not start with ADTS (M4A, LATM, garbage).
fn adts_frame_estimate(data: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    let mut frames = 0usize;
    while let Ok((hdr, _)) = AdtsHeader::parse(&data[pos..]) {
        let len = usize::from(hdr.aac_frame_length);
        if len == 0 || pos + len > data.len() {
            break;
        }
        pos += len;
        frames += 1;
    }
    (frames > 0).then_some(frames)
}
