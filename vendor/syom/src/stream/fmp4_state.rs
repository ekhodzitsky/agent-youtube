//! Push-fMP4 state ([`Fmp4Push`], [`Phase`]) and the incremental box-header
//! read for the pump in [`super::fmp4`](super).

use crate::budgets::MemoryBudgets;
use crate::error::{AacError, MalformedKind, Result, UnsupportedFeature};
use crate::isomp4::frag::{Fmp4Init, FragResolver};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Phase {
    /// Before the init `moov`.
    #[default]
    Init,
    /// Between fragments.
    Boxes,
    /// The `moof` at absolute `[start, end)` is buffered; waiting for the
    /// following `mdat` header. `next` is the absolute walk cursor after
    /// the moof (skippable boxes may intervene).
    Moof { start: u64, end: u64, next: u64 },
    /// Delivering the current fragment's resolved samples.
    Mdat,
    /// Skipping a box payload to its absolute end (mdat tail, `mfra`, …).
    Skip(u64),
    /// Presentation length reached: drain the unplayed tail undecoded.
    Drain,
}

/// `Container::Fmp4` state on [`Decoder`].
#[derive(Default)]
pub(crate) struct Fmp4Push {
    pub(super) phase: Phase,
    pub(super) init: Option<Fmp4Init>,
    pub(super) res: FragResolver,
    /// ASC scalars for the raw-AU decode calls.
    pub(super) asc: Option<(u8, u8, u32, u8)>,
    /// Current fragment's resolved `(absolute offset, size)` sample ranges.
    pub(super) frag: Vec<(u64, u32)>,
    pub(super) frag_idx: usize,
    /// Absolute end of the current `mdat` payload.
    pub(super) mdat_end: u64,
    pub(super) skip_left: usize,
    pub(super) skip0: usize,
    pub(super) play_left: Option<usize>,
}

impl Fmp4Push {
    pub(crate) fn reset(&mut self) {
        self.phase = Phase::Init;
        self.init = None;
        self.res.reset();
        self.asc = None;
        self.frag.clear();
        self.frag_idx = 0;
        self.mdat_end = 0;
        self.skip_left = 0;
        self.skip0 = 0;
        self.play_left = None;
    }
}

/// Box header at `pos`: `(declared size, header len, type)`; `None` = wait
/// for more bytes. Size 0 and size-below-header are typed errors.
pub(super) fn peek_box(buf: &[u8], pos: usize) -> Result<Option<(u64, u64, [u8; 4])>> {
    let Some(hdr) = buf.get(pos..pos + 8) else {
        return Ok(None);
    };
    let size32 = u64::from(u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]));
    let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (size, head) = match size32 {
        1 => {
            let Some(ext) = buf.get(pos + 8..pos + 16) else {
                return Ok(None);
            };
            let arr: [u8; 8] = ext
                .try_into()
                .map_err(|_| AacError::Malformed(MalformedKind::Syntax))?;
            (u64::from_be_bytes(arr), 16)
        }
        0 => {
            return Err(AacError::Unsupported(UnsupportedFeature::FragmentedMp4(
                "a box without a declared size is not supported on push",
            )));
        }
        s => (s, 8),
    };
    if size < head {
        return Err(AacError::Malformed(MalformedKind::Syntax));
    }
    Ok(Some((size, head, typ)))
}

/// A buffered-whole box (`moov` / `moof`) is metadata, fenced like the flat
/// sample tables.
pub(super) fn fence_box(mem: &MemoryBudgets, size: u64) -> Result<()> {
    mem.check_index(1, size).map(|_| ()).map_err(AacError::from)
}

/// Absolute end of a box. A wrapping size is a typed error, not a panic
/// and not a skip to a wrapped offset.
pub(super) fn checked_end(start: u64, size: u64) -> Result<u64> {
    start
        .checked_add(size)
        .ok_or(AacError::Malformed(MalformedKind::Syntax))
}
