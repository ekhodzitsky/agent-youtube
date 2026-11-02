//! M4A/ISOBMFF frame walk for `decode_streaming` (slice-only: the frame
//! index needs `moov`). A supported one-entry `elst` is honoured as a skip
//! of `media_time` then a cap of `segment_duration` at the output rate —
//! frames fully inside the skip region are still decoded (SBR/IMDCT overlap)
//! but not emitted; a partial frame emits `plane[skip..skip+play]`. Empty
//! edits, multiple edits, non-1.0 rates, and inexact timescale conversion
//! are errors, not a silent prefix skip.

use crate::engine::asc::AudioSpecificConfig;
use crate::engine::decode::StreamDecoder;
use crate::error::{AacError, Result};
use crate::isomp4::{self, AacTrack};
use crate::options::{ChannelMode, DecodeOptions};
use crate::stream::{Frame, StreamInfo};

pub(crate) fn map_m4a_parse_err(e: AacError) -> AacError {
    // Unsupported/malformed elst must stay Format (precise), not collapse
    // to NotAac the way a random ftyp lookalike does.
    if matches!(&e, AacError::Format(m) if m.contains("elst")) {
        e
    } else if e.is_format_class() {
        AacError::NotAac
    } else {
        e
    }
}

pub(crate) fn stream_m4a<F>(data: &[u8], opts: &DecodeOptions, on_frame: F) -> Result<StreamInfo>
where
    F: FnMut(Frame<'_>) -> Result<()>,
{
    // Bounded fMP4 (init moov + moof/trun) keeps its typed errors; the flat
    // path keeps collapsing parse failures to NotAac.
    let track = if isomp4::frag::is_fragmented(data) {
        isomp4::frag::parse_fmp4_track_with(data, &opts.memory)?
    } else {
        isomp4::parse_aac_track_with(data, &opts.memory).map_err(map_m4a_parse_err)?
    };
    play_m4a_track(
        track,
        crate::gapless::itunsmpb_in(data),
        opts,
        |off, len| sample_payload(data, off, len),
        on_frame,
    )
}

pub(crate) fn play_m4a_track<F, G>(
    track: AacTrack,
    gapless: Option<crate::gapless::Delay>,
    opts: &DecodeOptions,
    mut get_au: G,
    mut on_frame: F,
) -> Result<StreamInfo>
where
    F: FnMut(Frame<'_>) -> Result<()>,
    G: FnMut(u64, u32) -> Result<Vec<u8>>,
{
    let (asc, _) = AudioSpecificConfig::parse(&track.asc).map_err(AacError::from)?;
    let out_rate = asc.output_sample_rate;
    if out_rate == 0 || out_rate > opts.max_sample_rate {
        return Err(AacError::sample_rate(out_rate, opts.max_sample_rate));
    }
    let max_samples = opts.max_frames(out_rate);
    if track.total_samples > max_samples as u64 {
        let observed_s = track.total_samples as f64 / out_rate.max(1) as f64;
        return Err(AacError::too_long(observed_s, opts.max_duration_secs));
    }
    let mono = matches!(opts.channel_mode, ChannelMode::Mono);
    let n_ch = if mono {
        1
    } else {
        u32::from(asc.channel_configuration).max(1)
    };
    opts.memory.check_output(n_ch, track.total_samples)?;
    let mut dec = StreamDecoder::new();
    if let Some(pce) = asc.pce.clone() {
        dec.set_config_pce(pce);
    }
    dec.set_he_config(asc.sbr_present, asc.ps_present, asc.output_sample_rate);
    dec.set_frame_samples(asc.frame_samples);
    dec.mix_down_mono = mono;
    let mut scratch: Vec<f32> = Vec::new();
    let (mut skip_left, mut play_left) = track.edit_window(out_rate)?;
    // An edit list already trims. A matching `iTunSMPB` applies only when
    // `moov` has no `elst`, and only when the tag sums to the coded length.
    let mut applied = None;
    if !track.has_elst
        && let Some(d) = gapless
    {
        let spf = crate::gapless::output_frame_samples(asc.aot, asc.sbr_present, asc.ps_present);
        let coded = (track.frames.len() as u64).saturating_mul(spf);
        let sum = d
            .priming
            .saturating_add(d.source)
            .saturating_add(d.remainder);
        if spf > 0
            && coded == sum
            && let (Ok(skip), Ok(play)) = (usize::try_from(d.priming), usize::try_from(d.source))
        {
            skip_left = skip;
            play_left = Some(play);
            applied = Some(d);
        }
    }
    let skip0 = skip_left;
    let mut locked: Option<(u32, usize)> = None;
    let mut aac_frames = 0u64;
    let mut emitted = 0u64;
    let mut decoded = 0u64; // per-channel, pre-skip (cap counter)
    let mut samples_out = 0u64;

    for &(off, len) in &track.frames {
        opts.memory.check_declared_au(u64::from(len))?;
        let payload = get_au(off, len)?;
        let rate = if mono {
            scratch.clear();
            dec.decode_raw_mono_f32(
                asc.aot,
                asc.sampling_frequency_index,
                asc.sample_rate,
                asc.channel_configuration,
                1,
                &payload,
                &mut scratch,
            )
            .map_err(AacError::from)?
        } else {
            dec.decode_frame_scaled(
                asc.aot,
                asc.sampling_frequency_index,
                asc.sample_rate,
                asc.channel_configuration,
                &payload,
            )
            .map_err(AacError::from)?
        };
        aac_frames += 1;
        let (n_ch, n) = if mono {
            (1, scratch.len())
        } else {
            let planes = dec.frame_planes();
            (planes.len(), planes.first().map_or(0, Vec::len))
        };
        if n_ch == 0 {
            continue; // channel-less frame: consumed, not emitted
        }
        match locked {
            None => {
                if rate == 0 || rate > opts.max_sample_rate {
                    return Err(AacError::sample_rate(rate, opts.max_sample_rate));
                }
                locked = Some((rate, n_ch));
            }
            Some((r, c)) => {
                if rate != r {
                    return Err(AacError::decode(format!(
                        "aac: sample rate changed mid-stream ({r}Hz → {rate}Hz)"
                    )));
                }
                if c != n_ch {
                    return Err(AacError::decode(format!(
                        "aac: channel count changed mid-stream ({c} → {n_ch})"
                    )));
                }
            }
        }
        decoded += n as u64;
        if decoded > max_samples as u64 {
            let observed_s = decoded as f64 / f64::from(rate.max(1));
            return Err(AacError::too_long(observed_s, opts.max_duration_secs));
        }
        if skip_left >= n {
            skip_left -= n; // fully inside the edit-list skip region
            continue;
        }
        let start = skip_left;
        skip_left = 0;
        let mut take = n - start;
        if let Some(left) = play_left.as_mut() {
            if *left == 0 {
                break;
            }
            take = take.min(*left);
            *left -= take;
        }
        if take == 0 {
            continue;
        }
        emitted += 1;
        samples_out += take as u64;
        let end = start + take;
        if mono {
            let plane: [&[f32]; 1] = [&scratch[start..end]];
            on_frame(Frame {
                sample_rate: rate,
                samples: take,
                planar: &plane,
                meta: dec.last_meta(),
            })?;
        } else {
            let mut slots: [&[f32]; crate::layout::MAX_PLANES] = [&[]; crate::layout::MAX_PLANES];
            let src = dec.frame_planes();
            let n = src.len().min(crate::layout::MAX_PLANES);
            for (i, ch) in src.iter().enumerate().take(n) {
                slots[i] = &ch[start..end];
            }
            on_frame(Frame {
                sample_rate: rate,
                samples: take,
                planar: &slots[..n],
                meta: dec.last_meta(),
            })?;
        }
    }

    // Movie durations may round up by less than one tick (e.g. 32 samples
    // at 48 kHz with a 1 kHz movie clock). All indexed AUs were decoded.
    // Bound this metadata rounding to less than 1 ms as well; never pad PCM
    // or accept an unfinished priming skip or a whole missing AAC frame.
    let rounded_tail =
        play_left.is_some_and(|left| track.rounded_presentation_tail(left as u64, out_rate));
    if skip_left != 0 || (play_left.is_some_and(|left| left != 0) && !rounded_tail) {
        return Err(AacError::format("incomplete M4A presentation"));
    }
    if emitted == 0 {
        return Err(AacError::NotAac);
    }
    let (sample_rate, channels) = locked.unwrap_or((out_rate, 0));
    let priming = applied
        .map(|d| d.priming)
        .or_else(|| track.has_elst.then_some(skip0 as u64));
    let remainder = applied
        .map(|d| d.remainder)
        .or_else(|| track.unplayed_tail(skip0 as u64, samples_out));
    Ok(StreamInfo {
        sample_rate,
        core_rate: dec.last_core_rate(),
        channels,
        layout: dec.last_meta().layout,
        aac_frames,
        samples: samples_out,
        priming,
        remainder,
    })
}

fn sample_payload(data: &[u8], off: u64, len: u32) -> Result<Vec<u8>> {
    let end = off
        .checked_add(u64::from(len))
        .ok_or_else(|| AacError::format("aac: sample range overflow"))? as usize;
    data.get(off as usize..end)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| AacError::format("aac: sample range past end of file"))
}
