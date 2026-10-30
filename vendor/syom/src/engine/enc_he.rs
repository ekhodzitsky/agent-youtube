//! HE v1 access units (TASK-89): output-rate PCM → [`SbrPrep`] (halfband
//! core + 64-band slots) → the LC core at half rate, band-limited at the
//! SBR crossover, with the SBR `extension_payload` of the same AU
//! attached as a FIL before `END`. One AU per 2048 output samples; the
//! whole-stream `bitrate_bps` budget covers core + SBR. Crate-internal;
//! options, ADTS/M4A signalling and the public surface are TASK-90.

use super::enc_frame::LcEncoder;
use super::enc_ps_bits::PsWriter;
use super::enc_ps_est::PsFrameParams;
use super::enc_sbr_bits::sbr_extension_payload;
use super::enc_sbr_est::{SLOTS, SbrEstimator};
use super::enc_sbr_prep::{CORE_FRAME, FIR_DELAY, OUT_FRAME, QMF_SLOT, SbrPrep, he_core_rate};
use super::enc_sbr_qmf::{BANDS, EncSlot, core_cutoff_hz};
use super::error::{Error, Result};

/// Core kbps/channel at or below which a narrow frame may end the LC
/// core under the SBR crossover. 24 covers both 24 kbps speech rates
/// (12 and 24 per channel).
const SHAPE_CORE_KBPS: u32 = 24;
/// Output-rate samples per access unit.
pub(crate) const OUT_SAMPLES_PER_AU: u64 = OUT_FRAME as u64;
/// Analysis slots the envelope grid of AU `n` starts before core frame
/// `n − 1` (`tHFGen − tHFAdj`); AU `n` decodes core frame `n − 1` (LC
/// priming).
const GRID_LEAD: i64 = 6;
/// PS configuration refresh period in access units (a join point).
const PS_HEADER_EVERY: u64 = 8;
/// Output-rate delay of the decoder's SBR QMF chain on top of LC
/// priming and the halfband: measured through `SbrDecoder` with a
/// low-frequency burst (cross-correlation lag; the prototype filters
/// make it half-integral, rounded up). Pinned by the tests.
pub(crate) const SBR_DELAY_OUT: u64 = 962;
/// Output-rate samples from source sample 0 to its decoded position:
/// LC priming (1024 core), halfband (4 core), SBR chain.
pub(crate) const HE_PRIMING_OUT: u64 =
    2 * (CORE_FRAME as u64 + FIR_DELAY as u64 / 2) + SBR_DELAY_OUT;

/// End-of-stream tallies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeInfo {
    /// Output-rate source samples pushed.
    pub source: u64,
    /// Access units emitted (each 2048 output samples).
    pub aus: u64,
    /// Core samples carrying source content (halfband delay included).
    pub core_content: u64,
    /// Decoded output samples before source sample 0.
    pub priming_out: u64,
    /// Decoded output samples after the last source sample.
    pub remainder_out: u64,
}

/// Mono / stereo HE v1 encoder producing raw `raw_data_block`s.
pub(crate) struct HeEncoder {
    lc: Box<LcEncoder>,
    channels: usize,
    lookahead: bool,
    out_rate: u32,
    /// Apply [`core_cutoff_hz`] per frame (low-rate HE only).
    shape_core: bool,
    prep: [SbrPrep; 2],
    est: [SbrEstimator; 2],
    core: [Vec<f32>; 2],
    slots: [Vec<EncSlot>; 2],
    /// Absolute index of `slots[ch][0]`.
    slot_base: u64,
    /// Core frames handed to the LC (next AU index).
    frames: u64,
    aus: u64,
    source: u64,
    /// Lookahead: the fill of the frame the LC still holds.
    pending_fill: Vec<u8>,
    last_fill_len: usize,
    finished: bool,
    /// HE v2: PS payload writer and the parameter frames waiting for
    /// their access unit (`enc_he_ps.rs` feeds them; mono core only).
    ps: Option<(PsWriter, std::collections::VecDeque<PsFrameParams>)>,
}

