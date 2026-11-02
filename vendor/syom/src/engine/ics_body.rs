//! `individual_channel_stream()` — ISO/IEC 14496-3 Table 4.44 (LC, no gain control).

use super::bits::BitReader;
use super::error::{Error, Result};
use super::ics::IcsInfo;
use super::section::SectionData;
use super::sf::{self, ScaleFactors};
use super::spectrum::{self, PulseData};

/// One channel's decoded spectrum (window-major) plus the tools it needs.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ChannelBody {
    /// ICS of this channel (shared with the pair when `common_window`).
    pub ics: IcsInfo,
    /// Section codebooks.
    pub sections: SectionData,
    /// Absolute scalefactors / IS / PNS energies.
    pub sf: ScaleFactors,
    /// Inverse-quantised, scalefactor-applied spectrum (PNS bands still 0).
    pub spec: Vec<f32>,
    /// Optional TNS payload.
    pub tns: Option<super::tns::TnsData>,
}

/// Parse one `individual_channel_stream(common_window, scale_flag=0)`.
pub fn parse_ics(
    br: &mut BitReader<'_>,
    fs_index: u8,
    aot: u8,
    common: Option<&IcsInfo>,
) -> Result<ChannelBody> {
    parse_ics_len(br, fs_index, aot, common, 0)
}

/// [`parse_ics`] on a 960- or 480-sample grid (`frame_len` 0 keeps 1024/512).
pub(crate) fn parse_ics_len(
    br: &mut BitReader<'_>,
    fs_index: u8,
    aot: u8,
    common: Option<&IcsInfo>,
    frame_len: u16,
) -> Result<ChannelBody> {
    let mut quant = Vec::new();
    let mut spec = Vec::new();
    let mut sections = SectionData::default();
    let mut sf = ScaleFactors::default();
    let (ics, tns, _gain) = parse_ics_into(
        br,
        fs_index,
        aot,
        common,
        frame_len,
        &mut quant,
        &mut spec,
        &mut sections,
        &mut sf,
    )?;
    Ok(ChannelBody {
        ics,
        sections,
        sf,
        spec,
        tns,
    })
}

/// Parse into caller-owned buffers (capacity reused across frames).
#[allow(clippy::too_many_arguments)]
pub fn parse_ics_into(
    br: &mut BitReader<'_>,
    fs_index: u8,
    aot: u8,
    common: Option<&IcsInfo>,
    frame_len: u16,
    quant: &mut Vec<i32>,
    spec: &mut Vec<f32>,
    sections: &mut SectionData,
    sf: &mut ScaleFactors,
) -> Result<(
    IcsInfo,
    Option<super::tns::TnsData>,
    Option<super::ssr::GainControl>,
)> {
    let ld = aot == super::asc::AOT_LD;
    let global_gain = br.read(8)? as u8;
    let ics = if let Some(shared) = common {
        *shared
    } else {
        IcsInfo::parse_mode(br, fs_index, false, aot, frame_len)?
    };
    sections.parse_into(br, &ics)?;
    sf::parse_into(br, &ics, &sections.sfb_cb, global_gain, sf)?;
    let pulse_present = br.read_bit()?;
    let pulse = if pulse_present {
        if ics.window_sequence.is_eight_short() {
            return Err(Error::Format("pulse_data on eight_short"));
        }
        Some(PulseData::parse(br)?)
    } else {
        None
    };
    let tns_present = br.read_bit()?;
    // ER syntax (LD): tns_data is read after gain_control_data_present
    // (esc1_hcr / esc2_rvlc are no-ops without the resilience flags);
    // LC reads tns_data immediately after its present flag.
    let gain_present = if ld { br.read_bit()? } else { false };
    let tns = if tns_present {
        Some(super::tns::TnsData::parse(br, &ics)?)
    } else {
        None
    };
    let gain_present = if ld { gain_present } else { br.read_bit()? };
    let gain = if gain_present {
        if aot != super::asc::AOT_SSR {
            return Err(Error::Format("gain_control_data is SSR, not LC"));
        }
        Some(super::ssr::read_gain(br, ics.window_sequence)?)
    } else {
        None
    };
    // Pulse reconstruction adds to the integer quantisers. Without it the
    // Huffman read writes the scaled floats directly.
    if let Some(p) = pulse.as_ref() {
        spectrum::parse_quant_into(br, &ics, sections, fs_index, quant)?;
        spectrum::apply_pulse(quant, ics.swb_offsets(fs_index)?, p)?;
        spectrum::rescale_into(quant, &ics, sections, sf, fs_index, spec)?;
    } else {
        super::spectrum_scaled::decode_scaled_into(br, &ics, sections, sf, fs_index, spec)?;
    }
    Ok((ics, tns, gain))
}
