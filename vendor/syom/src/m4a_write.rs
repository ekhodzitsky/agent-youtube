//! Minimal ISOBMFF writer for AAC-LC: `ftyp` + `mdat` + `moov` (after
//! `mdat`, so `stco` needs no patching). One sound track, one chunk,
//! `stts` (n, 1024), per-frame `stsz`, absolute `stco`.
//!
//! Ceiling: version-0 32-bit boxes and `stco` (not `co64` / largesize).
//! Every size, offset, sample count and duration must fit in `u32` or the
//! mux errors before returning bytes. Max file size is [`MAX_M4A_V0_BYTES`].
//! A later seekable/large-file sink can emit `co64` using the same checks.
//!
//! Timeline (Apple QA1636): movie timescale = sample rate so counts are
//! exact. `elst.media_time` = priming; `elst.segment_duration` /
//! `mvhd`/`tkhd` duration = valid source samples; `mdhd` duration =
//! coded `n_frames * 1024`. Remainder is the unplayed media tail
//! `coded − priming − valid`. Decode skips `media_time` and caps
//! emission at the presentation duration.
//!
//! The reader side ([`crate::isomp4`]) is the structural oracle: its
//! `parse_esds` walk defines exactly what this writer emits.

use crate::engine::asc::{write_he, write_lc};
use crate::error::{AacError, Result};

#[path = "m4a_write_boxes.rs"]
mod boxes;
use boxes::{rate_16_16, write_dinf, write_edts, write_hdlr, write_mdhd, write_mvhd, write_tkhd};

/// AAC-LC `objectTypeIndication`.
const OBJECT_TYPE_AAC: u8 = 0x40;
/// streamType: audio (5) << 2 | upstream flag 0 | reserved 1.
const STREAM_TYPE_AUDIO: u8 = 0x15;
/// Version-0 box / `stco` ceiling. Larger files need `co64` (TASK-60).
pub(crate) const MAX_M4A_V0_BYTES: u64 = u32::MAX as u64;

pub(crate) fn u32_field(v: u64, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| AacError::encode(format!("m4a: {what} exceeds u32")))
}

fn box_size(total: usize, at: usize) -> Result<u32> {
    let size = total
        .checked_sub(at)
        .ok_or_else(|| AacError::encode("m4a: box size underflow"))?;
    u32_field(size as u64, "box size")
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Start a box; returns the position of its size field for [`end_box`].
fn start_box(out: &mut Vec<u8>, typ: &[u8; 4]) -> usize {
    let at = out.len();
    be32(out, 0);
    out.extend_from_slice(typ);
    at
}

fn end_box(out: &mut [u8], at: usize) -> Result<()> {
    let size = box_size(out.len(), at)?;
    out[at..at + 4].copy_from_slice(&size.to_be_bytes());
    Ok(())
}

/// `ftyp M4A ` + `mdat` + `moov` for LC payloads (one `raw_data_block` per
/// sample entry). `valid_samples` is source length N; `priming` is encoder
/// delay in media units (1024 for LC). Remainder is implied:
/// `n_frames*1024 − priming − valid_samples`.
pub fn mux_aac_lc(
    payloads: &[Vec<u8>],
    fs_index: u8,
    channels: usize,
    sample_rate: u32,
    valid_samples: u64,
    priming: u64,
) -> Result<Vec<u8>> {
    let ch8 =
        u8::try_from(channels).map_err(|_| AacError::encode("m4a: channel count exceeds u8"))?;
    mux_aac(
        payloads,
        &write_lc(fs_index, ch8),
        channels,
        sample_rate,
        valid_samples,
        priming,
        1024,
    )
}

/// HE-AAC v1: explicit two-rate AOT 5 `esds`, timeline at the SBR output
/// rate (`stts` 2048 per access unit, `elst.media_time` = `priming`).
pub fn mux_aac_he(
    payloads: &[Vec<u8>],
    core_fs_index: u8,
    out_fs_index: u8,
    channels: usize,
    out_rate: u32,
    valid_samples: u64,
    priming: u64,
) -> Result<Vec<u8>> {
    let ch8 =
        u8::try_from(channels).map_err(|_| AacError::encode("m4a: channel count exceeds u8"))?;
    let asc = write_he(core_fs_index, out_fs_index, ch8);
    mux_aac(
        payloads,
        &asc,
        channels,
        out_rate,
        valid_samples,
        priming,
        2048,
    )
}

/// Shared writer: `frame_len` media units per access unit.
pub(crate) fn mux_aac(
    payloads: &[Vec<u8>],
    asc: &[u8],
    channels: usize,
    sample_rate: u32,
    valid_samples: u64,
    priming: u64,
    frame_len: u32,
) -> Result<Vec<u8>> {
    let mut sizes = Vec::with_capacity(payloads.len());
    let mut payload_bytes = 0u64;
    for p in payloads {
        let n = u32_field(p.len() as u64, "sample size")?;
        payload_bytes = payload_bytes
            .checked_add(u64::from(n))
            .ok_or_else(|| AacError::encode("m4a: payload length overflow"))?;
        sizes.push(n);
    }
    let mut out = ftyp_bytes();
    let media_offset = u32_field(out.len() as u64 + 8, "mdat offset")?;
    preflight(media_offset, payload_bytes, sizes.len())?;
    // mdat: payloads contiguous; the single stco entry points here.
    let mdat = start_box(&mut out, b"mdat");
    for p in payloads {
        out.extend_from_slice(p);
    }
    end_box(&mut out, mdat)?;
    let plan = MoovPlan {
        asc,
        channels,
        sample_rate,
        valid_samples,
        priming,
        frame_len,
    };
    out.extend_from_slice(&moov_bytes(&plan, &sizes, media_offset)?);
    u32_field(out.len() as u64, "file size")?;
    Ok(out)
}

/// `ftyp M4A ` (the fixed 32-byte prefix).
pub(crate) fn ftyp_bytes() -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    let ftyp = start_box(&mut out, b"ftyp");
    out.extend_from_slice(b"M4A ");
    be32(&mut out, 0); // minor version
    out.extend_from_slice(b"M4A mp42isom");
    let _ = end_box(&mut out, ftyp); // 32 bytes: never overflows
    out
}

