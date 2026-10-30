//! Bounded fragmented-MP4 (fMP4) parsing (TASK-124, decision-25; contract
//! and measured semantics: lab/fmp4/REPORT.md §2). One unencrypted AAC
//! track: an init `moov` (empty sample table) with `mvex`/`trex` defaults,
//! then each `moof` resolves its `trun` samples to absolute byte ranges
//! fenced inside the following `mdat` payload (see [`moof`]).
//!
//! tfhd base resolution: explicit `base_data_offset` →
//! `default_base_is_moof` → implicit (end of the previous fragment's data;
//! the first fragment falls back to its own moof position). trun v0/v1;
//! the per-sample defaults chain is trun field → tfhd default → `trex`
//! default. `tfdt` overrides the accumulated decode timestamp.
//!
//! Out-of-envelope shapes are typed, never silent: encryption (`enca`,
//! traf-level `saiz`/`saio`/`senc`), more than one usable audio track or a
//! foreign-track `traf`, nonzero composition-time offsets and
//! sample-description switches are [`AacError::Unsupported`]; mfhd sequence
//! gaps, zero resolved durations without `duration_is_empty`, unknown trun
//! versions and sample ranges outside the following `mdat` are
//! [`AacError::Malformed`].

use super::{
    AacTrack, BOX_MOOV, BoxHdr, BoxIter, convert_exact, fence_depth, fence_entries, parse_elst,
    parse_mdhd_timescale, parse_mvhd_timescale, parse_stsd_asc, read_box, read_top_box, read_u32,
};
use crate::budgets::MemoryBudgets;
use crate::error::{AacError, Result, UnsupportedFeature};

#[path = "isomp4_frag_moof.rs"]
mod moof;
pub(crate) use moof::FragResolver;

pub(super) fn unsupported<T>(reason: &'static str) -> Result<T> {
    Err(AacError::Unsupported(UnsupportedFeature::FragmentedMp4(
        reason,
    )))
}

impl Fmp4Init {
    /// Push-mode edit window (TASK-125): same rules as
    /// [`AacTrack::edit_window`] with `edit_open_end` always on (a DASH init
    /// declares priming before the media duration is known).
    pub(crate) fn edit_window(&self, sample_rate: u32) -> Result<(usize, Option<usize>)> {
        if !self.has_elst {
            return Ok((0, None));
        }
        let skip = convert_exact(
            self.edit_start,
            sample_rate,
            self.media_timescale,
            "elst media_time",
        )?;
        if self.edit_duration == 0 {
            return Ok((skip, None));
        }
        let play = convert_exact(
            self.edit_duration,
            sample_rate,
            self.movie_timescale,
            "elst duration",
        )?;
        Ok((skip, Some(play)))
    }
}

/// Init-segment facts the fragments need: ASC, track id, edit, trex defaults.
#[derive(Debug)]
pub(crate) struct Fmp4Init {
    pub asc: Vec<u8>,
    pub track_id: u32,
    pub movie_timescale: u32,
    pub media_timescale: u32,
    pub edit_start: u64,
    pub edit_duration: u64,
    pub has_elst: bool,
    pub trex_duration: u32,
    pub trex_size: u32,
}

struct InitTrack {
    asc: Vec<u8>,
    track_id: u32,
    media_timescale: u32,
    edit_start: u64,
    edit_duration: u64,
    has_elst: bool,
}

/// fMP4 stsd: exactly one `mp4a` entry. A second entry is a
/// sample-description switch (typed Unsupported); `enca` is encrypted.
fn parse_stsd_single(body: &[u8]) -> Result<Vec<u8>> {
    if read_u32(body, 4)? > 1 {
        return unsupported("a sample-description switch (stsd entry count > 1) is not supported");
    }
    if let Some((_, etyp)) = read_box(body, 8)?
        && &etyp == b"enca"
    {
        return unsupported("encrypted media (enca) is not supported");
    }
    parse_stsd_asc(body)
}

