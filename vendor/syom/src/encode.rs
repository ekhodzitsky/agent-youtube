//! One-shot encode: planar f32 → AAC-LC (default), HE-AAC, or AAC-LD
//! under [`EncodeOptions`] (ADTS, M4A, or LOAS).
//!
//! The default encoder writes AAC-LC (no SBR/PS), mono, stereo or surround
//! (3, 4, 5, 6, 8 planes = `channel_configuration` 3–7, TASK-116), with block
//! switching (long windows for steady content, short windows on detected
//! attacks). ABR via an allowed-noise offset search plus one frame of bit
//! credit (default; `with_cbr` opts into a reservoir). LC and HE goldens under `src/goldens/` are
//! libavcodec-decoded proof; AAC-LD is qualified against FDK, not lavc.
//!
//! PCM samples must be finite and in `[-1, 1]`. NaN, infinities, and
//! `|x| > 1` are [`AacError::InvalidPcm`]. There is no silent clip. The
//! push [`crate::Encoder`] uses the same rule.

use crate::engine::adts::ADTS_SAMPLE_RATES_HZ;
use crate::engine::enc_frame::LcEncoder;
use crate::engine::enc_he::HeEncoder;
use crate::engine::enc_he_ps::{AnyHe, PsHeEncoder};
use crate::engine::swb::LONG_WINDOW_LEN as FRAME;
use crate::error::{AacError, Result};
use crate::m4a_write;
use crate::options::{EncodeContainer, EncodeOptions};

/// Build an LC encoder honoring lookahead, ATH, and tonality from `opts`.
pub(crate) fn new_lc(sample_rate: u32, channels: usize, opts: &EncodeOptions) -> Result<LcEncoder> {
    Ok(LcEncoder::new(sample_rate, channels, opts.bitrate_bps)?
        .with_quality(opts.quality)
        .with_lookahead(opts.lookahead)
        .with_psy(opts.ath, opts.tonality)
        .with_short_tns(opts.short_tns)
        .with_short_group(opts.short_group)
        .with_band_refine(opts.band_refine)
        .with_pns(opts.pns)
        .with_intensity(opts.intensity)
        .with_cbr(opts.cbr))
}

/// Encode planar f32 PCM in `[-1, 1]` to an ADTS stream: AAC-LC at 128 kbps.
///
/// Planes are anything `AsRef<[f32]>`: `&[Vec<f32>]`, `&[&[f32]]`, arrays —
/// no copy is made to satisfy the type (TASK-55). One plane per channel
/// (mono, stereo, or surround). The last block is padded and one extra MDCT
/// drains the overlap. A leading `iTunSMPB` tag names the 1024-sample
/// priming so a matching decode returns N samples.
#[inline]
pub fn encode<P: AsRef<[f32]>>(pcm: &[P], sample_rate: u32) -> Result<Vec<u8>> {
    encode_with(pcm, sample_rate, &EncodeOptions::default())
}

/// Build the HE v1 encoder: the output rate must halve to an ADTS rate and
/// the whole-stream bitrate must fit the core's 6144 bits/channel/frame.
pub(crate) fn new_he(sample_rate: u32, channels: usize, opts: &EncodeOptions) -> Result<AnyHe> {
    let core = crate::engine::enc_sbr_prep::he_core_rate(sample_rate)
        .map_err(|_| AacError::Unsupported(crate::UnsupportedFeature::EncodeHeRate(sample_rate)))?;
    if opts.ps && channels != 2 {
        return Err(AacError::encode("encode: HE v2 needs stereo input"));
    }
    let channels = if opts.ps { 1 } else { channels }; // v2: mono core
    let max_bps = crate::engine::enc_frame::max_bitrate_bps(core, channels);
    if opts.bitrate_bps > max_bps {
        return Err(AacError::encode(format!(
            "encode: HE bitrate {} bps exceeds 6144 bits/channel at the {core} Hz core (max {max_bps} bps)",
            opts.bitrate_bps
        )));
    }
    if opts.ps {
        let v2 = PsHeEncoder::new(
            sample_rate,
            opts.bitrate_bps,
            opts.lookahead,
            opts.ps_iid_fine,
        )?;
        return Ok(AnyHe::V2(Box::new(v2)));
    }
    Ok(AnyHe::V1(Box::new(HeEncoder::new(
        sample_rate,
        channels,
        opts.bitrate_bps,
        opts.lookahead,
    )?)))
}

