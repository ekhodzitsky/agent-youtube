//! `moof`/`traf` fragment resolution for bounded fMP4 (see
//! [`super`](super)): `mfhd` sequence continuity, `tfhd`/`tfdt`/`trun`
//! parsing, the three base addressing modes, the trun → tfhd → trex
//! defaults chain and the following-`mdat` fence.

use super::super::{AacTrack, BoxHdr, BoxIter, fence_depth, fence_entries, read_u32, read_u64};
use super::{Fmp4Init, unsupported};
use crate::budgets::{BudgetExceeded, BudgetKind, MemoryBudgets};
use crate::error::{AacError, MalformedKind, Result};

/// tfhd flag-selected fields (spec order).
struct Tfhd {
    track_id: u32,
    base_offset: Option<u64>,
    desc_index: Option<u32>,
    default_duration: Option<u32>,
    default_size: Option<u32>,
    duration_is_empty: bool,
    default_base_is_moof: bool,
}

fn parse_tfhd(body: &[u8]) -> Result<Tfhd> {
    let flags = read_u32(body, 0)? & 0x00FF_FFFF;
    let track_id = read_u32(body, 4)?;
    let mut o = 8usize;
    let base_offset = if flags & 0x1 != 0 {
        let v = read_u64(body, o)?;
        o += 8;
        Some(v)
    } else {
        None
    };
    let desc_index = if flags & 0x2 != 0 {
        let v = read_u32(body, o)?;
        o += 4;
        Some(v)
    } else {
        None
    };
    let default_duration = if flags & 0x8 != 0 {
        let v = read_u32(body, o)?;
        o += 4;
        Some(v)
    } else {
        None
    };
    let default_size = if flags & 0x10 != 0 {
        let v = read_u32(body, o)?;
        o += 4;
        Some(v)
    } else {
        None
    };
    if flags & 0x20 != 0 {
        let _ = read_u32(body, o)?; // default_sample_flags: no audio semantics
    }
    Ok(Tfhd {
        track_id,
        base_offset,
        desc_index,
        default_duration,
        default_size,
        duration_is_empty: flags & 0x1_0000 != 0,
        default_base_is_moof: flags & 0x2_0000 != 0,
    })
}

fn parse_tfdt(body: &[u8]) -> Result<u64> {
    match body.first() {
        Some(0) => Ok(u64::from(read_u32(body, 4)?)),
        Some(1) => read_u64(body, 4),
        _ => Err(AacError::Malformed(MalformedKind::Syntax)),
    }
}

/// Fragment-resolution state across `moof`s, in arrival order. One-shot
/// callers accumulate the whole input's sample ranges in `frames` (same
/// class as the flat `stsz`/`stco` index, fenced by the metadata budget);
/// the push model ([`Self::add_moof_push`], TASK-125) resolves into a
/// per-fragment caller table that is dropped with the fragment.
#[derive(Default)]
pub(crate) struct FragResolver {
    pub(super) frames: Vec<(u64, u32)>,
    /// Σ resolved sample durations (media timescale).
    pub(super) total: u64,
    /// Next decode timestamp (`tfdt` when present, else accumulated).
    pub(super) dts: u64,
    last_seq: Option<u32>,
    prev_data_end: Option<u64>,
    /// Per-sample dts, recorded only when `trace_dts` is set (timeline tests).
    /// The push path leaves it off, so steady-state allocation is unchanged.
    #[cfg(test)]
    pub(super) trace_dts: bool,
    #[cfg(test)]
    pub(super) sample_dts: Vec<u64>,
}

impl FragResolver {
    /// `tfdt`/mfhd state is kept; the implicit-base anchor is not (a reset
    /// push decoder starts a new stream).
    pub(crate) fn reset(&mut self) {
        self.frames.clear();
        self.total = 0;
        self.dts = 0;
        self.last_seq = None;
        self.prev_data_end = None;
        #[cfg(test)]
        {
            self.trace_dts = false;
            self.sample_dts.clear();
        }
    }

    /// Σ resolved sample durations (media timescale) so far.
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// Resolve one `moof` (content slice at absolute `moof_pos`, box ending
    /// at `moof_end`) against the init facts, appending sample ranges.
    /// `mdats` are the absolute `(content_start, content_end)` of every
    /// `mdat` payload in file order.
    pub(crate) fn add_moof(
        &mut self,
        moof: &[u8],
        moof_pos: u64,
        moof_end: u64,
        init: &Fmp4Init,
        mdats: &[(u64, u64)],
        mem: &MemoryBudgets,
    ) -> Result<()> {
        let fence = mdats.iter().copied().find(|&(s, _)| s >= moof_end);
        let mut frames = std::mem::take(&mut self.frames);
        let res = self.resolve_moof(moof, moof_pos, init, fence, &mut frames, mem);
        self.frames = frames;
        res
    }

