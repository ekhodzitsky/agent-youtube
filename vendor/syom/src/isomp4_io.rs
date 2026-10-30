//! Top-level ISOBMFF walk over `Read + Seek`: load `moov` (and, for
//! fragmented MP4, every `moof`), skip `mdat` payloads.

use std::io::{self, Read, Seek, SeekFrom};

use crate::budgets::{BudgetKind, MemoryBudgets, check_planned};
use crate::error::{AacError, Result};

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    loop {
        match r.read_exact(buf) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(AacError::truncated_at(None));
            }
            Err(e) => return Err(AacError::from(e)),
        }
    }
}

fn seek<R: Seek>(r: &mut R, pos: SeekFrom) -> Result<u64> {
    r.seek(pos).map_err(AacError::from)
}

/// Top-level boxes the track parse needs, per container flavour.
pub(crate) enum TrackBoxes {
    /// Flat M4A: the `moov` box (header included) and the file size.
    Flat { moov: Vec<u8>, file_len: u64 },
    /// Bounded fMP4: `moov` plus every `moof` content with its absolute
    /// `(pos, end)` and the absolute `mdat` payload ranges.
    Frag {
        moov: Vec<u8>,
        moofs: Vec<(u64, u64, Vec<u8>)>,
        mdats: Vec<(u64, u64)>,
    },
}

/// Walk the top-level boxes without loading `mdat`. Flat files load only
/// `moov`; fMP4 additionally loads each `moof` (each and their sum fenced
/// by the metadata budget — same class as the flat whole-file index).
pub(crate) fn load_track_boxes<R: Read + Seek>(
    r: &mut R,
    mem: &MemoryBudgets,
) -> Result<TrackBoxes> {
    let file_len = seek(r, SeekFrom::End(0))?;
    seek(r, SeekFrom::Start(0))?;
    let mut pos = 0u64;
    let mut moov = None;
    let mut moofs: Vec<(u64, u64, Vec<u8>)> = Vec::new();
    let mut mdats: Vec<(u64, u64)> = Vec::new();
    let mut moof_bytes = 0u64;
    while pos.saturating_add(8) <= file_len {
        seek(r, SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        read_exact(r, &mut hdr)?;
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (box_size, header_len) = match size32 {
            1 => {
                let mut ext = [0u8; 8];
                read_exact(r, &mut ext)?;
                (u64::from_be_bytes(ext), 16u64)
            }
            0 => (file_len.saturating_sub(pos), 8u64),
            s => (s, 8u64),
        };
        if box_size < header_len {
            return Err(AacError::format(format!(
                "isomp4: box size {box_size} smaller than its header"
            )));
        }
        let end = pos
            .checked_add(box_size)
            .filter(|&e| e <= file_len)
            .ok_or_else(|| AacError::format("isomp4: box extends past end of input"))?;
        match &typ {
            b"moov" if moov.is_none() => {
                let n = check_planned(BudgetKind::Metadata, box_size, mem.max_metadata_bytes)
                    .map_err(AacError::from)?;
                seek(r, SeekFrom::Start(pos))?;
                let mut buf = vec![0u8; n];
                read_exact(r, &mut buf)?;
                moov = Some(buf);
            }
            b"moof" => {
                let content_len = box_size - header_len;
                moof_bytes = moof_bytes.saturating_add(content_len);
                check_planned(BudgetKind::Metadata, moof_bytes, mem.max_metadata_bytes)
                    .map_err(AacError::from)?;
                let n = usize::try_from(content_len)
                    .map_err(|_| AacError::format("isomp4: moof exceeds usize"))?;
                seek(r, SeekFrom::Start(pos + header_len))?;
                let mut buf = vec![0u8; n];
                read_exact(r, &mut buf)?;
                moofs.push((pos, end, buf));
            }
            b"mdat" => mdats.push((pos + header_len, end)),
            _ => {}
        }
        pos = end;
    }
    let moov = moov.ok_or_else(|| AacError::format("isomp4: no moov box"))?;
    if moofs.is_empty() {
        Ok(TrackBoxes::Flat { moov, file_len })
    } else {
        Ok(TrackBoxes::Frag { moov, moofs, mdats })
    }
}
