//! Scalefactor-band offsets — ISO/IEC 14496-3 Tables 4.129–4.141.

use super::error::{Error, Result};

/// Long-window spectrum length (1024-line family).
pub const LONG_WINDOW_LEN: usize = 1024;
/// Short-window spectrum length (one of eight).
pub const SHORT_WINDOW_LEN: usize = 128;
/// AAC-LD spectrum length (512-line frames, long windows only).
pub const LD_WINDOW_LEN: usize = 512;

/// Table 4.129 — 44.1 / 48 kHz long (49 SWB).
const LONG_48K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 1024,
];
/// Table 4.131 — 32 kHz long (51 SWB).
const LONG_32K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 960, 992, 1024,
];
/// Table 4.132 — 8 kHz long (40 SWB).
const LONG_8K: &[u16] = &[
    0, 12, 24, 36, 48, 60, 72, 84, 96, 108, 120, 132, 144, 156, 172, 188, 204, 220, 236, 252, 268,
    288, 308, 328, 348, 372, 396, 420, 448, 476, 508, 544, 580, 620, 664, 712, 764, 820, 880, 944,
    1024,
];
/// Table 4.134 — 11.025 / 12 / 16 kHz long (43 SWB).
const LONG_16K: &[u16] = &[
    0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 100, 112, 124, 136, 148, 160, 172, 184, 196, 212,
    228, 244, 260, 280, 300, 320, 344, 368, 396, 424, 456, 492, 532, 572, 616, 664, 716, 772, 832,
    896, 960, 1024,
];
/// Table 4.136 — 22.05 / 24 kHz long (47 SWB).
const LONG_24K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 76, 84, 92, 100, 108, 116, 124, 136,
    148, 160, 172, 188, 204, 220, 240, 260, 284, 308, 336, 364, 396, 432, 468, 508, 552, 600, 652,
    704, 768, 832, 896, 960, 1024,
];
/// Table 4.138 — 64 kHz long (47 SWB).
const LONG_64K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 100, 112, 124, 140,
    156, 172, 192, 216, 240, 268, 304, 344, 384, 424, 464, 504, 544, 584, 624, 664, 704, 744, 784,
    824, 864, 904, 944, 984, 1024,
];
/// Table 4.140 — 88.2 / 96 kHz long (41 SWB).
const LONG_96K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 96, 108, 120, 132,
    144, 156, 172, 188, 212, 240, 276, 320, 384, 448, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
];

/// Table 4.130 — 32 / 44.1 / 48 kHz short (14 SWB).
const SHORT_48K: &[u16] = &[0, 4, 8, 12, 16, 20, 28, 36, 44, 56, 68, 80, 96, 112, 128];
/// Table 4.133 — 8 kHz short (15 SWB).
const SHORT_8K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 60, 72, 88, 108, 128,
];
/// Table 4.135 — 11.025 / 12 / 16 kHz short (15 SWB).
const SHORT_16K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 60, 72, 88, 108, 128,
];
/// Table 4.137 — 22.05 / 24 kHz short (15 SWB).
const SHORT_24K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 64, 76, 92, 108, 128,
];
/// Table 4.139 / 4.141 — 64 / 88.2 / 96 kHz short (12 SWB).
const SHORT_64K: &[u16] = &[0, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64, 92, 128];

/// ER AAC LD 512-line long — 44.1–96 kHz (36 SWB).
const LD_512_48K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 68, 76, 84, 92, 100, 112, 124,
    136, 148, 164, 184, 208, 236, 268, 300, 332, 364, 396, 428, 460, 512,
];
/// ER AAC LD 512-line long — 32 kHz (37 SWB).
const LD_512_32K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 96, 108, 120, 132,
    144, 160, 176, 192, 212, 236, 260, 288, 320, 352, 384, 416, 448, 480, 512,
];
/// ER AAC LD 512-line long — 7.35–24 kHz (31 SWB).
const LD_512_24K: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 80, 92, 104, 120, 140, 164, 192, 224,
    256, 288, 320, 352, 384, 416, 448, 480, 512,
];

const LONG: [&[u16]; 12] = [
    LONG_96K, LONG_96K, LONG_64K, LONG_48K, LONG_48K, LONG_32K, LONG_24K, LONG_24K, LONG_16K,
    LONG_16K, LONG_16K, LONG_8K,
];
const SHORT: [&[u16]; 12] = [
    SHORT_64K, SHORT_64K, SHORT_64K, SHORT_48K, SHORT_48K, SHORT_48K, SHORT_24K, SHORT_24K,
    SHORT_16K, SHORT_16K, SHORT_16K, SHORT_8K,
];

/// Long-window `swb_offset` for `samplingFrequencyIndex` 0..=11.
pub fn long_offsets(fs_index: u8) -> Result<&'static [u16]> {
    LONG.get(fs_index as usize)
        .copied()
        .ok_or(Error::UnsupportedSampleRateIndex(fs_index))
}

/// Short-window `swb_offset` for `samplingFrequencyIndex` 0..=11.
pub fn short_offsets(fs_index: u8) -> Result<&'static [u16]> {
    SHORT
        .get(fs_index as usize)
        .copied()
        .ok_or(Error::UnsupportedSampleRateIndex(fs_index))
}

#[path = "swb_grid.rs"]
mod grid;

/// 960-line long-window `swb_offset` (`frameLengthFlag = 1`).
pub fn long_offsets_960(fs_index: u8) -> Result<&'static [u16]> {
    grid::long_960(fs_index)
}

/// 120-line short-window `swb_offset` (one of eight in a 960-line frame).
pub fn short_offsets_960(fs_index: u8) -> Result<&'static [u16]> {
    grid::short_120(fs_index)
}

/// AAC-LD 480-line long-window `swb_offset` (44.1–48, 32, and 22.05–24 kHz).
pub fn long_offsets_ld480(fs_index: u8) -> Result<&'static [u16]> {
    grid::long_480(fs_index)
}

/// AAC-LD 512-line long-window `swb_offset` for `samplingFrequencyIndex`
/// 0..=11 (96–44.1 kHz share the 48K table, 24–7.35 kHz the 24K table).
pub fn long_offsets_ld(fs_index: u8) -> Result<&'static [u16]> {
    let table = match fs_index {
        0..=4 => LD_512_48K,
        5 => LD_512_32K,
        6..=11 => LD_512_24K,
        _ => return Err(Error::UnsupportedSampleRateIndex(fs_index)),
    };
    Ok(table)
}