    /// Push-mode resolution (TASK-125): one `moof` resolved against the
    /// single following `mdat` payload, ranges appended to the caller's
    /// per-fragment `out` (fenced per fragment, then dropped).
    pub(crate) fn add_moof_push(
        &mut self,
        moof: &[u8],
        moof_pos: u64,
        init: &Fmp4Init,
        mdat: (u64, u64),
        out: &mut Vec<(u64, u32)>,
        mem: &MemoryBudgets,
    ) -> Result<()> {
        self.resolve_moof(moof, moof_pos, init, Some(mdat), out, mem)
    }

    /// Sample ranges must land inside the mdat payload fence (catches the
    /// ffmpeg 7.0.2 global_sidx misdeclaration, lab/fmp4/REPORT.md §4.2).
    fn resolve_moof(
        &mut self,
        moof: &[u8],
        moof_pos: u64,
        init: &Fmp4Init,
        fence: Option<(u64, u64)>,
        frames: &mut Vec<(u64, u32)>,
        mem: &MemoryBudgets,
    ) -> Result<()> {
        fence_depth(mem, 1)?;
        let mut traf: Option<BoxHdr> = None;
        for child in BoxIter::new(moof, 0, moof.len()) {
            let (hdr, typ) = child?;
            match &typ {
                b"mfhd" => {
                    let seq = read_u32(&moof[hdr.content_start..hdr.content_end], 4)?;
                    if let Some(last) = self.last_seq
                        && seq != last.wrapping_add(1)
                    {
                        return Err(AacError::Malformed(MalformedKind::Syntax));
                    }
                    self.last_seq = Some(seq);
                }
                b"traf" => {
                    if traf.is_some() {
                        return unsupported("multiple track fragments per moof are not supported");
                    }
                    traf = Some(hdr);
                }
                _ => {}
            }
        }
        let traf = traf.ok_or(AacError::Malformed(MalformedKind::Syntax))?;
        fence_depth(mem, 2)?;
        let mut tfhd = None;
        let mut tfdt = None;
        for child in BoxIter::new(moof, traf.content_start, traf.content_end) {
            let (hdr, typ) = child?;
            let body = &moof[hdr.content_start..hdr.content_end];
            match &typ {
                b"tfhd" => tfhd = Some(parse_tfhd(body)?),
                b"tfdt" => tfdt = Some(parse_tfdt(body)?),
                b"saiz" | b"saio" | b"senc" => {
                    return unsupported("encrypted fragments (saiz/saio/senc) are not supported");
                }
                _ => {}
            }
        }
        let tfhd = tfhd.ok_or(AacError::Malformed(MalformedKind::Syntax))?;
        if tfhd.track_id != init.track_id {
            return unsupported("a fragment for another track is not supported");
        }
        if tfhd.desc_index.is_some_and(|d| d != 1) {
            return unsupported("a sample-description switch is not supported");
        }
        if let Some(t) = tfdt {
            self.dts = t;
        }
        let base = match (
            tfhd.base_offset,
            tfhd.default_base_is_moof,
            self.prev_data_end,
        ) {
            (Some(b), _, _) => b,
            (None, true, _) => moof_pos,
            (None, false, Some(e)) => e,
            (None, false, None) => moof_pos,
        };
        let mut next_off: Option<u64> = None;
        for child in BoxIter::new(moof, traf.content_start, traf.content_end) {
            let (hdr, typ) = child?;
            if &typ == b"trun" {
                next_off = Some(self.resolve_trun(
                    &moof[hdr.content_start..hdr.content_end],
                    base,
                    next_off,
                    &tfhd,
                    init,
                    fence,
                    frames,
                    mem,
                )?);
            }
        }
        if let Some(end) = next_off {
            self.prev_data_end = Some(end);
        }
        Ok(())
    }

