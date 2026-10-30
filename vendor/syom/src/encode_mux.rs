//! Sink and muxer entry points of the encoder (line cap): `encode_write`
//! over any `io::Write`, `wrap_adts_au`, and the raw-AU M4A muxers.

use super::{FRAME, adts_frame_into, fs_index};
use crate::error::{AacError, Result};
use crate::m4a_write::{self, MoovPlan};
use crate::options::{EncodeContainer, EncodeOptions};
use std::io::{Seek, SeekFrom, Write};

/// Encode planar PCM incrementally into any [`std::io::Write`] sink (TASK-59):
/// ADTS, LOAS, or raw access units are written as they are produced, so the
/// codec workspace stays one frame and no compressed buffer is collected.
/// ADTS and LOAS match [`encode_with`]. Raw is push-only
/// ([`crate::UnsupportedFeature::EncodeRawOneShot`] from one-shot). M4A is
/// [`crate::UnsupportedFeature::EncodeM4aStreaming`].
///
/// Lifecycle: each frame is written once with `write_all`; a sink error
/// ends the encode with [`AacError::Io`] after the frames that already
/// succeeded (nothing is retried or written twice), so the sink holds an
/// exact prefix of the stream. The sink is not flushed.
///
/// ```
/// use syom::{EncodeOptions, encode_write};
/// let pcm = vec![0.0f32; 3000];
/// let mut sink = Vec::new();
/// let info = encode_write(&mut sink, &[&pcm[..]], 48_000, &EncodeOptions::adts())?;
/// assert!(sink.starts_with(b"ID3"));
/// assert_eq!(info.samples, 3000);
/// assert_eq!(info.priming, 1024);
/// assert_eq!(info.remainder, 72);
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn encode_write<W: std::io::Write, P: AsRef<[f32]>>(
    mut sink: W,
    pcm: &[P],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<crate::EncodeInfo> {
    let planes: Vec<&[f32]> = pcm.iter().map(AsRef::as_ref).collect();
    let n = planes.first().map_or(0, |p| p.len());
    if planes.iter().any(|p| p.len() != n) {
        return Err(AacError::InvalidPcm(crate::PcmReject::PlaneLength));
    }
    let mut enc = crate::Encoder::new(sample_rate, planes.len(), opts)?;
    // The tag is the first output byte, but only after PCM checks inside
    // `feed` succeed. A rejected buffer leaves the sink empty.
    let mut tag = (n > 0 && opts.container == EncodeContainer::Adts && !opts.ld)
        .then(|| crate::gapless::tag_bytes(n as u64, opts.he));
    let mut at = 0usize;
    while at < n {
        let end = (at + 4096).min(n);
        let chunk: Vec<&[f32]> = planes.iter().map(|p| &p[at..end]).collect();
        enc.feed(&chunk, |f| write_au(&mut sink, &mut tag, f.au))?;
        at = end;
    }
    enc.finish(|f| write_au(&mut sink, &mut tag, f.au))
}

fn write_au<W: Write>(sink: &mut W, tag: &mut Option<Vec<u8>>, au: &[u8]) -> Result<()> {
    if let Some(tag) = tag.take() {
        sink.write_all(&tag).map_err(AacError::Io)?;
    }
    sink.write_all(au).map_err(AacError::Io)
}

/// Wrap one `raw_data_block` in an ADTS frame (no CRC, fullness `0x7FF`).
/// For HE v1 access units pass the **core** rate (half the input rate):
/// ADTS signals SBR implicitly and the header carries the core rate.
///
/// ```
/// use syom::{EncodeOptions, Encoder, wrap_adts_au};
/// let pcm = vec![0.1f32; 2048];
/// let mut enc = Encoder::new(48_000, 1, &EncodeOptions::raw())?;
/// let mut aus = Vec::new();
/// enc.feed(&[&pcm], |f| { aus.push(f.payload.to_vec()); Ok(()) })?;
/// enc.finish(|f| { aus.push(f.payload.to_vec()); Ok(()) })?;
/// let mut adts = Vec::new();
/// for au in &aus {
///     adts.extend_from_slice(&wrap_adts_au(au, 48_000, 1)?);
/// }
/// assert_eq!(adts[0], 0xff);
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn wrap_adts_au(payload: &[u8], sample_rate: u32, channels: usize) -> Result<Vec<u8>> {
    let fs = fs_index(sample_rate)?;
    if !(1..=2).contains(&channels) {
        return Err(AacError::encode(format!(
            "encode: channels must be 1 or 2, got {channels}"
        )));
    }
    let mut out = Vec::with_capacity(crate::engine::adts::ADTS_HEADER_BYTES_NO_CRC + payload.len());
    adts_frame_into(payload, fs, channels, &mut out);
    Ok(out)
}

/// Mux raw HE v1 access units (push [`crate::Encoder`] with
/// [`EncodeOptions::with_he`] + [`EncodeContainer::Raw`]) into M4A:
/// explicit two-rate ASC, output-rate timeline, `elst` priming as
/// reported by [`crate::EncodeInfo::priming`].
pub fn mux_raw_he_m4a(
    payloads: &[&[u8]],
    output_rate: u32,
    channels: usize,
    valid_samples: u64,
) -> Result<Vec<u8>> {
    let core = crate::engine::enc_sbr_prep::he_core_rate(output_rate)
        .map_err(|_| AacError::Unsupported(crate::UnsupportedFeature::EncodeHeRate(output_rate)))?;
    if !(1..=2).contains(&channels) {
        return Err(AacError::encode(format!(
            "encode: channels must be 1 or 2, got {channels}"
        )));
    }
    let owned: Vec<Vec<u8>> = payloads.iter().map(|p| p.to_vec()).collect();
    m4a_write::mux_aac_he(
        &owned,
        fs_index(core)?,
        fs_index(output_rate)?,
        channels,
        output_rate,
        valid_samples,
        crate::engine::enc_he::HE_PRIMING_OUT,
    )
}

/// Mux raw LC access units into M4A (`elst` priming 1024, presentation `valid_samples`).
pub fn mux_raw_lc_m4a(
    payloads: &[&[u8]],
    sample_rate: u32,
    channels: usize,
    valid_samples: u64,
) -> Result<Vec<u8>> {
    let fs = fs_index(sample_rate)?;
    if !(1..=2).contains(&channels) {
        return Err(AacError::encode(format!(
            "encode: channels must be 1 or 2, got {channels}"
        )));
    }
    let owned: Vec<Vec<u8>> = payloads.iter().map(|p| p.to_vec()).collect();
    m4a_write::mux_aac_lc(
        &owned,
        fs,
        channels,
        sample_rate,
        valid_samples,
        FRAME as u64,
    )
}

/// Encode planar PCM into an M4A written incrementally to a `Write + Seek`
/// sink (TASK-60): `ftyp` and an `mdat` header go out first, every access
/// unit is written as it is produced, and `finish` seeks back to patch the
/// `mdat` size and appends `moov` (sample tables, `elst` priming, exact
/// presentation duration). Retained memory is one frame of codec state
/// plus 4 bytes per access unit (the `stsz` index, at most
/// 2^20 units); compressed media is never accumulated. `opts.container`
/// is ignored — the output is always M4A. Bytes are identical to
/// [`encode_with`] with [`EncodeContainer::M4a`]; offsets are relative
/// to the sink position at entry, so the M4A must start at that position.
///
/// Lifecycle: the output is a valid M4A **only after `Ok`**. A write or
/// seek error ends the encode with [`AacError::Io`]; the sink then holds
/// `ftyp` plus an `mdat` prefix without its size or `moov` (not playable,
/// nothing written twice). A version-0 ceiling (offsets, `mdat` and file
/// size in `u32`) is checked before every write; exceeding it is
/// [`AacError::Encode`] with no wrapped field.
///
/// ```
/// use syom::{EncodeOptions, encode_write_m4a};
/// let pcm = vec![0.0f32; 3000];
/// let mut file = std::io::Cursor::new(Vec::new());
/// let info = encode_write_m4a(&mut file, &[&pcm[..]], 48_000, &EncodeOptions::default())?;
/// let bytes = file.into_inner();
/// assert!(syom::sniff_is_isobmff(&bytes));
/// assert_eq!(syom::decode_with(&bytes, &syom::DecodeOptions::audio())?.channels[0].len(), 3000);
/// assert_eq!(info.priming, 1024);
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn encode_write_m4a<W: Write + Seek, P: AsRef<[f32]>>(
    mut sink: W,
    pcm: &[P],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<crate::EncodeInfo> {
    let planes: Vec<&[f32]> = pcm.iter().map(AsRef::as_ref).collect();
    let raw = opts.clone().with_container(EncodeContainer::Raw);
    super::validate(&planes, sample_rate, &raw)?;
    let mut enc = crate::Encoder::new(sample_rate, planes.len(), &raw)?;
    let base = sink.stream_position().map_err(AacError::Io)?;
    let ftyp = m4a_write::ftyp_bytes();
    let media_offset = m4a_write::u32_field(ftyp.len() as u64 + 8, "mdat offset")?;
    sink.write_all(&ftyp).map_err(AacError::Io)?;
    sink.write_all(&[0, 0, 0, 0, b'm', b'd', b'a', b't'])
        .map_err(AacError::Io)?;
    let mut sizes: Vec<u32> = Vec::new();
    let mut payload_bytes = 0u64;
    let mut emit = |au: &[u8], sink: &mut W| -> Result<()> {
        let n = m4a_write::u32_field(au.len() as u64, "sample size")?;
        m4a_write::preflight(media_offset, payload_bytes + u64::from(n), sizes.len() + 1)?;
        sink.write_all(au).map_err(AacError::Io)?;
        sizes.push(n);
        payload_bytes += u64::from(n);
        Ok(())
    };
    let n = planes.first().map_or(0, |p| p.len());
    let mut at = 0usize;
    while at < n {
        let end = (at + 4096).min(n);
        let chunk: Vec<&[f32]> = planes.iter().map(|p| &p[at..end]).collect();
        enc.feed(&chunk, |f| emit(f.payload, &mut sink))?;
        at = end;
    }
    let info = enc.finish(|f| emit(f.payload, &mut sink))?;
    let mdat_size = m4a_write::u32_field(payload_bytes + 8, "mdat size")?;
    let mdat_at = base + ftyp.len() as u64;
    sink.seek(SeekFrom::Start(mdat_at)).map_err(AacError::Io)?;
    sink.write_all(&mdat_size.to_be_bytes())
        .map_err(AacError::Io)?;
    sink.seek(SeekFrom::Start(mdat_at + u64::from(mdat_size)))
        .map_err(AacError::Io)?;
    let plan = MoovPlan {
        asc: enc.asc(),
        channels: planes.len(),
        sample_rate,
        valid_samples: info.samples,
        priming: info.priming,
        frame_len: if opts.ld {
            512
        } else if opts.he {
            2048
        } else {
            FRAME as u32
        },
    };
    let moov = m4a_write::moov_bytes(&plan, &sizes, media_offset)?;
    m4a_write::u32_field(
        u64::from(media_offset) + payload_bytes + moov.len() as u64,
        "file size",
    )?;
    sink.write_all(&moov).map_err(AacError::Io)?;
    Ok(info)
}

/// LOAS frames for every raw access unit under one config (one-shot LATM).
pub(crate) fn wrap_latm(payloads: &[Vec<u8>], asc: &[u8]) -> Result<Vec<u8>> {
    let bits = crate::engine::latm_write::asc_bit_len(asc)?;
    let mut out = Vec::with_capacity(payloads.iter().map(|p| p.len() + 8).sum());
    for au in payloads {
        crate::engine::latm_write::loas_frame_into(asc, bits, au, &mut out)?;
    }
    Ok(out)
}

/// Wrap one raw access unit in a LOAS `AudioSyncStream` frame whose
/// `StreamMuxConfig` carries `asc` ([`crate::Encoder::asc`]: 2-byte LC,
/// 3-byte AAC-LD, or 4-byte HE). Every frame is a sync point. An
/// `AudioMuxElement` above 8191 bytes is an error (LC and HE v1 frames
/// never reach it).
///
/// ```
/// use syom::{EncodeOptions, Encoder, wrap_loas_au};
/// let pcm = vec![0.1f32; 2048];
/// let mut enc = Encoder::new(48_000, 1, &EncodeOptions::raw())?;
/// let asc = enc.asc().to_vec();
/// let mut loas = Vec::new();
/// enc.feed(&[&pcm], |f| { loas.extend_from_slice(&wrap_loas_au(f.payload, &asc)?); Ok(()) })?;
/// enc.finish(|f| { loas.extend_from_slice(&wrap_loas_au(f.payload, &asc)?); Ok(()) })?;
/// assert!(syom::sniff_is_latm(&loas));
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn wrap_loas_au(payload: &[u8], asc: &[u8]) -> Result<Vec<u8>> {
    let bits = crate::engine::latm_write::asc_bit_len(asc)?;
    let mut out = Vec::with_capacity(payload.len() + 8);
    crate::engine::latm_write::loas_frame_into(asc, bits, payload, &mut out)?;
    Ok(out)
}

/// One-shot surround encode (3, 4, 5, 6 or 8 planes): the push encoder's
/// raw access units under the requested container.
pub(super) fn encode_mc(pcm: &[&[f32]], sample_rate: u32, opts: &EncodeOptions) -> Result<Vec<u8>> {
    if opts.container == EncodeContainer::Raw {
        return Err(AacError::Unsupported(
            crate::UnsupportedFeature::EncodeRawOneShot,
        ));
    }
    let raw = opts.clone().with_container(EncodeContainer::Raw);
    let mut enc = crate::Encoder::new(sample_rate, pcm.len(), &raw)?;
    let mut aus: Vec<Vec<u8>> = Vec::new();
    enc.feed(pcm, |f| {
        aus.push(f.au.to_vec());
        Ok(())
    })?;
    let info = enc.finish(|f| {
        aus.push(f.au.to_vec());
        Ok(())
    })?;
    let fs = fs_index(sample_rate)?;
    let crate::Layout::Mpeg(cfg) = info.layout else {
        return Err(AacError::encode("encode: surround layout"));
    };
    match opts.container {
        EncodeContainer::Adts => Ok(crate::gapless::prefix_delay(
            super::wrap_adts(&aus, fs, usize::from(cfg)),
            info.priming,
            info.remainder,
            info.samples,
        )),
        EncodeContainer::Latm => wrap_latm(&aus, enc.asc()),
        _ => crate::m4a_write::mux_aac(
            &aus,
            enc.asc(),
            pcm.len(),
            sample_rate,
            info.samples,
            info.priming,
            1024,
        ),
    }
}
