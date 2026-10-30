//! Bounded metadata probe: container, profile, rates, layout, duration
//! certainty. No PCM and no allocation from declared sample counts.

use crate::budgets::MemoryBudgets;
use crate::engine::adts::{ADTS_HEADER_BYTES_NO_CRC, AdtsHeader};
use crate::engine::asc::AudioSpecificConfig;
use crate::engine::bits::BitReader;
use crate::engine::channel_map::PceChannelMap;
use crate::engine::error::Error as EngineError;
use crate::engine::latm::{LOAS_SYNC, MuxCfg};
use crate::error::{AacError, Result, UnsupportedFeature};
use crate::gapless::Delay;
use crate::isomp4::{convert_units, find_moov_slice, parse_aac_meta, sniff_is_isobmff};
use crate::layout::{self, Channel, FrameMeta, Layout, mpeg_channels};
use crate::sniff::{sniff_is_adts, sniff_is_latm};

/// Advertised transport the probe locked onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeContainer {
    Adts,
    Latm,
    M4a,
}

/// Codec profile from the header or ASC. ADTS implicit HE stays [`Self::Lc`]
/// (SBR is in-band, not in the 7-byte header). [`Self::Ld`] is ER AAC LD
/// (AOT 23, 512- or 480-sample frames). Both grids probe as [`Self::Ld`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeProfile {
    Lc,
    HeAac,
    HeAacV2,
    Ld,
}

/// How sure we are about presentation length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeDuration {
    Exact { samples: u64 },
    Estimated { samples: u64 },
    Unknown,
}

/// Encoder delay and unplayed tail. An ADTS `iTunSMPB` tag reports
/// [`Self::Exact`] only when its counts match the buffered frames (1024
/// or 2048 samples each). An M4A edit list reports [`Self::Exact`]. A
/// stale tag, untagged ADTS, and LATM stay [`Self::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeTrim {
    Exact { priming: u64, remainder: u64 },
    Unknown,
}

/// Read-only AAC metadata. `Copy`; no PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Probe {
    pub container: ProbeContainer,
    pub profile: ProbeProfile,
    pub meta: FrameMeta,
    pub duration: ProbeDuration,
    pub trim: ProbeTrim,
}

/// Inspect `data` under default memory budgets. Does not decode PCM.
///
/// ```
/// let bytes = include_bytes!("goldens/sine48.adts");
/// let info = syom::probe(bytes)?;
/// assert_eq!(info.container, syom::ProbeContainer::Adts);
/// assert_eq!(info.profile, syom::ProbeProfile::Lc);
/// assert_eq!(info.meta.core_rate, 48_000);
/// assert_eq!(info.meta.output_rate, 48_000);
/// assert!(matches!(info.duration, syom::ProbeDuration::Unknown));
/// assert!(matches!(info.trim, syom::ProbeTrim::Unknown));
/// let pcm = syom::decode(bytes)?;
/// assert_eq!(pcm.sample_rate, info.meta.output_rate);
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn probe(data: &[u8]) -> Result<Probe> {
    probe_with(data, &MemoryBudgets::default())
}

/// [`probe`] with explicit metadata / AU / channel fences.
pub fn probe_with(data: &[u8], mem: &MemoryBudgets) -> Result<Probe> {
    mem.validate()
        .map_err(|e| AacError::invalid_limits(format!("{} budget is 0", e.kind)))?;
    let (data, peeled, delay) = peel_id3(data, mem)?;
    if sniff_is_adts(data) {
        return probe_adts(data, mem, delay);
    }
    if sniff_is_latm(data) {
        return probe_latm(data, mem);
    }
    if sniff_is_isobmff(data) {
        return probe_m4a(data, mem);
    }
    // A leading ID3 that is incomplete, or a tag whose payload is not AAC,
    // stays NotAac. A short untagged prefix still asks for more bytes.
    if peeled || data.len() >= 8 {
        return Err(AacError::NotAac);
    }
    Err(need_more(data.len(), 8))
}

/// One leading ID3 tag. Incomplete or non-v2.3/v2.4 `ID3` is [`AacError::NotAac`].
fn peel_id3<'a>(data: &'a [u8], mem: &MemoryBudgets) -> Result<(&'a [u8], bool, Option<Delay>)> {
    if data.len() < 3 || &data[..3] != b"ID3" {
        return Ok((data, false, None));
    }
    let budget = mem.max_input_bytes.min(mem.max_buffered_input_bytes);
    match crate::gapless::id3_at(data, budget)? {
        crate::gapless::Id3At::Ready { len, delay } if len <= data.len() => {
            Ok((&data[len..], true, delay))
        }
        _ => Err(AacError::NotAac),
    }
}