    /// Resolve one `trun`; returns the end of its sample data. Without a
    /// `data_offset` the samples continue the previous trun's data
    /// (`next_off`), or start at the fragment base for the first trun.
    /// Ranges go to `frames` (the accumulated index one-shot, a per-fragment
    /// table on the push path).
    #[allow(clippy::too_many_arguments)]
    fn resolve_trun(
        &mut self,
        body: &[u8],
        base: u64,
        next_off: Option<u64>,
        tfhd: &Tfhd,
        init: &Fmp4Init,
        fence: Option<(u64, u64)>,
        frames: &mut Vec<(u64, u32)>,
        mem: &MemoryBudgets,
    ) -> Result<u64> {
        if body.first().copied().unwrap_or(2) > 1 {
            return Err(AacError::Malformed(MalformedKind::Syntax));
        }
        let flags = read_u32(body, 0)? & 0x00FF_FFFF;
        let count = read_u32(body, 4)? as usize;
        let mut o = 8usize;
        let data_offset = if flags & 0x1 != 0 {
            let v = i64::from(read_u32(body, o)? as i32);
            o += 4;
            v
        } else {
            0
        };
        if flags & 0x4 != 0 {
            let _ = read_u32(body, o)?; // first_sample_flags: no audio semantics
            o += 4;
        }
        if count == 0 {
            return Ok(next_off.unwrap_or(base));
        }
        let (mstart, mend) = fence.ok_or(AacError::Malformed(MalformedKind::Syntax))?;
        fence_entries(mem, frames.len() as u64 + count as u64, 16)?;
        frames.try_reserve(count).map_err(|_| {
            AacError::from(BudgetExceeded {
                kind: BudgetKind::Metadata,
                observed: (frames.len() as u64)
                    .saturating_add(count as u64)
                    .saturating_mul(16),
                max: mem.max_metadata_bytes,
            })
        })?;
        let mut off = if flags & 0x1 != 0 {
            i64::try_from(base)
                .ok()
                .and_then(|b| b.checked_add(data_offset))
                .and_then(|b| u64::try_from(b).ok())
                .ok_or(AacError::Malformed(MalformedKind::Syntax))?
        } else {
            next_off.unwrap_or(base)
        };
        let def_dur = tfhd.default_duration.unwrap_or(init.trex_duration);
        let def_size = tfhd.default_size.unwrap_or(init.trex_size);
        for _ in 0..count {
            let dur = if flags & 0x100 != 0 {
                let v = read_u32(body, o)?;
                o += 4;
                v
            } else {
                def_dur
            };
            let size = if flags & 0x200 != 0 {
                let v = read_u32(body, o)?;
                o += 4;
                v
            } else {
                def_size
            };
            if flags & 0x400 != 0 {
                let _ = read_u32(body, o)?; // sample_flags: no audio semantics
                o += 4;
            }
            if flags & 0x800 != 0 {
                let cto = read_u32(body, o)?;
                o += 4;
                if cto != 0 {
                    return unsupported("composition-time offsets are not supported");
                }
            }
            if dur == 0 && !tfhd.duration_is_empty {
                return Err(AacError::Malformed(MalformedKind::Syntax));
            }
            let end = off
                .checked_add(u64::from(size))
                .ok_or(AacError::Malformed(MalformedKind::Syntax))?;
            // Size 0 is still a sample: its offset has to sit in the
            // mdat. Skipping the fence let a push cursor run past the
            // payload and underflow the following skip.
            if off < mstart || end > mend {
                return Err(AacError::Malformed(MalformedKind::Syntax));
            }
            frames.push((off, size));
            #[cfg(test)]
            if self.trace_dts {
                self.sample_dts.push(self.dts);
            }
            off = end;
            self.dts = self
                .dts
                .checked_add(u64::from(dur))
                .ok_or(AacError::Malformed(MalformedKind::Syntax))?;
            self.total = self
                .total
                .checked_add(u64::from(dur))
                .ok_or(AacError::Malformed(MalformedKind::Syntax))?;
        }
        Ok(off)
    }

    /// Fold the resolved fragments into the shared track shape.
    pub(crate) fn finish(self, init: Fmp4Init) -> Result<AacTrack> {
        if self.frames.is_empty() {
            return Err(AacError::format("isomp4: fMP4 fragments carry no samples"));
        }
        Ok(AacTrack {
            asc: init.asc,
            total_samples: self.total,
            frames: self.frames,
            edit_start: init.edit_start,
            edit_duration: init.edit_duration,
            movie_timescale: init.movie_timescale,
            media_timescale: init.media_timescale,
            media_duration: self.dts,
            has_elst: init.has_elst,
            // A DASH init `elst` is written before the duration is known:
            // segment_duration 0 caps nothing (lab/fmp4/REPORT.md §4.3).
            edit_open_end: true,
        })
    }
}
