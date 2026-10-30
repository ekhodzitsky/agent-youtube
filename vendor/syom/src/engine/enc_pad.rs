//! ABR stuffing as EXT_FILL `fill_element()`s (TASK-121): undersized
//! frames are padded *before* `ID_END` with spec-legal filler, not with
//! zero bytes after it — fdk-aac (the AOSP platform decoder codebase)
//! keeps parsing past `ID_END` and answers trailing zeros with
//! `AAC_DEC_UNKNOWN`, killing the stream. Payload bytes are zero:
//! `extension_type` EXT_FILL (`0b0000`) followed by ignored `other_bits`.

use super::bits::BitWriter;
use super::enc_sbr_bits::FILL_MAX_BYTES;

/// Zero payload for one maximally-sized EXT_FILL `fill_element()`.
static ZEROES: [u8; FILL_MAX_BYTES] = [0; FILL_MAX_BYTES];

/// Bits needed to emit `pad` EXT_FILL payload bytes, chunked to the
/// `fill_element()` count syntax (≤ [`FILL_MAX_BYTES`] each).
#[must_use]
pub(crate) fn pad_fill_bits(mut pad: usize) -> usize {
    let mut bits = 0;
    while pad > 0 {
        let n = pad.min(FILL_MAX_BYTES);
        bits += 3 + 4 + 8 * n + usize::from(n >= 15) * 8;
        pad -= n;
    }
    bits
}

/// Largest EXT_FILL payload whose [`pad_fill_bits`] fits `room` bits.
#[must_use]
pub(crate) fn pad_fill_for_room(room: usize) -> usize {
    let mut pad = room / 8;
    while pad > 0 && pad_fill_bits(pad) > room {
        pad -= 1;
    }
    pad
}

/// Emit `pad` EXT_FILL payload bytes as `fill_element()`s (before END).
pub(crate) fn write_pad_fill(w: &mut BitWriter, mut pad: usize) {
    while pad > 0 {
        let n = pad.min(FILL_MAX_BYTES);
        // 1..=FILL_MAX_BYTES never fails the count syntax.
        let _ = super::enc_sbr_bits::write_fill_element(w, &ZEROES[..n]);
        pad -= n;
    }
}

#[cfg(test)]
#[path = "enc_pad_tests.rs"]
mod enc_pad_tests;
