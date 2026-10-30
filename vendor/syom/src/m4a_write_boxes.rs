//! Fixed-content `moov` boxes for [`super`] (line cap): `mvhd`, `tkhd`,
//! `edts`/`elst`, `mdhd`, `hdlr`, `dinf`.

use super::{be16, be32, be64, end_box, start_box};
use crate::error::{AacError, Result};

pub(super) fn rate_16_16(sample_rate: u32) -> Result<u32> {
    u32::try_from(u64::from(sample_rate) << 16)
        .map_err(|_| AacError::encode("m4a: sample rate exceeds stsd 16.16"))
}

/// 9 × u32 identity matrix (0x10000 / 0x40000000 fixed point).
pub(super) fn write_matrix(out: &mut Vec<u8>) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        be32(out, v);
    }
}

pub(super) fn write_mvhd(out: &mut Vec<u8>, timescale: u32, duration: u32) -> Result<()> {
    let b = start_box(out, b"mvhd");
    be32(out, 0); // version 0 + flags
    be32(out, 0); // creation_time
    be32(out, 0); // modification_time
    be32(out, timescale);
    be32(out, duration);
    be32(out, 0x0001_0000); // rate 1.0
    be16(out, 0x0100); // volume 1.0
    be16(out, 0);
    be64(out, 0);
    write_matrix(out);
    for _ in 0..6 {
        be32(out, 0); // pre_defined
    }
    be32(out, 2); // next_track_id
    end_box(out, b)
}

pub(super) fn write_tkhd(out: &mut Vec<u8>, duration: u32) -> Result<()> {
    let b = start_box(out, b"tkhd");
    be32(out, 0x0000_0003); // version 0, flags: enabled | in_movie
    be32(out, 0); // creation_time
    be32(out, 0); // modification_time
    be32(out, 1); // track_id
    be32(out, 0);
    be32(out, duration);
    be64(out, 0);
    be16(out, 0); // layer
    be16(out, 0); // alternate_group
    be16(out, 0x0100); // volume 1.0
    be16(out, 0);
    write_matrix(out);
    be32(out, 0); // width
    be32(out, 0); // height
    end_box(out, b)
}

/// `edts`/`elst` v0: play `segment_duration` movie units from `media_time`.
pub(super) fn write_edts(out: &mut Vec<u8>, segment_duration: u32, media_time: u32) -> Result<()> {
    let edts = start_box(out, b"edts");
    let elst = start_box(out, b"elst");
    be32(out, 0); // version 0 + flags
    be32(out, 1); // entry_count
    be32(out, segment_duration);
    be32(out, media_time);
    be16(out, 1); // media_rate_integer
    be16(out, 0); // media_rate_fraction
    end_box(out, elst)?;
    end_box(out, edts)
}

pub(super) fn write_mdhd(out: &mut Vec<u8>, sample_rate: u32, duration: u32) -> Result<()> {
    let b = start_box(out, b"mdhd");
    be32(out, 0); // version 0 + flags
    be32(out, 0); // creation_time
    be32(out, 0); // modification_time
    be32(out, sample_rate); // timescale
    be32(out, duration);
    be16(out, 0x55C4); // language: und
    be16(out, 0);
    end_box(out, b)
}

pub(super) fn write_hdlr(out: &mut Vec<u8>) -> Result<()> {
    let b = start_box(out, b"hdlr");
    be32(out, 0); // version/flags
    be32(out, 0); // pre_defined
    out.extend_from_slice(b"soun");
    be32(out, 0);
    be32(out, 0);
    be32(out, 0); // reserved
    out.push(0); // empty name
    end_box(out, b)
}

/// `dinf` with a self-contained `url ` data reference.
pub(super) fn write_dinf(out: &mut Vec<u8>) -> Result<()> {
    let dinf = start_box(out, b"dinf");
    let dref = start_box(out, b"dref");
    be32(out, 0); // version/flags
    be32(out, 1); // entry_count
    let url = start_box(out, b"url ");
    be32(out, 1); // version 0, flags 1: media data is in this file
    end_box(out, url)?;
    end_box(out, dref)?;
    end_box(out, dinf)
}
