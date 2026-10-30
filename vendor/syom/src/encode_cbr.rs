//! ADTS framing for one-shot encode. Default fullness is `0x7FF`
//! (VBR, no reservoir). Opt-in CBR passes one fullness per frame.

use crate::engine::adts::{ADTS_HEADER_BYTES_NO_CRC, AdtsHeader};

/// Append one ADTS frame. `channels` is the channel configuration.
pub(crate) fn adts_frame_into(payload: &[u8], fs_index: u8, channels: usize, out: &mut Vec<u8>) {
    adts_frame_fullness(payload, fs_index, channels, 0x7FF, out);
}

/// [`adts_frame_into`] with an explicit `adts_buffer_fullness`.
pub(crate) fn adts_frame_fullness(
    payload: &[u8],
    fs_index: u8,
    channels: usize,
    fullness: u16,
    out: &mut Vec<u8>,
) {
    let hdr = AdtsHeader {
        mpeg_version_mpeg2: false,
        protection_absent: true,
        profile: 1, // LC
        sampling_frequency_index: fs_index,
        channel_configuration: channels as u8,
        aac_frame_length: (ADTS_HEADER_BYTES_NO_CRC + payload.len()) as u16,
        adts_buffer_fullness: fullness,
        number_of_raw_data_blocks_in_frame: 1,
    };
    out.extend_from_slice(&hdr.write());
    out.extend_from_slice(payload);
}

/// ADTS-wrap each `raw_data_block` (no CRC, VBR fullness marker).
pub(super) fn wrap_adts(payloads: &[Vec<u8>], fs_index: u8, channels: usize) -> Vec<u8> {
    wrap_with_fullness(payloads, fs_index, channels, &[])
}

/// Like [`wrap_adts`], but each frame carries `fullness[i]` (CBR).
/// An empty `fullness` slice keeps the VBR marker on every frame.
pub(super) fn wrap_with_fullness(
    payloads: &[Vec<u8>],
    fs_index: u8,
    channels: usize,
    fullness: &[u16],
) -> Vec<u8> {
    let total: usize = payloads
        .iter()
        .map(|p| p.len() + ADTS_HEADER_BYTES_NO_CRC)
        .sum();
    let mut out = Vec::with_capacity(total);
    for (i, p) in payloads.iter().enumerate() {
        let full = if fullness.is_empty() {
            0x7FF
        } else {
            fullness.get(i).copied().unwrap_or(0)
        };
        adts_frame_fullness(p, fs_index, channels, full, &mut out);
    }
    out
}
