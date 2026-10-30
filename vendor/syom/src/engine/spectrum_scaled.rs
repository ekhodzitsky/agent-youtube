//! No-pulse spectral decode: one Huffman read writes `invquant(q) * sf_gain`.
//! Pulse frames stay on the integer path; `apply_pulse` needs the quantisers.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::huff::SpectralBook;
use super::ics::IcsInfo;
use super::section::{SectionData, has_spectral};
use super::sf::ScaleFactors;
use super::spectrum::sf_gain;

/// Fill `spec` (cleared / resized, capacity reused) from the spectral payload.
pub(crate) fn decode_scaled_into(
    br: &mut BitReader<'_>,
    ics: &IcsInfo,
    sections: &SectionData,
    sf: &ScaleFactors,
    fs_index: u8,
    spec: &mut Vec<f32>,
) -> Result<()> {
    let win_len = ics.window_len();
    let n = ics.num_windows as usize * win_len;
    if spec.len() != n {
        spec.resize(n, 0.0);
    }
    spec.fill(0.0);
    let offsets = ics.swb_offsets(fs_index)?;
    let mut wbase = 0usize;
    for g in 0..ics.num_window_groups as usize {
        let glen = ics.window_group_length[g] as usize;
        let cbs = sections.sfb_cb.get(g).ok_or(Error::SpectrumInvalid)?;
        for sfb in 0..ics.max_sfb as usize {
            let cb = *cbs.get(sfb).ok_or(Error::SpectrumInvalid)?;
            if !has_spectral(cb) {
                continue;
            }
            let gain = sf_gain(*sf.sf.get(g).and_then(|v| v.get(sfb)).unwrap_or(&0));
            let start = *offsets.get(sfb).ok_or(Error::SpectrumInvalid)? as usize;
            let end = *offsets.get(sfb + 1).ok_or(Error::SpectrumInvalid)? as usize;
            if end < start {
                return Err(Error::SpectrumInvalid);
            }
            let width = end - start;
            if width == 0 {
                continue;
            }
            let book = SpectralBook::open(cb)?;
            if glen == 1 {
                let base = wbase * win_len + start;
                if base + width > spec.len() {
                    return Err(Error::SpectrumInvalid);
                }
                book.fill(br, gain, &mut spec[base..base + width])?;
            } else {
                // One stream for the group: a tuple may straddle two windows.
                // Sign and escape bits of the tail are consumed, then dropped.
                let bins = width * glen;
                let mut filled = 0usize;
                while filled < bins {
                    let (nt, mag) = book.pull(br)?;
                    for &sample in mag.iter().take(nt) {
                        if filled >= bins {
                            break;
                        }
                        let b = filled / width;
                        let i = filled % width;
                        let idx = (wbase + b) * win_len + start + i;
                        if idx >= spec.len() {
                            return Err(Error::SpectrumInvalid);
                        }
                        spec[idx] = sample * gain;
                        filled += 1;
                    }
                }
            }
        }
        wbase += glen;
    }
    Ok(())
}
