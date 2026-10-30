//! Multichannel AAC-LC element orchestration (TASK-114, spec in
//! `lab/quality/MC_ENC.md`): `channel_configuration` 3–7 as one
//! [`LcEncoder`] per syntactic element, spliced into one
//! `raw_data_block` in ISO order — SCE(C), CPE(front), [CPE(side)],
//! CPE(back) or SCE(BC), LFE.
//!
//! Input planes use the decoder's public order (TASK-62): 3.0 FL FR FC,
//! 4.0 + BC, 5.0 FL FR FC BL BR, 5.1 FL FR FC LFE BL BR, 7.1 FL FR FC LFE
//! BL BR SL SR. ABR (TASK-115): one allowed-noise offset is searched for
//! the whole frame, so every element sits at the same noise-to-mask
//! distance and bits follow demand; one frame of credit and ABR stuffing
//! work as in stereo. Quality VBR (and the `fixed_split` lab switch) run
//! each element's own loop on a weight share (SCE 1, CPE 2, LFE 0.1).
//! Elements keep the 6144 bits/channel cap. LFE: long, no TNS, 120 Hz.

use super::bits::BitWriter;
use super::enc_frame::LcEncoder;
use super::error::{Error, Result};
use super::swb::LONG_WINDOW_LEN;

/// Syntactic element kind with its 3-bit `id_syn_ele`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Sce = 0,
    Cpe = 1,
    Lfe = 3,
}

/// One element: kind, `element_instance_tag`, source plane indices.
type Route = (Kind, u8, [usize; 2]);

/// Element routing per plane count (bitstream order).
fn routes(planes: usize) -> Option<(u8, &'static [Route])> {
    use Kind::{Cpe, Lfe, Sce};
    Some(match planes {
        3 => (3, &[(Sce, 0, [2, 2]), (Cpe, 0, [0, 1])]),
        4 => (4, &[(Sce, 0, [2, 2]), (Cpe, 0, [0, 1]), (Sce, 1, [3, 3])]),
        5 => (5, &[(Sce, 0, [2, 2]), (Cpe, 0, [0, 1]), (Cpe, 1, [3, 4])]),
        6 => (
            6,
            &[
                (Sce, 0, [2, 2]),
                (Cpe, 0, [0, 1]),
                (Cpe, 1, [4, 5]),
                (Lfe, 0, [3, 3]),
            ],
        ),
        8 => (
            7,
            &[
                (Sce, 0, [2, 2]),
                (Cpe, 0, [0, 1]),
                (Cpe, 1, [6, 7]),
                (Cpe, 2, [4, 5]),
                (Lfe, 0, [3, 3]),
            ],
        ),
        _ => return None,
    })
}

/// Budget weight in tenths (SCE 1, CPE 2, LFE 0.1).
fn weight(kind: Kind) -> u64 {
    match kind {
        Kind::Sce => 10,
        Kind::Cpe => 20,
        Kind::Lfe => 1,
    }
}

/// `channel_configuration` for a surround plane count (3, 4, 5, 6, 8).
#[must_use]
pub fn config_for(planes: usize) -> Option<u8> {
    routes(planes).map(|r| r.0)
}

/// Multichannel LC encoder: one `raw_data_block` per 1024 samples/plane.
pub struct McEncoder {
    config: u8,
    routes: &'static [Route],
    elems: Vec<LcEncoder>,
    fs_index: u8,
    /// Whole-frame ABR budget in bits, unspent credit, stuffing debt.
    budget: usize,
    credit: i64,
    pad_debt: i64,
    /// Previous frame's common noise offset.
    rate_guess: i32,
    /// Per-element rate loops on the weight split (quality VBR, lab A/B).
    fixed_split: bool,
    specs: Vec<super::enc_frame::Specs>,
}

impl McEncoder {
    /// `planes` must be 3, 4, 5, 6 or 8; `bitrate_bps` is whole-stream.
    #[cfg(test)]
    pub fn new(sample_rate: u32, planes: usize, bitrate_bps: u32) -> Result<Self> {
        Self::new_with(sample_rate, planes, bitrate_bps, LcEncoder::new)
    }

