//! ADTS `crc_check` — ISO/IEC 13818-7 §6.2.3 / §8.1.1.1 with the
//! ISO/IEC 11172-3 §2.4.3.1 CRC-16 (not MPEG-4 Audio §1.8.4.5).
//!
//! Protected bits (13818-7:2004 §8.1.1.1, matching the oxideav-aac and
//! US20030033569 field lists): all 56 ADTS header bits; for
//! `adts_header_error_check`, every `raw_data_block_position` field;
//! per `raw_data_block`, the first 192 bits of each SCE/CPE/CCE/LFE
//! *after* `id_syn_ele` (short elements zero-padded), the first 128 bits
//! of a CPE's second `individual_channel_stream`, and every bit of PCE
//! and DSE. FIL is skipped, not protected.

use super::adts::{ADTS_HEADER_BYTES_NO_CRC, AdtsHeader};
use super::bits::BitReader;
use super::channel_map::parse_pce;
use super::crc::mpeg1_crc16;
use super::error::{Error, Result};
use super::ics::IcsInfo;
use super::ics_body::parse_ics;
use super::raw_data_block::IdSynEle;
use super::skip::{fill_count, parse_cce, skip_dse};
use super::stereo::MsInfo;

/// Verify every `crc_check` in a complete ADTS frame. CRC-free frames
/// (`protection_absent`) return `Ok` without reading a CRC.
pub fn verify_adts_crc(frame: &[u8], hdr: &AdtsHeader) -> Result<()> {
    if hdr.protection_absent {
        return Ok(());
    }
    let n = hdr.number_of_raw_data_blocks_in_frame.max(1);
    let frame_len = usize::from(hdr.aac_frame_length);
    if frame.len() < frame_len {
        return Err(Error::UnexpectedEnd);
    }
    let frame = &frame[..frame_len];
    let aot = hdr.audio_object_type();
    let fs = hdr.sampling_frequency_index;
    if n == 1 {
        if frame.len() < ADTS_HEADER_BYTES_NO_CRC + 2 {
            return Err(Error::UnexpectedEnd);
        }
        let stored = u16::from_be_bytes([frame[7], frame[8]]);
        let mut bits = header_bits(frame);
        bits.extend(rdb_protected(&frame[9..], fs, aot)?.0);
        if mpeg1_crc16(&bits) != stored {
            return Err(Error::AdtsCrcMismatch);
        }
        return Ok(());
    }
    let pos_bytes = 2 * usize::from(n - 1);
    let crc_at = ADTS_HEADER_BYTES_NO_CRC + pos_bytes;
    if frame.len() < crc_at + 2 {
        return Err(Error::UnexpectedEnd);
    }
    let stored = u16::from_be_bytes([frame[crc_at], frame[crc_at + 1]]);
    let mut bits = header_bits(frame);
    push_range(&mut bits, frame, 56, 56 + u64::from(n - 1) * 16);
    if mpeg1_crc16(&bits) != stored {
        return Err(Error::AdtsCrcMismatch);
    }
    let mut rest = &frame[crc_at + 2..];
    for _ in 0..n {
        let (prot, used) = rdb_protected(rest, fs, aot)?;
        if rest.len() < used + 2 {
            return Err(Error::UnexpectedEnd);
        }
        let blk = u16::from_be_bytes([rest[used], rest[used + 1]]);
        if mpeg1_crc16(&prot) != blk {
            return Err(Error::AdtsCrcMismatch);
        }
        rest = &rest[used + 2..];
    }
    Ok(())
}

fn header_bits(frame: &[u8]) -> Vec<bool> {
    let mut v = Vec::with_capacity(56);
    push_range(&mut v, frame, 0, 56);
    v
}

fn push_range(out: &mut Vec<bool>, data: &[u8], start: u64, end: u64) {
    for pos in start..end {
        let byte = data[(pos / 8) as usize];
        out.push(byte & (0x80 >> (pos % 8)) != 0);
    }
}

fn feed_padded(out: &mut Vec<bool>, data: &[u8], start: u64, end: u64, width: u64) {
    let mut n = 0u64;
    for pos in start..end {
        if n >= width {
            break;
        }
        let byte = data[(pos / 8) as usize];
        out.push(byte & (0x80 >> (pos % 8)) != 0);
        n += 1;
    }
    while n < width {
        out.push(false);
        n += 1;
    }
}

/// Protected bits of one `raw_data_block()` and its byte-aligned length.
fn rdb_protected(payload: &[u8], fs_index: u8, aot: u8) -> Result<(Vec<bool>, usize)> {
    let mut br = BitReader::new(payload);
    let mut prot = Vec::new();
    loop {
        if br.bits_remaining() < 3 {
            break;
        }
        let id = IdSynEle::from_bits(br.read(3)? as u8);
        if matches!(id, IdSynEle::End) {
            break;
        }
        let body_start = br.bit_position();
        match id {
            IdSynEle::Sce | IdSynEle::Lfe => {
                let _tag = br.read(4)?;
                let _ = parse_ics(&mut br, fs_index, aot, None)?;
                feed_padded(&mut prot, payload, body_start, br.bit_position(), 192);
            }
            IdSynEle::Cpe => {
                let _tag = br.read(4)?;
                let common = br.read_bit()?;
                let ics_common = if common {
                    let ics = IcsInfo::parse(&mut br, fs_index, true)?;
                    let _ms = MsInfo::parse(&mut br, &ics)?;
                    Some(ics)
                } else {
                    None
                };
                let _ = parse_ics(&mut br, fs_index, aot, ics_common.as_ref())?;
                let ics2_start = br.bit_position();
                let _ = parse_ics(&mut br, fs_index, aot, ics_common.as_ref())?;
                let end = br.bit_position();
                feed_padded(&mut prot, payload, body_start, end, 192);
                feed_padded(&mut prot, payload, ics2_start, end, 128);
            }
            IdSynEle::Cce => {
                parse_cce(&mut br, fs_index, aot, 0)?;
                feed_padded(&mut prot, payload, body_start, br.bit_position(), 192);
            }
            IdSynEle::Dse => {
                skip_dse(&mut br)?;
                push_range(&mut prot, payload, body_start, br.bit_position());
            }
            IdSynEle::Pce => {
                let _ = parse_pce(&mut br)?;
                push_range(&mut prot, payload, body_start, br.bit_position());
            }
            IdSynEle::Fil => {
                let cnt = fill_count(&mut br)?;
                br.skip(cnt.saturating_mul(8))?;
            }
            IdSynEle::End => break,
        }
    }
    br.byte_align()?;
    Ok((prot, (br.bit_position() / 8) as usize))
}

#[cfg(test)]
#[path = "adts_crc_tests.rs"]
mod adts_crc_tests;
