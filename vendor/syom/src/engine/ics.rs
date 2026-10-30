//! `ics_info()` — ISO/IEC 14496-3 §4.4.6 Table 4.6.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::swb::{long_offsets, short_offsets};

/// `window_sequence` — Table 4.128.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowSequence {
    /// One 2048-point transform.
    OnlyLong = 0,
    /// Long transform, start window on the right half.
    LongStart = 1,
    /// Eight 256-point transforms.
    EightShort = 2,
    /// Long transform, stop window on the left half.
    LongStop = 3,
}

impl WindowSequence {
    fn from_bits(bits: u8) -> Self {
        match bits & 3 {
            0 => Self::OnlyLong,
            1 => Self::LongStart,
            2 => Self::EightShort,
            _ => Self::LongStop,
        }
    }

    /// `true` ⇔ eight short windows.
    #[must_use]
    pub fn is_eight_short(self) -> bool {
        matches!(self, Self::EightShort)
    }
}

/// `window_shape` — 0 sine, 1 KBD (§4.5.2.3.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowShape {
    /// Sine window.
    Sine = 0,
    /// Kaiser-Bessel-derived window.
    Kbd = 1,
}

impl WindowShape {
    fn from_bit(bit: bool) -> Self {
        if bit { Self::Kbd } else { Self::Sine }
    }
}

/// Parsed `ics_info()` plus derived grouping.
#[derive(Debug, Clone, Copy)]
pub struct IcsInfo {
    /// Table 4.128 window sequence.
    pub window_sequence: WindowSequence,
    /// This block's right-half shape (left half comes from the previous block).
    /// On the LD path `Kbd` means the low-overlap window (FDK maps the LD
    /// shape bit the same way); LD never uses a Kaiser window.
    pub window_shape: WindowShape,
    /// Number of coded scalefactor bands.
    pub max_sfb: u8,
    /// 1 for long, 8 for short.
    pub num_windows: u8,
    /// Groups implied by `scale_factor_grouping`.
    pub num_window_groups: u8,
    /// Windows per group (sums to `num_windows`).
    pub window_group_length: [u8; 8],
    /// Number of SWB in the table for this rate / sequence.
    pub num_swb: u8,
    /// ER AAC LD (AOT 23): long windows only. 512 or 480 lines.
    pub ld: bool,
    /// Core samples in this frame: 1024, 960, 512, or 480.
    /// Zero means the profile default (1024, or 512 when `ld`).
    pub frame_len: u16,
    /// `predictor_data_present` (Main or LTP). Eight-short frames stay clear.
    pub predictor_present: bool,
    /// Main reset group, 0 when the frame did not ask for a reset.
    pub pred_reset_group: u8,
    /// Bit `sfb` set means Main prediction is added on that band.
    pub pred_used: u64,
    /// LTP payload follows `predictor_data_present` and its present flag.
    pub ltp_present: bool,
    /// LTP lag in samples (11 bits).
    pub ltp_lag: u16,
    /// Index into the 8-entry LTP coefficient table.
    pub ltp_coef_idx: u8,
    /// Bit `sfb` set means add the LTP estimate on that long-window band.
    pub ltp_used: u64,
}

impl IcsInfo {
    /// Parse Table 4.6. LC rejects predictor / LTP bodies.
    pub fn parse(br: &mut BitReader<'_>, fs_index: u8, common_window: bool) -> Result<Self> {
        Self::parse_mode(br, fs_index, common_window, super::asc::AOT_LC, 0)
    }