    /// [`Self::new`] with the caller's element factory `(rate, channels,
    /// bitrate share)` so encode options apply to every element.
    pub fn new_with(
        sample_rate: u32,
        planes: usize,
        bitrate_bps: u32,
        make: impl Fn(u32, usize, u32) -> Result<LcEncoder>,
    ) -> Result<Self> {
        let (config, routes) =
            routes(planes).ok_or(Error::Format("LC encoder: planes must be 3, 4, 5, 6 or 8"))?;
        let total: u64 = routes.iter().map(|r| weight(r.0)).sum();
        let mut elems = Vec::with_capacity(routes.len());
        for &(kind, _, _) in routes {
            let ch = if kind == Kind::Cpe { 2 } else { 1 };
            let share = (u64::from(bitrate_bps) * weight(kind) / total).max(1);
            let cap = u64::from(super::enc_frame::max_bitrate_bps(sample_rate, ch));
            let mut e = make(sample_rate, ch, share.min(cap) as u32)?;
            if kind == Kind::Lfe {
                e.set_lfe();
            }
            elems.push(e);
        }
        let fs_index = elems.first().map_or(0, LcEncoder::fs_index);
        let cap: usize = routes
            .iter()
            .map(|r| if r.0 == Kind::Cpe { 2 } else { 1 })
            .sum::<usize>()
            * super::enc_frame::MAX_BITS_PER_CHANNEL;
        let budget = (u64::from(bitrate_bps) * 1024 / u64::from(sample_rate)) as usize;
        let fixed_split = elems.iter().any(LcEncoder::is_quality);
        Ok(Self {
            config,
            routes,
            specs: vec![[[0.0; LONG_WINDOW_LEN]; 2]; elems.len()],
            elems,
            fs_index,
            budget: budget.min(cap).min(super::enc_frame::MAX_PAYLOAD_BYTES * 8),
            credit: 0,
            pad_debt: 0,
            rate_guess: OFFSET_LO,
            fixed_split,
        })
    }

    /// `channel_configuration` (3–7) for the ADTS header / ASC.
    #[must_use]
    pub fn channel_configuration(&self) -> u8 {
        self.config
    }

    #[must_use]
    pub fn fs_index(&self) -> u8 {
        self.fs_index
    }

    /// Input planes this encoder takes.
    #[must_use]
    pub fn planes(&self) -> usize {
        self.routes.iter().map(|r| r.2[1]).max().unwrap_or(0) + 1
    }

    /// Drop overlap and rate state of every element.
    pub fn reset(&mut self) {
        for e in &mut self.elems {
            e.reset();
        }
        self.credit = 0;
        self.pad_debt = 0;
        self.rate_guess = OFFSET_LO;
    }

    /// Test hook: force the next common-offset search to start cold.
    #[cfg(test)]
    pub(crate) fn pin_rate_guess(&mut self, v: i32) {
        self.rate_guess = v;
    }

    /// Lab A/B: per-element loops on the fixed weight split (TASK-114).
    #[cfg(test)]
    pub fn set_fixed_split(&mut self, on: bool) {
        self.fixed_split = on;
    }

    /// Encode 1024 samples per plane (public plane order) into `out`.
    pub fn encode_into(&mut self, pcm: &[&[f32]], out: &mut Vec<u8>) -> Result<()> {
        if pcm.len() != self.planes() || pcm.iter().any(|p| p.len() != LONG_WINDOW_LEN) {
            return Err(Error::Format("LC encoder: bad frame shape"));
        }
        if !self.fixed_split {
            return self.encode_common(pcm, out);
        }
        let mut w = BitWriter::from_vec(std::mem::take(out));
        let mut padded = 0usize;
        for (enc, &(kind, tag, src)) in self.elems.iter_mut().zip(self.routes.iter()) {
            if kind == Kind::Cpe {
                enc.encode_into(&[pcm[src[0]], pcm[src[1]]])?;
            } else {
                enc.encode_into(&[pcm[src[0]]])?;
            }
            let rdb = enc.payload();
            padded += rdb.len();
            // The element's own block is `id tag body END pad`: END (111)
            // holds the last set bit, so the body is bits [7, last − 2).
            let last = last_set_bit(rdb).ok_or(Error::Format("LC encoder: empty element"))?;
            w.write(kind as u32, 3);
            w.write(u32::from(tag), 4);
            append_bits(&mut w, rdb, 7, last - 2);
        }
        // Keep the elements' ABR stuffing as EXT_FILL FILs before END.
        let base = w.bit_len() as usize;
        let target = padded.min(super::enc_frame::MAX_PAYLOAD_BYTES);
        let room = target.saturating_mul(8).saturating_sub(base + 3);
        super::enc_pad::write_pad_fill(&mut w, super::enc_pad::pad_fill_for_room(room));
        w.write(7, 3); // END
        let bytes = w.finish();
        *out = bytes;
        Ok(())
    }
}

impl McEncoder {
    /// Frame bits with every element at `offset` (each element's build
    /// carries its own END + pad; the frame has one).
    fn bits_at(&mut self, offset: i32) -> usize {
        let each: usize = self
            .elems
            .iter_mut()
            .zip(self.specs.iter())
            .zip(self.routes.iter())
            .map(|((e, s), r)| e.mc_bits(s, elem_offset(r.0, offset)))
            .sum();
        each - 10 * (self.elems.len() - 1)
    }

