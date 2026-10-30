//! Push (`feed`) delivery of bounded fragmented MP4 (TASK-125, decision-25;
//! budgets: lab/fmp4/REPORT.md §2). The init `moov` (with `mvex`; a flat
//! moov keeps [`UnsupportedFeature::M4aPush`]) installs the ASC and the
//! edit window, then each `moof` is buffered whole (metadata budget) until
//! the following `mdat` header arrives, resolved against that `mdat`
//! payload, and its samples decode one AU at a time as the bytes complete
//! — the per-fragment table is dropped with the fragment, so resident
//! state is one box payload / one AU plus headers, never a lifetime index.
//!
//! Box framing is incremental with absolute stream offsets
//! ([`Decoder::abs_base`]); `mfhd`/`tfdt`/implicit-base continuity lives in
//! the shared [`FragResolver`]. A finish with a pending box or owed samples
//! is a typed [`AacError::Truncated`]; lifecycle and `reset` mirror the
//! ADTS/LATM push rules. A box with size 0 ("to end of stream") has no
//! push-side fence and is out of the envelope.

use crate::engine::asc::AudioSpecificConfig;
use crate::error::{AacError, MalformedKind, Result, UnsupportedFeature};
use crate::isomp4::frag::{moov_is_fragmented, parse_init};
use crate::options::ChannelMode;

use super::{Decoder, Frame};

#[path = "fmp4_state.rs"]
mod state;
pub(super) use state::Fmp4Push;
use state::{Phase, checked_end, fence_box, peek_box};

