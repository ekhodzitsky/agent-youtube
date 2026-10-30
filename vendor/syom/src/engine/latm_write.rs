//! LATM / LOAS writer (TASK-94): one `AudioSyncStream` frame per access
//! unit — `AudioMuxElement(1)` with `StreamMuxConfig()` in **every**
//! frame (`useSameStreamMux = 0`, so any frame is a sync point and a
//! config refresh), `audioMuxVersion 0`, one program / layer / subframe,
//! `frameLengthType 0` (8-bit length chunks), `latmBufferFullness 0xFF`,
//! no other data, no CRC. Mirrors [`super::latm::MuxCfg::parse`] and
//! [`super::latm::read_payload`], which are the in-tree oracle.

use super::asc::AudioSpecificConfig;
use super::bits::{BitReader, BitWriter};
use super::error::{Error, Result};
use super::latm::LOAS_SYNC;

/// Largest `audioMuxLengthBytes` the 13-bit LOAS header can carry.
pub const LOAS_MAX_MUX_BYTES: usize = 0x1FFF;

/// The `AudioSpecificConfig` bit count inside `asc` (its bytes end with
/// alignment padding that `StreamMuxConfig` must not carry).
pub(crate) fn asc_bit_len(asc: &[u8]) -> Result<u32> {
    let (_, bits) = AudioSpecificConfig::parse(asc)?;
    u32::try_from(bits).map_err(|_| Error::LatmConfigOutOfRange)
}

/// Append one LOAS frame carrying `au` under the config `asc`
/// (`asc_bits` from [`asc_bit_len`]).
pub(crate) fn loas_frame_into(
    asc: &[u8],
    asc_bits: u32,
    au: &[u8],
    out: &mut Vec<u8>,
) -> Result<()> {
    let mut w = BitWriter::new();
    w.write_bit(false); // useSameStreamMux: config follows
    w.write_bit(false); // audioMuxVersion 0
    w.write_bit(true); // allStreamsSameTimeFraming
    w.write(0, 6); // numSubFrames
    w.write(0, 4); // numProgram
    w.write(0, 3); // numLayer
    let mut br = BitReader::new(asc);
    for _ in 0..asc_bits {
        w.write_bit(br.read_bit()?);
    }
    w.write(0, 3); // frameLengthType: payload length in bytes
    w.write(0xFF, 8); // latmBufferFullness: not a CBR stream
    w.write_bit(false); // otherDataPresent
    w.write_bit(false); // crcCheckPresent
    let mut left = au.len();
    while left >= 255 {
        w.write(255, 8);
        left -= 255;
    }
    w.write(left as u32, 8);
    au.iter().for_each(|&b| w.write(u32::from(b), 8));
    let body = w.finish();
    if body.len() > LOAS_MAX_MUX_BYTES {
        return Err(Error::LatmConfigOutOfRange);
    }
    let hdr = (LOAS_SYNC << 13) | body.len() as u32;
    out.extend_from_slice(&[(hdr >> 16) as u8, (hdr >> 8) as u8, hdr as u8]);
    out.extend_from_slice(&body);
    Ok(())
}