/// One init `trak`: `Some` only for a usable sound track (AAC `mp4a` +
/// `tkhd`); non-sound and broken tracks are skipped like the flat path.
fn parse_init_trak(moov: &[u8], trak: BoxHdr, mem: &MemoryBudgets) -> Result<Option<InitTrack>> {
    let mut track_id = None;
    let mut edit = None;
    let mut sound = false;
    let mut timescale = 0u32;
    let mut asc = None;
    for child in BoxIter::new(moov, trak.content_start, trak.content_end) {
        let (hdr, typ) = child?;
        let body = &moov[hdr.content_start..hdr.content_end];
        match &typ {
            b"tkhd" => {
                let v = *body
                    .first()
                    .ok_or_else(|| AacError::format("isomp4: truncated tkhd"))?;
                let off = match v {
                    0 => 12,
                    1 => 20,
                    _ => return Ok(None),
                };
                track_id = Some(read_u32(body, off)?);
            }
            b"edts" => {
                fence_depth(mem, 3)?;
                for ed in BoxIter::new(moov, hdr.content_start, hdr.content_end) {
                    let (ehdr, etyp) = ed?;
                    if &etyp == b"elst" {
                        fence_depth(mem, 4)?;
                        if let Some(e) =
                            parse_elst(&moov[ehdr.content_start..ehdr.content_end], mem)?
                        {
                            edit = Some(e);
                        }
                    }
                }
            }
            b"mdia" => {
                fence_depth(mem, 3)?;
                for sub in BoxIter::new(moov, hdr.content_start, hdr.content_end) {
                    let (shdr, styp) = sub?;
                    match &styp {
                        b"hdlr" => {
                            let handler = moov
                                .get(shdr.content_start + 8..shdr.content_start + 12)
                                .ok_or_else(|| AacError::format("isomp4: truncated hdlr"))?;
                            sound = handler == b"soun";
                        }
                        b"mdhd" => {
                            timescale =
                                parse_mdhd_timescale(&moov[shdr.content_start..shdr.content_end])?;
                        }
                        b"minf" => {
                            fence_depth(mem, 4)?;
                            asc = parse_stbl_stsd(moov, shdr, mem)?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !sound {
        return Ok(None);
    }
    let (asc, track_id) = match (asc, track_id) {
        (Some(a), Some(id)) => (a, id),
        _ => return Ok(None),
    };
    let (edit_start, edit_duration) = edit.unwrap_or((0, 0));
    Ok(Some(InitTrack {
        asc,
        track_id,
        media_timescale: timescale,
        edit_start,
        edit_duration,
        has_elst: edit.is_some(),
    }))
}

fn parse_stbl_stsd(moov: &[u8], minf: BoxHdr, mem: &MemoryBudgets) -> Result<Option<Vec<u8>>> {
    for m in BoxIter::new(moov, minf.content_start, minf.content_end) {
        let (mhdr, mtyp) = m?;
        if &mtyp != b"stbl" {
            continue;
        }
        fence_depth(mem, 5)?;
        for s in BoxIter::new(moov, mhdr.content_start, mhdr.content_end) {
            let (shdr, styp) = s?;
            if &styp != b"stsd" {
                continue;
            }
            let body = &moov[shdr.content_start..shdr.content_end];
            match parse_stsd_single(body) {
                Ok(a) => return Ok(Some(a)),
                // Typed rejections (enca, description switch) and budget
                // failures stay fatal; a broken description just makes the
                // track unusable (flat path parity).
                Err(e @ (AacError::Unsupported(_) | AacError::Limit { .. })) => return Err(e),
                Err(_) => return Ok(None),
            }
        }
    }
    Ok(None)
}

/// Parse the init `moov` (full box, header included) of a bounded fMP4
/// stream: first usable AAC sound track, `mvhd`/`mdhd` timescales, the
/// supported one-entry `elst`, and the matching `trex` defaults.
pub(crate) fn parse_init(moov: &[u8], mem: &MemoryBudgets) -> Result<Fmp4Init> {
    fence_depth(mem, 0)?;
    let (hdr, typ) =
        read_box(moov, 0)?.ok_or_else(|| AacError::format("isomp4: truncated init moov"))?;
    if typ != BOX_MOOV {
        return Err(AacError::format(
            "isomp4: fMP4 init does not start with moov",
        ));
    }
    fence_depth(mem, 1)?;
    let mut movie_timescale = 0u32;
    let mut trex: Vec<(u32, u32, u32)> = Vec::new();
    let mut track: Option<InitTrack> = None;
    for child in BoxIter::new(moov, hdr.content_start, hdr.content_end) {
        let (chdr, ctyp) = child?;
        match &ctyp {
            b"mvhd" => {
                movie_timescale =
                    parse_mvhd_timescale(&moov[chdr.content_start..chdr.content_end])?;
            }
            b"mvex" => {
                fence_depth(mem, 2)?;
                for t in BoxIter::new(moov, chdr.content_start, chdr.content_end) {
                    let (thdr, ttyp) = t?;
                    if &ttyp == b"trex" {
                        let tb = &moov[thdr.content_start..thdr.content_end];
                        fence_entries(mem, (trex.len() + 1) as u64, 12)?;
                        trex.push((read_u32(tb, 4)?, read_u32(tb, 12)?, read_u32(tb, 16)?));
                    }
                }
            }
            b"trak" => {
                fence_depth(mem, 2)?;
                if let Some(t) = parse_init_trak(moov, chdr, mem)? {
                    if track.is_some() {
                        return unsupported("more than one audio track is not supported");
                    }
                    track = Some(t);
                }
            }
            _ => {}
        }
    }
    let track = track.ok_or_else(|| AacError::format("isomp4: no AAC audio track found"))?;
    let (trex_duration, trex_size) = trex
        .iter()
        .find(|t| t.0 == track.track_id)
        .map(|t| (t.1, t.2))
        .unwrap_or((0, 0));
    Ok(Fmp4Init {
        asc: track.asc,
        track_id: track.track_id,
        movie_timescale,
        media_timescale: track.media_timescale,
        edit_start: track.edit_start,
        edit_duration: track.edit_duration,
        has_elst: track.has_elst,
        trex_duration,
        trex_size,
    })
}

/// True when `data` is fMP4-shaped: a top-level `moof`, or an init `moov`
/// carrying `mvex` (also when only the moov prefix is visible, so a
/// truncated fMP4 init keeps its typed error instead of the flat path's
/// `NotAac` collapse). A walk error before any fMP4 signal is "not ours".
pub(crate) fn is_fragmented(data: &[u8]) -> bool {
    let mut pos = 0usize;
    loop {
        match read_box(data, pos) {
            Ok(Some((hdr, typ))) => {
                if &typ == b"moof" {
                    return true;
                }
                if &typ == b"moov" && moov_has_mvex(data, hdr.content_start, hdr.content_end) {
                    return true;
                }
                pos = hdr.content_end;
            }
            Ok(None) => return false,
            Err(_) => {
                return data.get(pos + 4..pos + 8) == Some(b"moov".as_slice())
                    && moov_has_mvex(data, pos + 8, data.len());
            }
        }
    }
}

/// Tolerant scan of a `moov`'s immediate children for `mvex`; `end` may
/// run past the buffer (truncated init) — the scan stops at the first
/// unparseable child, but a truncated tail child's header alone still
/// proves `mvex`.
fn moov_has_mvex(data: &[u8], start: usize, end: usize) -> bool {
    let mut pos = start;
    loop {
        match read_box(data, pos) {
            Ok(Some((hdr, typ))) if hdr.content_end <= end => {
                if &typ == b"mvex" {
                    return true;
                }
                pos = hdr.content_end;
            }
            _ => {
                return matches!(
                    data.get(pos..pos + 8),
                    Some(h)
                        if pos + 8 <= end
                            && &h[4..8] == b"mvex"
                            && u32::from_be_bytes([h[0], h[1], h[2], h[3]]) >= 8
                );
            }
        }
    }
}

/// True when a complete init `moov` (full box, header included) carries an
/// `mvex` — the push path's fragmented-vs-flat gate: a flat moov keeps
/// [`UnsupportedFeature::M4aPush`] (TASK-125).
pub(crate) fn moov_is_fragmented(moov: &[u8]) -> bool {
    let Ok(Some((hdr, typ))) = read_box(moov, 0) else {
        return false;
    };
    typ == BOX_MOOV && moov_has_mvex(moov, hdr.content_start, hdr.content_end)
}

/// Whole-slice fMP4: init `moov` + every `moof` resolved against the
/// absolute `mdat` payload ranges of the same buffer.
pub(crate) fn parse_fmp4_track_with(data: &[u8], mem: &MemoryBudgets) -> Result<AacTrack> {
    fence_depth(mem, 0)?;
    let mut moov: Option<(usize, BoxHdr)> = None;
    let mut moofs: Vec<(u64, u64, BoxHdr)> = Vec::new();
    let mut mdats: Vec<(u64, u64)> = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let (hdr, typ) = read_top_box(data, pos)?;
        match &typ {
            b"moov" => {
                if moov.is_none() {
                    moov = Some((pos, hdr));
                }
            }
            b"moof" => moofs.push((pos as u64, hdr.content_end as u64, hdr)),
            b"mdat" => mdats.push((hdr.content_start as u64, hdr.content_end as u64)),
            _ => {}
        }
        pos = hdr.content_end;
    }
    let (moov_pos, moov_hdr) =
        moov.ok_or_else(|| AacError::format("isomp4: fMP4 without an init moov"))?;
    if moofs.is_empty() {
        return Err(AacError::format("isomp4: fMP4 init without fragments"));
    }
    let init = parse_init(&data[moov_pos..moov_hdr.content_end], mem)?;
    let mut res = FragResolver::default();
    for (mpos, mend, hdr) in &moofs {
        res.add_moof(
            &data[hdr.content_start..hdr.content_end],
            *mpos,
            *mend,
            &init,
            &mdats,
            mem,
        )?;
    }
    res.finish(init)
}

/// Seekable-reader fMP4: same resolution over boxes already loaded by
/// `isomp4_io` (`moov` with header; `moofs` as `(pos, end, content)`).
pub(crate) fn track_from_parts(
    moov: &[u8],
    moofs: &[(u64, u64, Vec<u8>)],
    mdats: &[(u64, u64)],
    mem: &MemoryBudgets,
) -> Result<AacTrack> {
    let init = parse_init(moov, mem)?;
    let mut res = FragResolver::default();
    for (pos, end, content) in moofs {
        res.add_moof(content, *pos, *end, &init, mdats, mem)?;
    }
    res.finish(init)
}

#[cfg(test)]
#[path = "isomp4_frag_tests.rs"]
mod isomp4_frag_tests;
#[cfg(test)]
#[path = "isomp4_frag_timeline_tests.rs"]
mod isomp4_frag_timeline_tests;
