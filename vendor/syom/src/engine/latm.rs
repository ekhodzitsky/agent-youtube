//! LATM / LOAS transport — ISO/IEC 14496-3 §1.7 (AAC payloads).
//!
//! The resumable byte-stream state machine lives in `crate::stream`; this
//! module owns the wire structures it reuses: the LOAS syncword,
//! `StreamMuxConfig()` (sticky across packets), and payload extraction.

use super::asc::AudioSpecificConfig;
use super::bits::BitReader;
use super::crc::stream_mux_config_crc;
use super::error::{Error, Result};

/// LOAS `AudioSyncStream` syncword.
pub const LOAS_SYNC: u32 = 0x2B7;

/// Parsed `StreamMuxConfig()` for the AAC-single-stream subset.
pub(crate) struct MuxCfg {
    pub(crate) asc: AudioSpecificConfig,
    pub(crate) frame_length_type: u8,
    pub(crate) frame_length: u32,
    /// Wire `numSubFrames`; access units in one AudioMuxElement = this + 1.
    pub(crate) num_sub_frames: u8,
}

impl MuxCfg {
    pub(crate) fn parse(br: &mut BitReader<'_>) -> Result<Self> {
        // The crcCheckSum covers StreamMuxConfig() from audioMuxVersion
        // up to but excluding crcCheckPresent (§1.7.3.1 Table 1.42).
        let cfg_start = br.bit_position();
        let audio_mux_version = br.read_bit()?;
        if audio_mux_version {
            let audio_mux_version_a = br.read_bit()?;
            if audio_mux_version_a {
                return Err(Error::LatmAudioMuxVersionAReserved);
            }
            let _tara = latm_value(br)?;
        }
        let _same_time = br.read_bit()?;
        let num_sub_frames = br.read(6)? as u8;
        let num_program = br.read(4)?;
        let num_layer = br.read(3)?;
        if num_program != 0 || num_layer != 0 {
            return Err(Error::LatmConfigOutOfRange);
        }
        if audio_mux_version {
            let asc_len = latm_value(br)?;
            let start = br.bit_position();
            let (asc, _) = AudioSpecificConfig::parse_from_reader(br)?;
            let used = br.bit_position().saturating_sub(start);
            if used > u64::from(asc_len) {
                return Err(Error::Format("LATM ASC longer than ascLen"));
            }
            if used < u64::from(asc_len) {
                br.skip((u64::from(asc_len) - used) as u32)?;
            }
            let frame_length_type = br.read(3)? as u8;
            let frame_length = read_frame_len(br, frame_length_type)?;
            skip_mux_tail(br, cfg_start, true)?;
            return Ok(Self {
                asc,
                frame_length_type,
                frame_length,
                num_sub_frames,
            });
        }
        let (asc, _) = AudioSpecificConfig::parse_from_reader(br)?;
        let frame_length_type = br.read(3)? as u8;
        let frame_length = read_frame_len(br, frame_length_type)?;
        skip_mux_tail(br, cfg_start, false)?;
        Ok(Self {
            asc,
            frame_length_type,
            frame_length,
            num_sub_frames,
        })
    }
}

fn read_frame_len(br: &mut BitReader<'_>, ty: u8) -> Result<u32> {
    match ty {
        0 => {
            let _ = br.read(8)?; // latmBufferFullness
            Ok(0)
        }
        1 => Ok(br.read(9)?),
        _ => Err(Error::LatmUnsupportedFrameLengthType),
    }
}

fn skip_mux_tail(br: &mut BitReader<'_>, cfg_start: u64, audio_mux_version: bool) -> Result<()> {
    let other = br.read_bit()?;
    if other {
        if audio_mux_version {
            // ISO/IEC 14496-3: otherDataLenBits = latmGetValue()
            let _ = latm_value(br)?;
        } else {
            let escaped = br.read_bit()?;
            if escaped {
                loop {
                    let more = br.read_bit()?;
                    let _ = br.read(8)?;
                    if !more {
                        break;
                    }
                }
            } else {
                let n = br.read(8)? + 1;
                br.skip(n)?;
            }
        }
    }
    // crcCheckSum covers StreamMuxConfig() up to but excluding
    // crcCheckPresent (§1.7.3.1 Table 1.42).
    let crc_end = br.bit_position();
    let crc = br.read_bit()?;
    if crc {
        let expected = br.read(8)? as u8;
        let computed = stream_mux_config_crc(&br.bits_range(cfg_start, crc_end));
        if computed != expected {
            return Err(Error::LatmCrcMismatch);
        }
    }
    Ok(())
}

/// `latmGetValue()`: 2-bit `bytesForValue` then `(n+1)*8` bits
/// (FFmpeg `latm_get_value`, FDK `LatmGetValue`, FAAD2).
fn latm_value(br: &mut BitReader<'_>) -> Result<u32> {
    let n = br.read(2)?;
    br.read((n + 1) * 8)
}

pub(crate) fn read_payload(br: &mut BitReader<'_>, cfg: &MuxCfg) -> Result<Vec<u8>> {
    let nbytes = if cfg.frame_length_type == 1 {
        cfg.frame_length.div_ceil(8) as usize
    } else {
        let mut n = 0u32;
        loop {
            let b = br.read(8)?;
            n += b;
            if b != 255 {
                break;
            }
        }
        n as usize
    };
    let mut out = Vec::with_capacity(nbytes);
    for _ in 0..nbytes {
        out.push(br.read(8)? as u8);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "latm_tests.rs"]
mod latm_tests;