/// Slots of core frame `n` inside a queue whose index 0 is absolute
/// `slot_base` (the caller passes `start = n·SLOTS − slot_base`).
fn frame_slots(slots: &[EncSlot], start: usize) -> &[EncSlot] {
    if start >= slots.len() {
        &[]
    } else {
        &slots[start..(start + SLOTS).min(slots.len())]
    }
}

impl HeEncoder {
    /// `out_rate` must be twice an ADTS table rate (16–48 kHz); the core
    /// runs at half of it. `bitrate_bps` is the whole-stream budget.
    pub(crate) fn new(
        out_rate: u32,
        channels: usize,
        bitrate_bps: u32,
        lookahead: bool,
    ) -> Result<Self> {
        let core_rate = he_core_rate(out_rate)?;
        if !(1..=2).contains(&channels) {
            return Err(Error::Format("HE encoder: channels must be 1 or 2"));
        }
        let mut lc =
            Box::new(LcEncoder::new(core_rate, channels, bitrate_bps)?.with_lookahead(lookahead));
        let kbps = bitrate_bps / 1000 / channels as u32;
        let est = [
            SbrEstimator::new(out_rate, kbps)?,
            SbrEstimator::new(out_rate, kbps)?,
        ];
        let k0_hz = u64::from(out_rate) * est[0].bands().k_x as u64 / (2 * BANDS as u64);
        lc.set_cutoff_hz(Some(k0_hz as u32));
        Ok(Self {
            lc,
            channels,
            lookahead,
            out_rate,
            shape_core: kbps <= SHAPE_CORE_KBPS,
            prep: [SbrPrep::new(), SbrPrep::new()],
            est,
            core: [Vec::new(), Vec::new()],
            slots: [Vec::new(), Vec::new()],
            slot_base: 0,
            frames: 0,
            aus: 0,
            source: 0,
            pending_fill: Vec::new(),
            last_fill_len: 0,
            finished: false,
            ps: None,
        })
    }

    /// Wire `sampling_frequency_index` of the core (ADTS implicit SBR).
    pub(crate) fn fs_index(&self) -> u8 {
        self.lc.fs_index()
    }

    /// `extension_payload` bytes of the most recently emitted AU.
    #[cfg(test)]
    pub(crate) fn last_fill_len(&self) -> usize {
        self.last_fill_len
    }

    /// Drop all signal; keep the prepared tables and buffers.
    pub(crate) fn reset(&mut self) {
        self.lc.reset();
        for ch in 0..2 {
            self.prep[ch].reset();
            self.est[ch].reset();
            self.core[ch].clear();
            self.slots[ch].clear();
        }
        self.slot_base = 0;
        self.frames = 0;
        self.aus = 0;
        self.source = 0;
        self.pending_fill.clear();
        self.last_fill_len = 0;
        self.finished = false;
        if let Some((w, q)) = &mut self.ps {
            w.reset();
            q.clear();
        }
    }

    /// Carry parametric stereo (HE v2). Mono core only. `fine_iid`
    /// writes `iid_mode` 4; the product path passes `false` (mode 1).
    pub(crate) fn enable_ps(&mut self, fine_iid: bool) -> Result<()> {
        if self.channels != 1 {
            return Err(Error::Format("HE encoder: PS needs a mono core"));
        }
        let w = if fine_iid {
            PsWriter::new().with_fine_iid()
        } else {
            PsWriter::new()
        };
        self.ps = Some((w, std::collections::VecDeque::new()));
        Ok(())
    }

    /// (`ps_data` bits, extended-data block bits) written so far.
    pub(crate) fn ps_side_info(&self) -> (u64, u64) {
        match &self.ps {
            Some((w, _)) => (w.ps_bits(), w.ext_bits()),
            None => (0, 0),
        }
    }

    /// Queue the PS parameters of the next uncovered access unit.
    pub(crate) fn push_ps(&mut self, p: PsFrameParams) {
        if let Some((_, q)) = &mut self.ps {
            q.push_back(p);
        }
    }

