//! HE v1 SBR header policy (TASK-87/89): one fixed header per SBR rate
//! and core bitrate — 3.0 dB, Table 4.63 defaults, no extras — whose
//! crossover `k0` follows the kbps per channel the core gets.

use super::error::{Error, Result};
use super::sbr_freq_bands::k0;
use super::sbr_header::SbrHeader;

/// v1 `bs_stop_freq` per SBR rate (k2 ≈ 7.5–15.4 kHz; pinned in tests).
const STOP_FREQ: [(u32, u8); 6] = [
    (16_000, 11),
    (22_050, 9),
    (24_000, 9),
    (32_000, 9),
    (44_100, 8),
    (48_000, 8),
];
/// Crossover target per core kbps per channel (24 → 6.75 kHz, 32+ →
/// 9 kHz at 48 kHz), clamped to `[4.5 kHz, 0.2·fs_sbr]`: k0 under ≈ 12
/// at 44.1/48 kHz has no valid patch set, and a core starved by the
/// rate loop cannot feed patches.
const XOVER_HZ_PER_KBPS: f32 = 281.25;
const XOVER_MIN_HZ: f32 = 4500.0;

/// v1 header (3.0 dB, Table 4.63 defaults, no extras); the largest
/// `bs_start_freq` whose k0 stays under the crossover target.
pub(crate) fn he_header(fs_sbr: u32, kbps_per_channel: u32) -> Result<SbrHeader> {
    let (_, stop_freq) = STOP_FREQ
        .iter()
        .copied()
        .find(|&(fs, _)| fs == fs_sbr)
        .ok_or(Error::UnsupportedSampleRateIndex(0xFF))?;
    let hi = fs_sbr as f32 * 0.2;
    let target = (kbps_per_channel as f32 * XOVER_HZ_PER_KBPS).clamp(XOVER_MIN_HZ.min(hi), hi);
    let band_hz = fs_sbr as f32 / 128.0;
    let start_freq = (0..16u8)
        .filter(|&s| k0(fs_sbr, s).is_ok_and(|k| k as f32 * band_hz <= target))
        .max()
        .unwrap_or(0);
    Ok(SbrHeader {
        amp_res: true,
        start_freq,
        stop_freq,
        xover_band: 0,
        reserved: 0,
        header_extra_1: false,
        header_extra_2: false,
        freq_scale: 2,
        alter_scale: true,
        noise_bands: 2,
        limiter_bands: 2,
        limiter_gains: 2,
        interpol_freq: true,
        smoothing_mode: true,
    })
}