impl Decoder {
    /// Pump fMP4 boxes out of `buf`; per-AU delivery inside the current
    /// `mdat`. `final_flush` turns any pending box or owed sample into
    /// [`AacError::Truncated`].
    pub(super) fn pump_fmp4<F>(
        &mut self,
        buf: &[u8],
        on_frame: &mut F,
        final_flush: bool,
    ) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        self.pump_fmp4_run(buf, on_frame)?;
        if final_flush {
            let done = match self.fmp4.phase {
                Phase::Drain => true,
                Phase::Init | Phase::Boxes => self.pos == buf.len(),
                // A skip not completed, a moof whose mdat never came, and a
                // fragment still owing samples are all mid-stream cuts.
                Phase::Skip(_) | Phase::Moof { .. } | Phase::Mdat => false,
            };
            if !done {
                return Err(AacError::truncated_at(Some(
                    self.abs_base + self.pos as u64,
                )));
            }
        }
        Ok(())
    }

    fn pump_fmp4_run<F>(&mut self, buf: &[u8], on_frame: &mut F) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        loop {
            match self.fmp4.phase {
                Phase::Drain => {
                    self.pos = buf.len();
                    return Ok(());
                }
                Phase::Mdat => {
                    if !self.pump_fmp4_mdat(buf, on_frame)? {
                        return Ok(()); // waiting for the next AU's bytes
                    }
                }
                Phase::Skip(end) => {
                    let cur = self.abs_base + self.pos as u64;
                    // A sample resolved past this box would wrap the cursor.
                    if end < cur {
                        return Err(AacError::Malformed(MalformedKind::Syntax));
                    }
                    let avail_end = self.abs_base + buf.len() as u64;
                    self.pos += (end.min(avail_end) - cur) as usize;
                    if avail_end < end {
                        return Ok(());
                    }
                    // A skipped box (ftyp, mfra, an mdat tail) may precede
                    // the init moov; only a parsed init ends phase Init.
                    self.fmp4.phase = if self.fmp4.init.is_some() {
                        Phase::Boxes
                    } else {
                        Phase::Init
                    };
                }
                Phase::Moof { start, end, next } => {
                    if !self.pump_fmp4_moof(buf, start, end, next)? {
                        return Ok(()); // waiting for the mdat header
                    }
                }
                Phase::Init | Phase::Boxes => {
                    let pos = self.pos;
                    let Some((size, _head, typ)) = peek_box(buf, pos)? else {
                        return Ok(());
                    };
                    let box_start = self.abs_base + pos as u64;
                    let box_end = checked_end(box_start, size)?;
                    match &typ {
                        b"moov" if matches!(self.fmp4.phase, Phase::Init) => {
                            fence_box(&self.opts.memory, size)?;
                            if size > (buf.len() - pos) as u64 {
                                return Ok(()); // wait for the whole moov
                            }
                            let full = &buf[pos..pos + size as usize];
                            if !moov_is_fragmented(full) {
                                return Err(AacError::Unsupported(UnsupportedFeature::M4aPush));
                            }
                            self.install_fmp4_init(full)?;
                            self.pos += size as usize;
                            self.fmp4.phase = Phase::Boxes;
                        }
                        b"moov" => {
                            return Err(AacError::Unsupported(UnsupportedFeature::FragmentedMp4(
                                "a second init segment is not supported",
                            )));
                        }
                        b"moof" => {
                            if matches!(self.fmp4.phase, Phase::Init) {
                                return Err(AacError::Malformed(MalformedKind::Syntax));
                            }
                            fence_box(&self.opts.memory, size)?;
                            if size > (buf.len() - pos) as u64 {
                                return Ok(()); // wait for the whole moof
                            }
                            self.fmp4.phase = Phase::Moof {
                                start: box_start,
                                end: box_end,
                                next: box_end,
                            };
                        }
                        // An mdat with no pending moof cannot be attributed;
                        // stream past it instead of buffering (residency).
                        _ => self.fmp4.phase = Phase::Skip(box_end),
                    }
                }
            }
        }
    }

    /// Moof buffered at absolute `[start, end)`; resolve it once the
    /// following `mdat` header is available at the walk cursor `next`.
    /// `Ok(false)` = keep waiting. The consume cursor stays at the moof
    /// start until resolution, so compaction cannot drop the moof.
    fn pump_fmp4_moof(&mut self, buf: &[u8], start: u64, end: u64, next: u64) -> Result<bool> {
        let moof_start = usize::try_from(start - self.abs_base)
            .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
        let moof_end = usize::try_from(end - self.abs_base)
            .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
        // The resolver takes the moof's content slice (absolute `start`
        // still names the box for base addressing).
        let moof_head = match peek_box(buf, moof_start)? {
            Some((_, head, _)) => head as usize,
            None => return Ok(false),
        };
        let at = usize::try_from(next - self.abs_base)
            .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
        let Some((size, head, typ)) = peek_box(buf, at)? else {
            return Ok(false);
        };
        match &typ {
            b"mdat" => {
                let content_start = checked_end(next, head)?;
                let content_end = checked_end(next, size)?;
                if content_start < end {
                    return Err(AacError::Malformed(MalformedKind::Syntax));
                }
                let init = self
                    .fmp4
                    .init
                    .as_ref()
                    .ok_or(AacError::Malformed(MalformedKind::Syntax))?;
                self.fmp4.res.add_moof_push(
                    &buf[moof_start + moof_head..moof_end],
                    start,
                    init,
                    (content_start, content_end),
                    &mut self.fmp4.frag,
                    &self.opts.memory,
                )?;
                self.fmp4.frag_idx = 0;
                self.fmp4.mdat_end = content_end;
                self.pos = usize::try_from(content_start - self.abs_base)
                    .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
                self.fmp4.phase = Phase::Mdat;
                Ok(true)
            }
            b"moof" | b"moov" => Err(AacError::Malformed(MalformedKind::Syntax)),
            _ => {
                // Small skippable box between the moof and its mdat: buffer
                // it whole (the moof already is) and continue the walk.
                if ((buf.len() - at) as u64) < size {
                    return Ok(false);
                }
                self.fmp4.phase = Phase::Moof {
                    start,
                    end,
                    next: checked_end(next, size)?,
                };
                Ok(true)
            }
        }
    }

    /// Deliver the current fragment's samples; `Ok(false)` when the next
    /// AU's bytes have not arrived yet.
    fn pump_fmp4_mdat<F>(&mut self, buf: &[u8], on_frame: &mut F) -> Result<bool>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        loop {
            let Some(&(off, size)) = self.fmp4.frag.get(self.fmp4.frag_idx) else {
                let tail = self.fmp4.mdat_end;
                self.fmp4.frag.clear(); // per-fragment table dies here
                self.fmp4.phase = Phase::Skip(tail);
                return Ok(true);
            };
            let cur = self.abs_base + self.pos as u64;
            if off < cur {
                return Err(AacError::Malformed(MalformedKind::Syntax));
            }
            let end = off + u64::from(size);
            if end > self.abs_base + buf.len() as u64 {
                return Ok(false); // wait for the AU
            }
            self.pos += usize::try_from(off - cur)
                .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
            let payload = &buf[self.pos..self.pos + size as usize];
            self.decode_fmp4_au(payload, on_frame)?;
            self.pos += size as usize;
            self.fmp4.frag_idx += 1;
            if matches!(self.fmp4.phase, Phase::Drain) {
                return Ok(true);
            }
        }
    }

    /// Decode one resolved fMP4 sample and emit the edit-windowed slice —
    /// the push twin of `play_m4a_track`'s per-frame block.
    fn decode_fmp4_au<F>(&mut self, payload: &[u8], on_frame: &mut F) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        self.opts.memory.check_declared_au(payload.len() as u64)?;
        let (aot, fs, sr, ch) = self
            .fmp4
            .asc
            .ok_or(AacError::Malformed(MalformedKind::Syntax))?;
        let mono = matches!(self.opts.channel_mode, ChannelMode::Mono);
        let rate = if mono {
            self.mono_scratch.clear();
            self.dec
                .decode_raw_mono_f32(aot, fs, sr, ch, 1, payload, &mut self.mono_scratch)
                .map_err(AacError::from)?
        } else {
            self.dec
                .decode_frame_scaled(aot, fs, sr, ch, payload)
                .map_err(AacError::from)?
        };
        self.aac_frames += 1; // every decoded AU, emitted or skipped
        let (n_ch, n) = if mono {
            (1, self.mono_scratch.len())
        } else {
            let planes = self.dec.frame_planes();
            (planes.len(), planes.first().map_or(0, Vec::len))
        };
        if n_ch == 0 {
            return Ok(()); // channel-less frame: consumed, not emitted
        }
        self.tally_decoded(rate, n_ch, n)?;
        let f = &mut self.fmp4;
        if f.skip_left >= n {
            f.skip_left -= n; // fully inside the edit-list skip region
            return Ok(());
        }
        let start = f.skip_left;
        f.skip_left = 0;
        let mut take = n - start;
        if let Some(left) = f.play_left.as_mut() {
            if *left == 0 {
                f.phase = Phase::Drain; // presentation over: unplayed tail
                return Ok(());
            }
            take = take.min(*left);
            *left -= take;
        }
        if take == 0 {
            return Ok(());
        }
        self.samples_out += take as u64;
        self.emitted += 1;
        let end = start + take;
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
            samples: take,
            planar,
            meta: self.dec.last_meta(),
        })
    }

    /// Install the init segment: ASC onto the engine and the edit window.
    fn install_fmp4_init(&mut self, moov: &[u8]) -> Result<()> {
        let init = parse_init(moov, &self.opts.memory)?;
        let (asc, _) = AudioSpecificConfig::parse(&init.asc).map_err(AacError::from)?;
        let out = asc.output_sample_rate;
        if out == 0 || out > self.opts.max_sample_rate {
            return Err(AacError::sample_rate(out, self.opts.max_sample_rate));
        }
        if let Some(pce) = asc.pce.clone() {
            self.dec.set_config_pce(pce);
        }
        self.dec
            .set_he_config(asc.sbr_present, asc.ps_present, asc.output_sample_rate);
        self.dec.set_frame_samples(asc.frame_samples);
        let (skip, play) = init.edit_window(out)?;
        self.fmp4.asc = Some((
            asc.aot,
            asc.sampling_frequency_index,
            asc.sample_rate,
            asc.channel_configuration,
        ));
        self.fmp4.skip_left = skip;
        self.fmp4.skip0 = skip;
        self.fmp4.play_left = play;
        self.fmp4.init = Some(init);
        Ok(())
    }

    /// fMP4 `StreamInfo` priming/remainder, mirroring `play_m4a_track`.
    pub(super) fn fmp4_trim(&self) -> (Option<u64>, Option<u64>) {
        let has_elst = self.fmp4.init.as_ref().is_some_and(|i| i.has_elst);
        (
            has_elst.then_some(self.fmp4.skip0 as u64),
            has_elst.then_some(
                self.fmp4
                    .res
                    .total()
                    .saturating_sub(self.fmp4.skip0 as u64)
                    .saturating_sub(self.samples_out),
            ),
        )
    }
}