    /// ABR with one allowed-noise offset for the whole frame.
    fn encode_common(&mut self, pcm: &[&[f32]], out: &mut Vec<u8>) -> Result<()> {
        for ((enc, specs), &(kind, _, src)) in self
            .elems
            .iter_mut()
            .zip(self.specs.iter_mut())
            .zip(self.routes.iter())
        {
            *specs = if kind == Kind::Cpe {
                enc.mc_analyze(&[pcm[src[0]], pcm[src[1]]])?
            } else {
                enc.mc_analyze(&[pcm[src[0]]])?
            };
        }
        let spend = self.budget + self.credit.min(self.budget as i64 / 2) as usize;
        let limit = spend.saturating_sub(32);
        let guess = self.rate_guess;
        let offset = super::enc_frame::search_monotonic(OFFSET_LO, OFFSET_HI, guess, |off| {
            self.bits_at(off) <= limit
        });
        self.rate_guess = offset;
        // Over budget even at the ceiling (short-window bursts): every
        // element drops top bands to its proportional share, as the
        // stereo loop's hard cap does.
        let total = self.bits_at(offset);
        let scale = (total > limit).then_some((limit, total));
        let mut w = BitWriter::from_vec(std::mem::take(out));
        let mut silent = true;
        for ((enc, specs), &(kind, tag, _)) in self
            .elems
            .iter_mut()
            .zip(self.specs.iter())
            .zip(self.routes.iter())
        {
            let at = elem_offset(kind, offset);
            let cap = scale.map_or(usize::MAX, |(limit, total)| {
                enc.mc_bits(specs, at) * limit / total
            });
            let (_, zero) = enc.mc_emit(specs, at, cap);
            silent &= zero;
            let rdb = enc.payload();
            let last = last_set_bit(rdb).ok_or(Error::Format("LC encoder: empty element"))?;
            w.write(kind as u32, 3);
            w.write(u32::from(tag), 4);
            append_bits(&mut w, rdb, 7, last - 2);
        }
        // ABR stuffing before END as EXT_FILL FILs, as in stereo
        // (`fit_budget`); trailing zeros are rejected by fdk-aac.
        let base = w.bit_len() as usize;
        let coded = (base + 3).div_ceil(8) * 8;
        let pad_to = self.budget.saturating_sub(self.pad_debt.max(0) as usize);
        if !silent && coded < pad_to * 97 / 100 {
            let room = (pad_to / 8).saturating_mul(8).saturating_sub(base + 3);
            super::enc_pad::write_pad_fill(&mut w, super::enc_pad::pad_fill_for_room(room));
        }
        w.write(7, 3); // END
        let bytes = w.finish();
        let emitted = (bytes.len() * 8) as i64;
        self.pad_debt = (self.pad_debt + emitted - self.budget as i64).max(0);
        self.credit =
            (self.credit + self.budget as i64 - coded as i64).clamp(0, self.budget as i64);
        *out = bytes;
        Ok(())
    }
}

/// LFE codes 12 dB below the common allowed noise: a lone sub-bass tone
/// has nothing else in its element to mask the noise, and the element
/// costs a few dozen bits (lab/quality/MC_ALLOC.md).
const LFE_BIAS: i32 = -16;

fn elem_offset(kind: Kind, offset: i32) -> i32 {
    if kind == Kind::Lfe {
        (offset + LFE_BIAS).max(OFFSET_LO)
    } else {
        offset
    }
}

/// Allowed-noise offset search range (the LC rate loop's).
const OFFSET_LO: i32 = -60;
const OFFSET_HI: i32 = 240;

/// Index (MSB-first) of the last set bit.
fn last_set_bit(bytes: &[u8]) -> Option<usize> {
    let i = bytes.iter().rposition(|&b| b != 0)?;
    Some(i * 8 + 7 - bytes[i].trailing_zeros() as usize)
}

/// Append bits `[from, to)` of `bytes` (MSB-first) to `w`.
fn append_bits(w: &mut BitWriter, bytes: &[u8], from: usize, to: usize) {
    let mut at = from;
    while at < to {
        let in_byte = 8 - at % 8;
        let n = in_byte.min(to - at);
        let v = (u32::from(bytes[at / 8]) >> (in_byte - n)) & ((1u32 << n) - 1);
        w.write(v, n as u32);
        at += n;
    }
}

#[cfg(test)]
#[path = "enc_mc_tests.rs"]
mod enc_mc_tests;
