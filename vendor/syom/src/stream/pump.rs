//! Frame pump loops: ADTS and LATM/LOAS iteration over a byte buffer,
//! per-frame caps/consistency ([`Decoder::tally`]), and delivery — either
//! the borrowed-plane callback ([`Decoder::emit`]) or, for one-shot mono,
//! a sink `Vec` the engine appends to directly (no scratch, no copy).

use crate::budgets::{BudgetExceeded, BudgetKind};
use crate::engine::adts::AdtsHeader;
use crate::engine::bits::BitReader;
use crate::engine::error::Error as EngineError;
use crate::engine::latm::{LOAS_SYNC, MuxCfg, read_payload};
use crate::error::{AacError, Result};
use crate::isomp4::sniff_is_isobmff;
use crate::options::ChannelMode;
use crate::sniff::{sniff_is_adts, sniff_is_latm};

use super::{Container, Decoder, Frame};

impl Decoder {
    /// Pump complete frames out of `buf` (starting at `self.pos`). When
    /// `sink` is `Some` (one-shot mono), frames decode straight into it and
    /// `on_frame` never fires; `est_frames` then pre-sizes the sink after
    /// the first frame.
    pub(super) fn pump<F>(
        &mut self,
        buf: &[u8],
        sink: Option<&mut Vec<f32>>,
        est_frames: Option<usize>,
        on_frame: &mut F,
        final_flush: bool,
    ) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        if matches!(self.container, Container::Unknown) {
            let avail = &buf[self.pos..];
            if avail.len() < 8 && !final_flush {
                return Ok(()); // wait for enough bytes to sniff
            }
            // Sniff order and ADTS fallback mirror `decode_with`. ISOBMFF
            // enters the fMP4 pump: a fragmented init (`mvex`) decodes
            // incrementally, a flat moov keeps its M4aPush rejection there.
            self.container = if sniff_is_isobmff(avail) {
                Container::Fmp4
            } else if sniff_is_adts(avail) || !sniff_is_latm(avail) {
                Container::Adts
            } else {
                Container::Latm
            };
        }
        match self.container {
            Container::Adts => self.pump_adts(buf, sink, est_frames, on_frame, final_flush),
            Container::Latm => self.pump_latm(buf, sink, est_frames, on_frame),
            Container::Fmp4 => self.pump_fmp4(buf, on_frame, final_flush),
            Container::Au => {
                if final_flush {
                    Ok(())
                } else {
                    Err(AacError::Unsupported(
                        crate::UnsupportedFeature::RawAccessUnit,
                    ))
                }
            }
            Container::Unknown => Ok(()),
        }
    }

    fn pump_adts<F>(
        &mut self,
        buf: &[u8],
        mut sink: Option<&mut Vec<f32>>,
        est_frames: Option<usize>,
        on_frame: &mut F,
        final_flush: bool,
    ) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        let tag_budget = self
            .opts
            .memory
            .max_buffered_input_bytes
            .min(self.opts.memory.max_input_bytes);
        if self.gap.hold(buf, &mut self.pos, final_flush, tag_budget)? {
            return Ok(());
        }
        loop {
            let avail = &buf[self.pos..];
            let (hdr, payload_off) = match AdtsHeader::parse(avail) {
                Ok(v) => v,
                // Partial header: wait for more bytes (finish drops it).
                Err(EngineError::UnexpectedEnd) => return Ok(()),
                // No sync: resync one byte (garbage prefix / inter-frame junk).
                Err(EngineError::AdtsSyncNotFound) => {
                    self.pos += 1;
                    continue;
                }
                // Definite header syntax: do not skip into NotAac (TASK-51).
                Err(e) => return Err(AacError::from(e)),
            };
            let frame_len = usize::from(hdr.aac_frame_length);
            self.opts
                .memory
                .check_declared_au(u64::from(hdr.aac_frame_length))?;
            if avail.len() < frame_len {
                if final_flush && self.emitted == 0 {
                    return Err(AacError::truncated_at(Some(self.pos as u64)));
                }
                return Ok(()); // wait; finish drops a trailing partial frame
            }
            if !hdr.protection_absent {
                crate::engine::adts_crc::verify_adts_crc(&avail[..frame_len], &hdr)
                    .map_err(AacError::from)?;
            }
            let payload = &avail[payload_off..frame_len];
            let rate = match sink.as_deref_mut() {
                Some(dst) => self.adts_into(dst, &hdr, payload, est_frames)?,
                None => self.decode_adts(&hdr, payload, self.aac_frames)?,
            };
            self.pos += frame_len;
            if sink.is_none() {
                self.emit(rate, on_frame)?;
            } else {
                self.aac_frames += 1;
            }
        }
    }

    /// Decode one ADTS payload: mono fast path into `mono_scratch`, or
    /// scaled planes borrowed via `frame_planes`.
    fn decode_adts(&mut self, hdr: &AdtsHeader, payload: &[u8], _idx: u64) -> Result<u32> {
        if matches!(self.opts.channel_mode, ChannelMode::Mono) {
            self.mono_scratch.clear();
            self.dec
                .decode_adts_header_payload(hdr, payload, Some(&mut self.mono_scratch))
                .map_err(AacError::from)
        } else {
            self.dec
                .decode_adts_header_payload(hdr, payload, None)
                .map_err(AacError::from)
        }
    }

    fn pump_latm<F>(
        &mut self,
        buf: &[u8],
        mut sink: Option<&mut Vec<f32>>,
        est_frames: Option<usize>,
        on_frame: &mut F,
    ) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        loop {
            let avail = &buf[self.pos..];
            if avail.len() < 3 {
                return Ok(()); // retain the trailing bytes across feeds
            }
            let v = (u32::from(avail[0]) << 16) | (u32::from(avail[1]) << 8) | u32::from(avail[2]);
            if v >> 13 != LOAS_SYNC {
                self.pos += 1; // byte-scan resync, like the one-shot loop
                continue;
            }
            let mux_len = (v & 0x1FFF) as usize;
            self.opts.memory.check_declared_au(mux_len as u64)?;
            if avail.len() < 3 + mux_len {
                return Ok(()); // partial LOAS frame: wait (finish drops it)
            }
            let body = &avail[3..3 + mux_len];
            let mut br = BitReader::new(body);
            let use_same = br.read_bit().map_err(AacError::from)?;
            if !use_same {
                let cfg = MuxCfg::parse(&mut br).map_err(AacError::from)?;
                if let Some(pce) = cfg.asc.pce.clone() {
                    self.dec.set_config_pce(pce);
                }
                self.dec.set_he_config(
                    cfg.asc.sbr_present,
                    cfg.asc.ps_present,
                    cfg.asc.output_sample_rate,
                );
                self.dec.set_frame_samples(cfg.asc.frame_samples);
                self.mux = Some(cfg);
            }
            let cfg = self
                .mux
                .as_ref()
                .ok_or(EngineError::LatmNoPreviousMuxConfig)
                .map_err(AacError::from)?;
            let (aot, fs, sr, ch, n_au) = (
                cfg.asc.aot,
                cfg.asc.sampling_frequency_index,
                cfg.asc.sample_rate,
                cfg.asc.channel_configuration,
                cfg.num_sub_frames.saturating_add(1),
            );
            for _ in 0..n_au {
                let payload = {
                    let cfg = self
                        .mux
                        .as_ref()
                        .ok_or(EngineError::LatmNoPreviousMuxConfig)
                        .map_err(AacError::from)?;
                    read_payload(&mut br, cfg).map_err(AacError::from)?
                };
                let idx = self.aac_frames;
                let rate = match sink.as_deref_mut() {
                    Some(dst) => {
                        self.check_sink_growth(dst.len())?;
                        let before = dst.len();
                        let rate = self
                            .dec
                            .decode_raw_mono_f32(aot, fs, sr, ch, 1, &payload, dst)
                            .map_err(AacError::from)?;
                        let n = dst.len() - before;
                        self.tally(rate, 1, n)?;
                        self.sink_frame_samples = n;
                        self.opts.memory.check_output(1, dst.len() as u64)?;
                        if idx == 0 {
                            self.reserve_hint(dst, est_frames, n)?;
                        }
                        rate
                    }
                    None => {
                        if matches!(self.opts.channel_mode, ChannelMode::Mono) {
                            self.mono_scratch.clear();
                            self.dec
                                .decode_raw_mono_f32(
                                    aot,
                                    fs,
                                    sr,
                                    ch,
                                    1,
                                    &payload,
                                    &mut self.mono_scratch,
                                )
                                .map_err(AacError::from)?
                        } else {
                            self.dec
                                .decode_frame_scaled(aot, fs, sr, ch, &payload)
                                .map_err(AacError::from)?
                        }
                    }
                };
                if sink.is_none() {
                    self.emit(rate, on_frame)?;
                } else {
                    self.aac_frames += 1;
                }
            }
            self.pos += 3 + mux_len;
        }
    }

    /// Caps + consistency for one decoded frame (delivery is the caller's).
    pub(super) fn tally(&mut self, rate: u32, n_ch: usize, n_samples: usize) -> Result<()> {
        self.tally_decoded(rate, n_ch, n_samples)?;
        self.samples_out += n_samples as u64;
        self.emitted += 1;
        Ok(())
    }

    /// Channel/workspace fences, the rate/channel lock and the
    /// decoded-duration cap — everything but the delivery counters, which
    /// the fMP4 windowing path (stream/fmp4.rs) accounts itself.
    pub(super) fn tally_decoded(&mut self, rate: u32, n_ch: usize, n_samples: usize) -> Result<()> {
        if n_ch == 0 {
            return Ok(()); // channel-less frame: consumed, not emitted
        }
        let n_ch_u32 = u32::try_from(n_ch).unwrap_or(u32::MAX);
        self.opts.memory.check_channels(n_ch_u32)?;
        self.opts.memory.check_workspace(n_ch_u32, false, false)?;
        match self.locked {
            None => {
                if rate == 0 || rate > self.opts.max_sample_rate {
                    return Err(AacError::sample_rate(rate, self.opts.max_sample_rate));
                }
                self.locked = Some((rate, n_ch));
                self.max_samples = self.opts.max_frames(rate);
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
        self.samples_decoded += n_samples as u64;
        if self.samples_decoded > self.max_samples as u64 {
            let observed_s = self.samples_decoded as f64 / f64::from(rate.max(1));
            return Err(AacError::too_long(observed_s, self.opts.max_duration_secs));
        }
        Ok(())
    }

    /// Pre-size the mono sink after frame 1: exact frame count × first
    /// frame's samples beats geometric growth (hint only, capped).
    pub(super) fn reserve_hint(
        &self,
        dst: &mut Vec<f32>,
        est_frames: Option<usize>,
        first: usize,
    ) -> Result<()> {
        if let Some(frames) = est_frames {
            let want = frames.saturating_mul(first).min(self.max_samples);
            if self.opts.memory.check_output(1, want as u64).is_ok() {
                dst.try_reserve(want.saturating_sub(dst.len()))
                    .map_err(|_| {
                        AacError::from(BudgetExceeded {
                            kind: BudgetKind::Output,
                            observed: (want as u64).saturating_mul(4),
                            max: self.opts.memory.effective_output_bytes(),
                        })
                    })?;
            }
        }
        Ok(())
    }

    pub(super) fn check_sink_growth(&self, dst_len: usize) -> Result<()> {
        let guess = if self.sink_frame_samples == 0 {
            2048
        } else {
            self.sink_frame_samples as u64
        };
        self.opts
            .memory
            .check_output(1, dst_len as u64 + guess)
            .map(|_| ())
            .map_err(AacError::from)
    }

    /// Caps + consistency for one decoded frame, then the callback.
    pub(super) fn emit<F>(&mut self, rate: u32, on_frame: &mut F) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        let mono = matches!(self.opts.channel_mode, ChannelMode::Mono);
        let (n_ch, n_samples) = if mono {
            (1, self.mono_scratch.len())
        } else {
            let planes = self.dec.frame_planes();
            (planes.len(), planes.first().map_or(0, Vec::len))
        };
        if n_ch == 0 {
            return Ok(()); // channel-less frame: consumed, not emitted
        }
        self.tally_decoded(rate, n_ch, n_samples)?;
        if self.aac_frames == 0 {
            self.gap.arm(n_samples);
        }
        let (start, end) = self.gap.take(n_samples);
        self.aac_frames += 1;
        if end == start {
            return Ok(());
        }
        let shown = end - start;
        self.samples_out += shown as u64;
        self.emitted += 1;
        // Stack plane views — no per-frame heap (TASK-78).
        let mut slots: [&[f32]; crate::layout::MAX_PLANES] = [&[]; crate::layout::MAX_PLANES];
        let planar: &[&[f32]] = if mono {
            slots[0] = &self.mono_scratch[start..end];
            &slots[..1]
        } else {
            let planes = self.dec.frame_planes();
            let n = planes.len().min(crate::layout::MAX_PLANES);
            for (i, p) in planes.iter().enumerate().take(n) {
                slots[i] = &p[start..end];
            }
            &slots[..n]
        };
        on_frame(Frame {
            sample_rate: rate,
            samples: shown,
            planar,
            meta: self.dec.last_meta(),
        })
    }
}