/// Explicit HE `AudioSpecificConfig`: AOT 5 two-rate, or AOT 29 for v2.
pub(crate) fn he_asc(enc: &AnyHe, sample_rate: u32, planes: usize) -> Result<Vec<u8>> {
    let out = fs_index(sample_rate)?;
    Ok(match enc {
        AnyHe::V1(_) => crate::engine::asc::write_he(enc.fs_index(), out, planes as u8),
        AnyHe::V2(_) => crate::engine::asc::write_he_ps(enc.fs_index(), out),
    })
}

/// HE v1 one-shot: every access unit, then ADTS (implicit SBR, core rate
/// in the header) or M4A (explicit two-rate ASC, output-rate timeline).
/// The tuple is `(bytes, ps_data bits, extended-data block bits)`.
pub(crate) fn encode_he(
    planes: &[&[f32]],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<(Vec<u8>, u64, u64)> {
    validate(planes, sample_rate, opts)?;
    let channels = planes.len();
    let mut enc = new_he(sample_rate, channels, opts)?;
    let mut aus: Vec<Vec<u8>> = Vec::new();
    enc.push(planes, |au| {
        aus.push(au.to_vec());
        Ok(())
    })?;
    let info = enc.finish(|au| {
        aus.push(au.to_vec());
        Ok(())
    })?;
    let (ps_bits, ext_bits) = enc.ps_side_info();
    let asc = he_asc(&enc, sample_rate, channels)?;
    let bytes = match opts.container {
        EncodeContainer::Adts => Ok(crate::gapless::prefix_delay(
            wrap_adts(&aus, enc.fs_index(), enc.core_channels(channels)),
            info.priming_out,
            info.remainder_out,
            info.source,
        )),
        EncodeContainer::Latm => mux::wrap_latm(&aus, &asc),
        EncodeContainer::M4a => m4a_write::mux_aac(
            &aus,
            &asc,
            channels,
            sample_rate,
            info.source,
            info.priming_out,
            2048,
        ),
        EncodeContainer::Raw => Err(AacError::Unsupported(
            crate::UnsupportedFeature::EncodeRawOneShot,
        )),
    }?;
    Ok((bytes, ps_bits, ext_bits))
}

/// Encode planar f32 PCM under `opts` (container, bitrate, lookahead,
/// quality VBR, HE v1/v2, AAC-LD, or the opt-in LC tools).
///
/// With [`EncodeOptions::lookahead`] on, the attack detector runs one
/// frame ahead (better pre-echo suppression on early-in-frame onsets) at
/// one extra frame of internal latency; drain still emits one silent
/// successor so the last source samples reconstruct.
pub fn encode_with<P: AsRef<[f32]>>(
    pcm: &[P],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<Vec<u8>> {
    let pcm: Vec<&[f32]> = pcm.iter().map(AsRef::as_ref).collect();
    let pcm = pcm.as_slice();
    validate(pcm, sample_rate, opts)?;
    if opts.ld {
        return ld_api::encode_ld(pcm, sample_rate, opts);
    }
    if opts.he {
        return Ok(encode_he(pcm, sample_rate, opts)?.0);
    }
    if pcm.len() > 2 {
        return mux::encode_mc(pcm, sample_rate, opts);
    }
    let channels = pcm.len();
    let mut enc = new_lc(sample_rate, channels, opts)?;
    let n_samples = pcm[0].len();
    let n_frames = n_samples.div_ceil(FRAME);
    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(n_frames);
    let mut bufs = [[0.0f32; FRAME]; 2];
    for f in 0..n_frames {
        let start = f * FRAME;
        let end = (start + FRAME).min(n_samples);
        for (ch, plane) in pcm.iter().enumerate() {
            bufs[ch] = [0.0; FRAME];
            bufs[ch][..end - start].copy_from_slice(&plane[start..end]);
        }
        let planes: Vec<&[f32]> = bufs[..channels].iter().map(|b| &b[..]).collect();
        if opts.lookahead {
            if let Some(payload) = enc.push_frame(&planes)? {
                payloads.push(payload);
            }
        } else {
            payloads.push(enc.encode_frame(&planes)?);
        }
    }
    payloads.extend(enc.drain_overlap()?);
    match opts.container {
        EncodeContainer::Adts => {
            let fs = enc.fs_index();
            let wrapped = match enc.take_cbr_log() {
                Some(log) => cbr_adts::wrap_with_fullness(&payloads, fs, channels, &log),
                None => wrap_adts(&payloads, fs, channels),
            };
            Ok(crate::gapless::prefix_lc(wrapped, n_samples as u64))
        }
        EncodeContainer::Latm => {
            let asc = crate::engine::asc::write_lc(enc.fs_index(), channels as u8);
            mux::wrap_latm(&payloads, &asc)
        }
        EncodeContainer::M4a => m4a_write::mux_aac_lc(
            &payloads,
            enc.fs_index(),
            channels,
            sample_rate,
            n_samples as u64,
            FRAME as u64,
        ),
        EncodeContainer::Raw => Err(AacError::Unsupported(
            crate::UnsupportedFeature::EncodeRawOneShot,
        )),
    }
}

#[path = "encode_ld_api.rs"]
mod ld_api;
#[path = "encode_mux.rs"]
mod mux;
pub use mux::{
    encode_write, encode_write_m4a, mux_raw_he_m4a, mux_raw_lc_m4a, wrap_adts_au, wrap_loas_au,
};

fn fs_index(sample_rate: u32) -> Result<u8> {
    ADTS_SAMPLE_RATES_HZ
        .iter()
        .position(|&r| r == sample_rate)
        .map(|i| i as u8)
        .ok_or_else(|| {
            AacError::encode(format!(
                "encode: unsupported sample rate {sample_rate}Hz (not in the AAC table)"
            ))
        })
}

/// Encode to ADTS and write the file.
pub fn write<P: AsRef<[f32]>>(
    path: impl AsRef<std::path::Path>,
    pcm: &[P],
    sample_rate: u32,
) -> Result<()> {
    write_with(path, pcm, sample_rate, &EncodeOptions::default())
}

/// Encode under `opts` and write the file.
pub fn write_with<P: AsRef<[f32]>>(
    path: impl AsRef<std::path::Path>,
    pcm: &[P],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<()> {
    let bytes = encode_with(pcm, sample_rate, opts)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Mode combinations shared by one-shot and push encode: a quality level
/// must be `0..=10` and is LC only.
pub(crate) fn check_mode(opts: &EncodeOptions) -> Result<()> {
    if opts.cbr && (opts.he || opts.ps || opts.ld || opts.quality.is_some()) {
        return Err(AacError::encode(
            "encode: CBR reservoir is AAC-LC only (not HE, AAC-LD, or quality VBR)",
        ));
    }
    if let Some(q) = opts.quality {
        if q > crate::engine::enc_frame::QUALITY_MAX {
            return Err(AacError::encode(format!(
                "encode: quality level {q} is above 10"
            )));
        }
        if opts.he {
            return Err(AacError::encode(
                "encode: quality VBR is LC only (he must be off)",
            ));
        }
    }
    if opts.ps && !opts.he {
        return Err(AacError::encode("encode: ps needs he (use with_he_v2)"));
    }
    if opts.ld {
        if opts.container == crate::EncodeContainer::Adts {
            return Err(AacError::encode("encode: AAC-LD cannot be carried in ADTS"));
        }
        if opts.he || opts.ps || opts.lookahead || opts.quality.is_some() {
            return Err(AacError::encode(
                "encode: AAC-LD is not HE, lookahead, or quality VBR",
            ));
        }
        if opts.ath
            || opts.tonality
            || opts.short_tns
            || opts.short_group
            || opts.band_refine
            || opts.pns
            || opts.intensity
        {
            return Err(AacError::encode(
                "encode: AAC-LD does not use those LC tools",
            ));
        }
    }
    Ok(())
}

/// Highest LD bitrate that still fits 6144 bits/channel on a 512-sample frame.
pub(crate) fn max_ld_bitrate(sample_rate: u32, channels: usize) -> u32 {
    let bits = 6144u64 * channels as u64 * u64::from(sample_rate) / 512;
    bits.min(u64::from(u32::MAX)) as u32
}

fn validate(pcm: &[&[f32]], sample_rate: u32, opts: &EncodeOptions) -> Result<()> {
    check_mode(opts)?;
    if !ADTS_SAMPLE_RATES_HZ.contains(&sample_rate) {
        return Err(AacError::encode(format!(
            "encode: unsupported sample rate {sample_rate}Hz (not in the AAC table)"
        )));
    }
    check_planes(pcm.len(), opts)?;
    if opts.bitrate_bps == 0 {
        return Err(AacError::encode("encode: bitrate must be > 0"));
    }
    let max_bps = if opts.ld {
        max_ld_bitrate(sample_rate, pcm.len())
    } else {
        crate::engine::enc_frame::max_bitrate_bps(sample_rate, pcm.len())
    };
    if opts.bitrate_bps > max_bps {
        let profile = if opts.ld { "AAC-LD" } else { "AAC-LC" };
        return Err(AacError::encode(format!(
            "encode: bitrate {} bps exceeds {profile} 6144 bits/channel (max {max_bps} bps)",
            opts.bitrate_bps
        )));
    }
    let n = pcm[0].len();
    if n == 0 {
        return Err(AacError::InvalidPcm(crate::PcmReject::Empty));
    }
    if pcm.iter().any(|p| p.len() != n) {
        return Err(AacError::InvalidPcm(crate::PcmReject::PlaneLength));
    }
    check_pcm_samples(pcm.iter().copied())?;
    Ok(())
}

/// Plane counts: 1–2 (LC / HE), or surround 3, 4, 5, 6, 8 — AAC-LC only,
/// no lookahead (`channel_configuration` 3–7, TASK-116).
pub(crate) fn check_planes(planes: usize, opts: &EncodeOptions) -> Result<()> {
    if opts.cbr && !matches!(planes, 1 | 2) {
        return Err(AacError::encode("encode: CBR reservoir is mono or stereo"));
    }
    if opts.ld && !matches!(planes, 1 | 2) {
        return Err(AacError::encode("encode: AAC-LD is mono or stereo"));
    }
    if (1..=2).contains(&planes) {
        return Ok(());
    }
    if crate::engine::enc_mc::config_for(planes).is_none() {
        return Err(AacError::encode(format!(
            "encode: channels must be 1, 2, 3, 4, 5, 6 or 8, got {planes}"
        )));
    }
    if opts.he || opts.lookahead {
        return Err(AacError::encode(
            "encode: surround is AAC-LC without lookahead (he and lookahead must be off)",
        ));
    }
    Ok(())
}

/// Finite samples in `[-1, 1]`. Shared by one-shot encode and push encode.
pub(crate) fn check_pcm_samples<'a, I>(planes: I) -> Result<()>
where
    I: IntoIterator<Item = &'a [f32]>,
{
    for plane in planes {
        for &x in plane {
            if !x.is_finite() {
                return Err(AacError::InvalidPcm(crate::PcmReject::NonFinite));
            }
            if x.abs() > 1.0 {
                return Err(AacError::InvalidPcm(crate::PcmReject::Amplitude));
            }
        }
    }
    Ok(())
}

#[path = "encode_cbr.rs"]
mod cbr_adts;
use cbr_adts::wrap_adts;
pub(crate) use cbr_adts::{adts_frame_fullness, adts_frame_into};