/// Access units the sample tables may index (4 bytes of `stsz` each,
/// 8 B with the frame count): the retained index of a streamed M4A.
pub(crate) const MAX_M4A_AUS: usize = 1 << 20;

/// Version-0 ceiling check before anything is committed: every `stco`
/// offset, the `mdat` size and the file size must fit `u32`, and the
/// sample index stays under [`MAX_M4A_AUS`]. No field ever wraps.
pub(crate) fn preflight(media_offset: u32, payload_bytes: u64, n_aus: usize) -> Result<()> {
    if n_aus > MAX_M4A_AUS {
        return Err(AacError::encode(
            "m4a: access units exceed the 2^20 index budget",
        ));
    }
    let planned = u64::from(media_offset)
        .checked_add(payload_bytes)
        .and_then(|v| v.checked_add(n_aus as u64 * 4))
        .and_then(|v| v.checked_add(1024))
        .ok_or_else(|| AacError::encode("m4a: file size overflow"))?;
    if planned > MAX_M4A_V0_BYTES {
        return Err(AacError::encode(
            "m4a: file exceeds 32-bit box/stco ceiling",
        ));
    }
    Ok(())
}

/// What `moov` needs besides the sample sizes and the media offset.
pub(crate) struct MoovPlan<'a> {
    pub asc: &'a [u8],
    pub channels: usize,
    pub sample_rate: u32,
    pub valid_samples: u64,
    pub priming: u64,
    pub frame_len: u32,
}

/// The complete `moov` box: movie timescale = sample rate so `elst`
/// duration is N exactly; one chunk at `media_offset`.
pub(crate) fn moov_bytes(plan: &MoovPlan<'_>, sizes: &[u32], media_offset: u32) -> Result<Vec<u8>> {
    if sizes.is_empty() {
        return Err(AacError::encode("m4a: no access units"));
    }
    if plan.sample_rate == 0 {
        return Err(AacError::encode("m4a: sample rate is 0"));
    }
    let n_frames = u32_field(sizes.len() as u64, "frame count")?;
    let coded = u64::from(n_frames)
        .checked_mul(u64::from(plan.frame_len))
        .ok_or_else(|| AacError::encode("m4a: coded duration overflow"))?;
    let play_end = plan
        .priming
        .checked_add(plan.valid_samples)
        .ok_or_else(|| AacError::encode("m4a: priming + valid overflow"))?;
    if play_end > coded {
        return Err(AacError::encode(
            "m4a: valid samples exceed coded duration after priming",
        ));
    }
    let presentation = u32_field(plan.valid_samples, "presentation duration")?;
    let media_duration = u32_field(coded, "media duration")?;
    let media_time = u32_field(plan.priming, "priming")?;
    let payload_bytes: u64 = sizes.iter().map(|&n| u64::from(n)).sum();
    let bitrate = payload_bytes
        .checked_mul(8)
        .and_then(|b| b.checked_mul(u64::from(plan.sample_rate)))
        .and_then(|b| b.checked_div(coded))
        .and_then(|b| u32::try_from(b).ok())
        .unwrap_or(u32::MAX);
    let ch8 = u8::try_from(plan.channels)
        .map_err(|_| AacError::encode("m4a: channel count exceeds u8"))?;
    let rate_fixed = rate_16_16(plan.sample_rate)?;
    let mut out = Vec::new();
    let moov = start_box(&mut out, b"moov");
    write_mvhd(&mut out, plan.sample_rate, presentation)?;
    let trak = start_box(&mut out, b"trak");
    write_tkhd(&mut out, presentation)?;
    write_edts(&mut out, presentation, media_time)?;
    let mdia = start_box(&mut out, b"mdia");
    write_mdhd(&mut out, plan.sample_rate, media_duration)?;
    write_hdlr(&mut out)?;
    let minf = start_box(&mut out, b"minf");
    let smhd = start_box(&mut out, b"smhd");
    be32(&mut out, 0); // version/flags
    be16(&mut out, 0); // balance
    be16(&mut out, 0);
    end_box(&mut out, smhd)?;
    write_dinf(&mut out)?;
    let stbl = start_box(&mut out, b"stbl");
    write_stsd(&mut out, plan.asc, u16::from(ch8), rate_fixed, bitrate)?;
    write_stts(&mut out, n_frames, plan.frame_len)?;
    write_stsc(&mut out, n_frames)?;
    write_stsz(&mut out, n_frames, sizes)?;
    let stco = start_box(&mut out, b"stco");
    be32(&mut out, 0);
    be32(&mut out, 1);
    be32(&mut out, media_offset);
    end_box(&mut out, stco)?;
    end_box(&mut out, stbl)?;
    end_box(&mut out, minf)?;
    end_box(&mut out, mdia)?;
    end_box(&mut out, trak)?;
    end_box(&mut out, moov)?;
    Ok(out)
}