    /// Push planar output-rate PCM of any length; complete AUs go to
    /// `on_au` in order.
    pub(crate) fn push<F>(&mut self, planes: &[&[f32]], mut on_au: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if self.finished {
            return Err(Error::Format("HE encoder: finished"));
        }
        if planes.len() != self.channels || planes.iter().any(|p| p.len() != planes[0].len()) {
            return Err(Error::Format("HE encoder: bad plane shape"));
        }
        for (ch, plane) in planes.iter().enumerate() {
            let (prep, core, slots) = (&mut self.prep[ch], &mut self.core[ch], &mut self.slots[ch]);
            prep.push(plane, core, |s| slots.push(*s))?;
        }
        self.source += planes[0].len() as u64;
        while self.core[0].len() >= CORE_FRAME {
            self.encode_core_frame(&mut on_au)?;
        }
        Ok(())
    }

    /// End of input: flush the filterbanks, code the padded content tail
    /// and silent drain frames until priming plus the source fit, and
    /// report the accounting.
    pub(crate) fn finish<F>(&mut self, mut on_au: F) -> Result<HeInfo>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if self.finished {
            return Err(Error::Format("HE encoder: finished"));
        }
        if self.source == 0 {
            return Err(Error::Format("HE encoder: no input"));
        }
        for ch in 0..self.channels {
            let (prep, core, slots) = (&mut self.prep[ch], &mut self.core[ch], &mut self.slots[ch]);
            prep.finish(core, |s| slots.push(*s))?;
        }
        let core_content = (self.source + FIR_DELAY as u64).div_ceil(2);
        let content_frames = core_content.div_ceil(CORE_FRAME as u64);
        while self.frames < content_frames {
            for core in self.core.iter_mut().take(self.channels) {
                if core.len() < CORE_FRAME {
                    core.resize(CORE_FRAME, 0.0);
                }
            }
            self.encode_core_frame(&mut on_au)?;
        }
        // Drain: one silent frame so the last content overlap-adds.
        // The SBR chain can still leave the decoded tail short of the
        // source by up to SBR_DELAY_OUT samples (priming is 3018). One
        // extra silent core frame closes that; lengths that already
        // cover priming + source stay byte-identical. With lookahead the
        // held frame is counted in `frames` and emitted by the flush
        // below, so the comparison is against `frames`, not `aus`.
        loop {
            for core in self.core.iter_mut().take(self.channels) {
                core.clear();
                core.resize(CORE_FRAME, 0.0);
            }
            self.encode_core_frame(&mut on_au)?;
            if self.frames.saturating_mul(OUT_FRAME as u64) >= HE_PRIMING_OUT + self.source {
                break;
            }
        }
        if self.lookahead {
            self.lc.set_fill(&std::mem::take(&mut self.pending_fill))?;
            if let Some(au) = self.lc.flush()? {
                self.emit(&au, &mut on_au)?;
            }
        }
        self.finished = true;
        let coded_out = self.aus * OUT_FRAME as u64;
        Ok(HeInfo {
            source: self.source,
            aus: self.aus,
            core_content,
            priming_out: HE_PRIMING_OUT,
            remainder_out: coded_out.saturating_sub(HE_PRIMING_OUT + self.source),
        })
    }

    fn emit<F>(&mut self, au: &[u8], on_au: &mut F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        self.aus += 1;
        on_au(au)
    }

    /// Slots `[32(n−1) − 6, 32(n−1) + 26)` for AU `n`; out-of-range
    /// indices are silence (before the stream, or past the flushed
    /// banks, whose output is exactly zero there).
    fn slot_window(&self, ch: usize, n: u64) -> Vec<EncSlot> {
        let zero = EncSlot {
            re: [0.0; BANDS],
            im: [0.0; BANDS],
        };
        let first = (n as i64 - 1) * SLOTS as i64 - GRID_LEAD;
        (0..SLOTS as i64)
            .map(|i| {
                let abs = first + i;
                if abs < 0 {
                    return zero;
                }
                let rel = abs as u64;
                if rel < self.slot_base {
                    return zero;
                }
                self.slots[ch]
                    .get((rel - self.slot_base) as usize)
                    .copied()
                    .unwrap_or(zero)
            })
            .collect()
    }

    /// SBR `extension_payload` for AU `n` (header in every AU).
    fn fill_for(&mut self, n: u64) -> Result<Vec<u8>> {
        let mut params = Vec::with_capacity(self.channels);
        for ch in 0..self.channels {
            let win = self.slot_window(ch, n);
            params.push(self.est[ch].estimate(&win)?);
        }
        let refs: Vec<_> = params.iter().collect();
        // PS: AU n carries analysis frame n. The decoder reaches a frame's
        // parameters at the END of its PS frame, so sending frame n one
        // unit early centres the interpolation ramp on the source event
        // (−0.5 k instead of +1.5 k samples; lab/quality/HE_V2.md). The
        // configuration repeats every PS_HEADER_EVERY units; an empty
        // queue (tail, drain) holds the last parameters.
        let ps = match &mut self.ps {
            Some((w, q)) => {
                let header = n.is_multiple_of(PS_HEADER_EVERY);
                Some(w.frame(q.pop_front().as_ref(), header)?)
            }
            None => None,
        };
        let (header, bands) = (self.est[0].header(), self.est[0].bands());
        sbr_extension_payload(header, true, bands, &refs, ps.as_ref())
    }

    /// Hand the next core frame (already buffered) to the LC with its AU's
    /// SBR fill; lookahead delays the fill by one frame with the LC.
    fn encode_core_frame<F>(&mut self, on_au: &mut F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let n = self.frames;
        if self.shape_core {
            let kx = self.est[0].bands().k_x as usize;
            let start = (n * SLOTS as u64).saturating_sub(self.slot_base) as usize;
            let s0 = frame_slots(&self.slots[0], start);
            let s1 = frame_slots(&self.slots[1], start);
            let ch: [&[EncSlot]; 2] = [s0, s1];
            let hz = core_cutoff_hz(&ch[..self.channels], kx, self.out_rate);
            self.lc.set_cutoff_hz(Some(hz));
        }
        let fill = self.fill_for(n)?;
        let mut frames = [[0.0f32; CORE_FRAME]; 2];
        for (frame, core) in frames
            .iter_mut()
            .zip(self.core.iter_mut())
            .take(self.channels)
        {
            frame.copy_from_slice(&core[..CORE_FRAME]);
            core.drain(..CORE_FRAME);
        }
        let planes: Vec<&[f32]> = frames[..self.channels].iter().map(|f| &f[..]).collect();
        if self.lookahead {
            let prev = std::mem::replace(&mut self.pending_fill, fill);
            self.lc.set_fill(&prev)?;
            self.last_fill_len = prev.len();
            if let Some(au) = self.lc.push_frame(&planes)? {
                self.emit(&au, on_au)?;
            }
        } else {
            self.lc.set_fill(&fill)?;
            self.last_fill_len = fill.len();
            self.lc.encode_into(&planes)?;
            self.aus += 1;
            on_au(self.lc.payload())?;
        }
        self.frames += 1;
        // Slots before AU (n+1)'s window are never read again.
        let keep_from = ((n as i64) * SLOTS as i64 - GRID_LEAD).max(0) as u64;
        if keep_from > self.slot_base {
            let drop = (keep_from - self.slot_base) as usize;
            for slots in self.slots.iter_mut().take(self.channels) {
                slots.drain(..drop.min(slots.len()));
            }
            self.slot_base = keep_from;
        }
        debug_assert_eq!(QMF_SLOT * SLOTS, OUT_FRAME);
        Ok(())
    }
}

#[cfg(test)]
#[path = "enc_he_tests.rs"]
mod enc_he_tests;