    /// `aot` selects LD windows and the Main / LTP predictor body.
    /// `frame_len` is 960, 480, 512, or 1024; zero selects 512 for LD and
    /// 1024 otherwise.
    pub fn parse_mode(
        br: &mut BitReader<'_>,
        fs_index: u8,
        common_window: bool,
        aot: u8,
        frame_len: u16,
    ) -> Result<Self> {
        let ld = aot == super::asc::AOT_LD;
        let _reserved = br.read_bit()?;
        let window_sequence = WindowSequence::from_bits(br.read(2)? as u8);
        let window_shape = WindowShape::from_bit(br.read_bit()?);
        if ld && !matches!(window_sequence, WindowSequence::OnlyLong) {
            // ER AAC LD carries long windows only (FDK: AAC_DEC_PARSE_ERROR).
            return Err(Error::IcsInfoInvalid);
        }
        let mut predictor_present = false;
        let mut pred_reset_group = 0u8;
        let mut pred_used = 0u64;
        let mut ltp_present = false;
        let mut ltp_lag = 0u16;
        let mut ltp_coef_idx = 0u8;
        let mut ltp_used = 0u64;
        let (max_sfb, grouping) = if window_sequence.is_eight_short() {
            (br.read(4)? as u8, Some(br.read(7)? as u8))
        } else {
            let max_sfb = br.read(6)? as u8;
            if br.read_bit()? {
                if aot == super::asc::AOT_MAIN {
                    let (group, used) = super::pred::read_main(br, max_sfb, fs_index)?;
                    predictor_present = true;
                    pred_reset_group = group;
                    pred_used = used;
                } else if aot == super::asc::AOT_LTP {
                    let (present, lag, coef, used) = super::ltp::read_present(br, max_sfb)?;
                    predictor_present = true;
                    ltp_present = present;
                    ltp_lag = lag;
                    ltp_coef_idx = coef;
                    ltp_used = used;
                } else {
                    // AAC-LC shall not carry predictor / LTP (§1.5.1.1).
                    return Err(Error::Format("predictor/LTP on LC stream"));
                }
            }
            let _ = common_window;
            (max_sfb, None)
        };

        let (num_windows, num_window_groups, window_group_length) =
            grouping_of(window_sequence, grouping);
        let frame_len = resolve_frame_len(ld, frame_len);
        let offsets = if window_sequence.is_eight_short() {
            if frame_len == 960 {
                super::swb::short_offsets_960(fs_index)?
            } else {
                short_offsets(fs_index)?
            }
        } else if ld {
            if frame_len == 480 {
                super::swb::long_offsets_ld480(fs_index)?
            } else {
                super::swb::long_offsets_ld(fs_index)?
            }
        } else if frame_len == 960 {
            super::swb::long_offsets_960(fs_index)?
        } else {
            long_offsets(fs_index)?
        };
        let num_swb = offsets.len().saturating_sub(1) as u8;
        if max_sfb as usize > num_swb as usize {
            return Err(Error::IcsInfoInvalid);
        }
        Ok(IcsInfo {
            window_sequence,
            window_shape,
            max_sfb,
            num_windows,
            num_window_groups,
            window_group_length,
            num_swb,
            ld,
            frame_len,
            predictor_present,
            pred_reset_group,
            pred_used,
            ltp_present,
            ltp_lag,
            ltp_coef_idx,
            ltp_used,
        })
    }

    /// Samples in one long window, or one of the eight short windows.
    #[must_use]
    pub fn window_len(&self) -> usize {
        let core = self.core_len();
        if self.window_sequence.is_eight_short() {
            core / 8
        } else {
            core
        }
    }

    /// PCM samples this frame contributes (1024, 960, 512, or 480).
    #[must_use]
    pub fn core_len(&self) -> usize {
        match self.frame_len {
            960 | 480 | 512 | 1024 => usize::from(self.frame_len),
            _ => {
                if self.ld {
                    super::swb::LD_WINDOW_LEN
                } else {
                    super::swb::LONG_WINDOW_LEN
                }
            }
        }
    }

    /// `swb_offset` table for this frame's sequence / profile.
    pub fn swb_offsets(&self, fs_index: u8) -> Result<&'static [u16]> {
        let frame_len = self.core_len();
        if self.window_sequence.is_eight_short() {
            if frame_len == 960 {
                super::swb::short_offsets_960(fs_index)
            } else {
                short_offsets(fs_index)
            }
        } else if self.ld {
            if frame_len == 480 {
                super::swb::long_offsets_ld480(fs_index)
            } else {
                super::swb::long_offsets_ld(fs_index)
            }
        } else if frame_len == 960 {
            super::swb::long_offsets_960(fs_index)
        } else {
            long_offsets(fs_index)
        }
    }
}

fn resolve_frame_len(ld: bool, frame_len: u16) -> u16 {
    if ld {
        if frame_len == 480 { 480 } else { 512 }
    } else if frame_len == 960 {
        960
    } else {
        1024
    }
}

pub(crate) fn grouping_of(seq: WindowSequence, grouping: Option<u8>) -> (u8, u8, [u8; 8]) {
    if !seq.is_eight_short() {
        let mut lens = [0u8; 8];
        lens[0] = 1;
        return (1, 1, lens);
    }
    let gbits = grouping.unwrap_or(0);
    let mut lens = [0u8; 8];
    lens[0] = 1;
    let mut groups = 1u8;
    for i in 1..8 {
        // bit 6 of the 7-bit field groups window 1 with window 0, …
        if (gbits >> (6 - (i - 1))) & 1 == 1 {
            lens[(groups - 1) as usize] += 1;
        } else {
            lens[groups as usize] = 1;
            groups += 1;
        }
    }
    (8, groups, lens)
}