/// `stsd` with one `mp4a` AudioSampleEntry v0 carrying `esds`.
fn write_stsd(
    out: &mut Vec<u8>,
    asc: &[u8],
    channels: u16,
    rate_fixed: u32,
    bitrate: u32,
) -> Result<()> {
    let stsd = start_box(out, b"stsd");
    be32(out, 0); // version/flags
    be32(out, 1); // entry_count
    let mp4a = start_box(out, b"mp4a");
    for _ in 0..6 {
        out.push(0); // reserved
    }
    be16(out, 1); // data_reference_index
    be16(out, 0); // version 0
    be16(out, 0); // revision
    be32(out, 0); // vendor
    be16(out, channels);
    be16(out, 16); // sample size
    be16(out, 0); // compression id
    be16(out, 0); // packet size
    be32(out, rate_fixed);
    write_esds(out, asc, bitrate)?;
    end_box(out, mp4a)?;
    end_box(out, stsd)
}

/// `esds`: version/flags, then the descriptor tree ES_Descriptor 0x03 →
/// DecoderConfig 0x04 (objectType 0x40, streamType 0x15) →
/// DecoderSpecificInfo 0x05 = ASC, → SLConfig 0x06 predefined 2. All
/// descriptor bodies are under 128 bytes, so lengths are single bytes.
fn write_esds(out: &mut Vec<u8>, asc: &[u8], bitrate: u32) -> Result<()> {
    let esds = start_box(out, b"esds");
    be32(out, 0); // version/flags
    let dc_len = 13 + 2 + asc.len() + 2 + 1;
    let es_len = 3 + 2 + dc_len;
    let dc_len = u8_desc(dc_len, "decoder config")?;
    let es_len = u8_desc(es_len, "ES descriptor")?;
    let asc_len = u8_desc(asc.len(), "ASC")?;
    out.push(0x03);
    out.push(es_len);
    be16(out, 1); // ES_ID
    out.push(0); // flags: no stream dependence / URL / OCR
    out.push(0x04);
    out.push(dc_len);
    out.push(OBJECT_TYPE_AAC);
    out.push(STREAM_TYPE_AUDIO);
    // bufferSizeDB
    out.push(0);
    out.push(0);
    out.push(0);
    be32(out, bitrate); // maxBitrate
    be32(out, bitrate); // avgBitrate
    out.push(0x05);
    out.push(asc_len);
    out.extend_from_slice(asc);
    out.push(0x06);
    out.push(1);
    out.push(2); // predefined = 2
    end_box(out, esds)
}

fn u8_desc(v: usize, what: &str) -> Result<u8> {
    if v >= 128 {
        return Err(AacError::encode(format!(
            "m4a: {what} needs multi-byte descriptor length"
        )));
    }
    u8::try_from(v).map_err(|_| AacError::encode(format!("m4a: {what} exceeds u8")))
}

/// `stts` v0: every sample is 1024 media units.
fn write_stts(out: &mut Vec<u8>, n_frames: u32, frame_len: u32) -> Result<()> {
    let b = start_box(out, b"stts");
    be32(out, 0);
    be32(out, 1); // entry_count
    be32(out, n_frames);
    be32(out, frame_len);
    end_box(out, b)
}

/// `stsc` v0: one chunk holding all samples.
fn write_stsc(out: &mut Vec<u8>, n_frames: u32) -> Result<()> {
    let b = start_box(out, b"stsc");
    be32(out, 0);
    be32(out, 1); // entry_count
    be32(out, 1); // first_chunk (1-based)
    be32(out, n_frames); // samples_per_chunk
    be32(out, 1); // sample_description_index
    end_box(out, b)
}

/// `stsz` v0 with per-frame sizes (already checked to fit `u32`).
fn write_stsz(out: &mut Vec<u8>, n_frames: u32, sizes: &[u32]) -> Result<()> {
    let b = start_box(out, b"stsz");
    be32(out, 0);
    be32(out, 0); // sample_size 0 ⇒ per-entry table
    be32(out, n_frames);
    for &n in sizes {
        be32(out, n);
    }
    end_box(out, b)
}

#[cfg(test)]
#[path = "m4a_write_tests.rs"]
mod m4a_write_tests;