fn need_more(have: usize, need: usize) -> AacError {
    AacError::NeedMore {
        have: have as u64,
        need: need as u64,
    }
}

fn probe_adts(data: &[u8], mem: &MemoryBudgets, delay: Option<Delay>) -> Result<Probe> {
    if data.len() < ADTS_HEADER_BYTES_NO_CRC {
        return Err(need_more(data.len(), ADTS_HEADER_BYTES_NO_CRC));
    }
    if data.len() >= 2 && data[1] & 1 == 0 && data.len() < 9 {
        return Err(need_more(data.len(), 9));
    }
    let (hdr, _) = match AdtsHeader::parse(data) {
        Ok(v) => v,
        Err(EngineError::UnexpectedEnd) => {
            return Err(need_more(data.len(), data.len().saturating_add(1)));
        }
        Err(e) => return Err(AacError::from(e)),
    };
    let aot = hdr.audio_object_type();
    if aot != 2 {
        return Err(AacError::Unsupported(UnsupportedFeature::AudioObjectType(
            aot,
        )));
    }
    mem.check_declared_au(u64::from(hdr.aac_frame_length))?;
    let cfg = hdr.channel_configuration;
    let labels = mpeg_channels(cfg);
    if !labels.is_empty() {
        mem.check_channels(labels.len() as u32)?;
    }
    let rate = hdr.sample_rate();
    let layout = layout::mpeg_layout(cfg);
    let (duration, trim) = match delay.filter(|d| tag_fits(d, data)) {
        Some(d) => (
            ProbeDuration::Exact { samples: d.source },
            ProbeTrim::Exact {
                priming: d.priming,
                remainder: d.remainder,
            },
        ),
        None => (ProbeDuration::Unknown, ProbeTrim::Unknown),
    };
    Ok(Probe {
        container: ProbeContainer::Adts,
        profile: ProbeProfile::Lc,
        meta: FrameMeta::from_labels(layout, rate, rate, labels),
        duration,
        trim,
    })
}

/// The tag names a whole LC (1024) or HE (2048) frame grid, and `data`
/// is exactly those frames. A prefix or a stale tag stays unknown.
fn tag_fits(d: &Delay, data: &[u8]) -> bool {
    let Some(frames) = crate::gapless::exact_adts_frames(data) else {
        return false;
    };
    let sum = d
        .priming
        .saturating_add(d.source)
        .saturating_add(d.remainder);
    frames > 0 && (frames.saturating_mul(1024) == sum || frames.saturating_mul(2048) == sum)
}

fn probe_latm(data: &[u8], mem: &MemoryBudgets) -> Result<Probe> {
    if data.len() < 3 {
        return Err(need_more(data.len(), 3));
    }
    let v = (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]);
    if v >> 13 != LOAS_SYNC {
        return Err(AacError::NotAac);
    }
    let mux_len = (v & 0x1FFF) as usize;
    mem.check_declared_au(mux_len as u64)?;
    let need = 3 + mux_len;
    if data.len() < need {
        return Err(need_more(data.len(), need));
    }
    let mut br = BitReader::new(&data[3..need]);
    let use_same = br.read_bit().map_err(AacError::from)?;
    if use_same {
        return Err(AacError::from(EngineError::LatmNoPreviousMuxConfig));
    }
    let cfg = MuxCfg::parse(&mut br).map_err(AacError::from)?;
    from_asc(
        ProbeContainer::Latm,
        &cfg.asc,
        mem,
        ProbeDuration::Unknown,
        ProbeTrim::Unknown,
    )
}

fn probe_m4a(data: &[u8], mem: &MemoryBudgets) -> Result<Probe> {
    if crate::isomp4::frag::is_fragmented(data) {
        return probe_fmp4(data, mem);
    }
    let moov = find_moov_slice(data, mem)?;
    let track = parse_aac_meta(moov, mem)?;
    let (asc, _) = AudioSpecificConfig::parse(&track.asc).map_err(AacError::from)?;
    let (duration, trim) = m4a_timing(&track, asc.output_sample_rate)?;
    from_asc(ProbeContainer::M4a, &asc, mem, duration, trim)
}

