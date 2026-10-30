//! Per-element filterbank overlap, keyed by `(kind, tag)`, not encounter index.
//!
//! Identities present in the frame keep history across reordering. Identities
//! absent from a frame are dropped: a later tag reused as a different kind,
//! or a mono/stereo/layout change, starts from a cold filterbank. Duplicate
//! `(kind, tag)` in one access unit is `Error::Format`.

use super::channel_map::{ElemKind, Element};
use super::error::{Error, Result};
use super::filterbank::Filterbank;

#[derive(Debug)]
struct Slot {
    kind: ElemKind,
    tag: u8,
    part: u8,
    seen: bool,
    fb: Filterbank,
}

#[derive(Debug, Default)]
pub(crate) struct FbPool {
    slots: Vec<Slot>,
}

impl FbPool {
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
    pub(crate) fn begin_frame(&mut self) {
        for s in &mut self.slots {
            s.seen = false;
        }
    }

    /// Keep overlap only for identities decoded in this frame.
    pub(crate) fn retain_seen(&mut self) {
        self.slots.retain(|s| s.seen);
    }

    /// Drop identities and overlap; keep `Vec` capacity for the next session.
    pub(crate) fn reset(&mut self) {
        self.slots.clear();
    }

    pub(crate) fn swap_in(&mut self, kind: ElemKind, tag: u8, fb: &mut Filterbank) {
        std::mem::swap(fb, self.slot(kind, tag, 0));
    }

    pub(crate) fn swap_pair(&mut self, tag: u8, left: &mut Filterbank, right: &mut Filterbank) {
        std::mem::swap(left, self.slot(ElemKind::Cpe, tag, 0));
        std::mem::swap(right, self.slot(ElemKind::Cpe, tag, 1));
    }

    fn slot(&mut self, kind: ElemKind, tag: u8, part: u8) -> &mut Filterbank {
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.kind == kind && s.tag == tag && s.part == part)
        {
            self.slots[i].seen = true;
            return &mut self.slots[i].fb;
        }
        self.slots.push(Slot {
            kind,
            tag,
            part,
            seen: true,
            fb: Filterbank::new(),
        });
        let i = self.slots.len() - 1;
        &mut self.slots[i].fb
    }
}

pub(crate) fn reject_dup(elems: &[Element], kind: ElemKind, tag: u8) -> Result<()> {
    if elems.iter().any(|e| e.kind == kind && e.tag == tag) {
        Err(Error::Format("duplicate channel element identity"))
    } else {
        Ok(())
    }
}