/// Bounded fMP4 (TASK-124): the init `moov` carries ASC, rates and the
/// optional priming edit; the coded length lives in the fragments, so
/// duration stays `Unknown` unless the edit caps it.
fn probe_fmp4(data: &[u8], mem: &MemoryBudgets) -> Result<Probe> {
    let moov = find_moov_slice(data, mem)?;
    let init = crate::isomp4::frag::parse_init(moov, mem)?;
    let (asc, _) = AudioSpecificConfig::parse(&init.asc).map_err(AacError::from)?;
    let out_rate = asc.output_sample_rate;
    let (duration, trim) = if init.has_elst {
        let priming = convert_units(
            init.edit_start,
            out_rate,
            init.media_timescale,
            "elst media_time",
        )?;
        let duration = if init.edit_duration == 0 {
            ProbeDuration::Unknown // DASH init: written before the duration
        } else {
            ProbeDuration::Exact {
                samples: convert_units(
                    init.edit_duration,
                    out_rate,
                    init.movie_timescale,
                    "elst duration",
                )?,
            }
        };
        (
            duration,
            ProbeTrim::Exact {
                priming,
                remainder: 0,
            },
        )
    } else {
        (ProbeDuration::Unknown, ProbeTrim::Unknown)
    };
    from_asc(ProbeContainer::M4a, &asc, mem, duration, trim)
}

fn m4a_timing(
    track: &crate::isomp4::AacMeta,
    output_rate: u32,
) -> Result<(ProbeDuration, ProbeTrim)> {
    if track.has_elst {
        let samples = convert_units(
            track.edit_duration,
            output_rate,
            track.movie_timescale,
            "elst duration",
        )?;
        let priming = convert_units(
            track.edit_start,
            output_rate,
            track.media_timescale,
            "elst media_time",
        )?;
        let remainder = if track.media_duration != 0 && track.media_timescale != 0 {
            let coded = convert_units(
                track.media_duration,
                output_rate,
                track.media_timescale,
                "mdhd duration",
            )?;
            coded.saturating_sub(priming).saturating_sub(samples)
        } else {
            0
        };
        Ok((
            ProbeDuration::Exact { samples },
            ProbeTrim::Exact { priming, remainder },
        ))
    } else if track.media_duration != 0 && track.media_timescale != 0 {
        let samples = convert_units(
            track.media_duration,
            output_rate,
            track.media_timescale,
            "mdhd duration",
        )?;
        Ok((ProbeDuration::Exact { samples }, ProbeTrim::Unknown))
    } else {
        Ok((ProbeDuration::Unknown, ProbeTrim::Unknown))
    }
}

fn from_asc(
    container: ProbeContainer,
    asc: &AudioSpecificConfig,
    mem: &MemoryBudgets,
    duration: ProbeDuration,
    trim: ProbeTrim,
) -> Result<Probe> {
    let profile = if asc.ps_present {
        ProbeProfile::HeAacV2
    } else if asc.sbr_present {
        ProbeProfile::HeAac
    } else if asc.aot == crate::engine::asc::AOT_LD {
        ProbeProfile::Ld
    } else {
        ProbeProfile::Lc
    };
    let pce_labels;
    let (layout, labels): (Layout, &[Channel]) = if let Some(pce) = asc.pce.as_ref() {
        pce_labels = pce_channel_labels(pce);
        (Layout::Pce, pce_labels.as_slice())
    } else if asc.ps_present {
        (Layout::Mpeg(2), mpeg_channels(2))
    } else {
        let cfg = asc.channel_configuration;
        if cfg > 7 {
            return Err(AacError::Unsupported(
                UnsupportedFeature::ChannelConfiguration(cfg),
            ));
        }
        (layout::mpeg_layout(cfg), mpeg_channels(cfg))
    };
    if !labels.is_empty() {
        mem.check_channels(labels.len() as u32)?;
    }
    Ok(Probe {
        container,
        profile,
        meta: FrameMeta::from_labels(layout, asc.sample_rate, asc.output_sample_rate, labels),
        duration,
        trim,
    })
}

fn pce_channel_labels(pce: &PceChannelMap) -> Vec<Channel> {
    let mut out = Vec::new();
    push_pce_list(&mut out, &pce.front, 0);
    push_pce_list(&mut out, &pce.side, 1);
    push_pce_list(&mut out, &pce.back, 2);
    for _ in &pce.lfe {
        out.push(Channel::Lfe);
    }
    out
}

fn push_pce_list(out: &mut Vec<Channel>, list: &[(bool, u8)], where_: u8) {
    let mut sce = 0usize;
    for &(is_cpe, _) in list {
        match (where_, is_cpe, sce) {
            (0, true, _) => {
                out.push(Channel::FrontLeft);
                out.push(Channel::FrontRight);
            }
            (0, false, 0) => out.push(Channel::FrontCenter),
            (1, true, _) => {
                out.push(Channel::SideLeft);
                out.push(Channel::SideRight);
            }
            (2, true, _) => {
                out.push(Channel::BackLeft);
                out.push(Channel::BackRight);
            }
            (2, false, 0) => out.push(Channel::BackCenter),
            _ => out.push(Channel::Other),
        }
        if !is_cpe {
            sce += 1;
        }
    }
}
