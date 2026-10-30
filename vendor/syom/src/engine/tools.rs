//! Per-channel Main / LTP / SSR state for [`super::decode::StreamDecoder`].
//!
//! The fast mono/stereo path keeps two sides, matching `fb_l` / `fb_r`.
//! Multichannel frames swap those sides with a pool keyed by element
//! identity, the same way the filterbank pool does. LC frames never touch
//! the banks, so the delay lines stay unallocated.

use super::channel_map::ElemKind;
use super::error::Result;
use super::ics::IcsInfo;
use super::ltp::LtpChan;
use super::pred::PredChan;
use super::ssr::{GainControl, SsrChan};

#[derive(Clone, Debug, Default)]
struct Side {
    pred: PredChan,
    ltp: LtpChan,
    ssr: SsrChan,
}

#[derive(Clone, Debug)]
struct Slot {
    kind: ElemKind,
    tag: u8,
    part: u8,
    seen: bool,
    side: Side,
}

/// Profile tools carried across frames.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tools {
    pub aot: u8,
    pub multichannel: bool,
    left: Side,
    right: Side,
    pool: Vec<Slot>,
}

impl Tools {
    pub(crate) fn reset(&mut self) {
        self.aot = 2;
        self.multichannel = false;
        self.left = Side::default();
        self.right = Side::default();
        self.pool.clear();
    }

    pub(crate) fn begin_frame(&mut self) {
        for slot in &mut self.pool {
            slot.seen = false;
        }
    }

    pub(crate) fn retain_seen(&mut self) {
        if self.multichannel {
            self.pool.retain(|s| s.seen);
        }
    }

    pub(crate) fn swap(&mut self, kind: ElemKind, tag: u8, part: u8) {
        let i = self.ensure(kind, tag, part);
        std::mem::swap(&mut self.left, &mut self.pool[i].side);
    }

    pub(crate) fn swap_pair(&mut self, tag: u8) {
        let i0 = self.ensure(ElemKind::Cpe, tag, 0);
        let i1 = self.ensure(ElemKind::Cpe, tag, 1);
        std::mem::swap(&mut self.left, &mut self.pool[i0].side);
        std::mem::swap(&mut self.right, &mut self.pool[i1].side);
    }

    fn ensure(&mut self, kind: ElemKind, tag: u8, part: u8) -> usize {
        if let Some(i) = self
            .pool
            .iter()
            .position(|s| s.kind == kind && s.tag == tag && s.part == part)
        {
            self.pool[i].seen = true;
            return i;
        }
        self.pool.push(Slot {
            kind,
            tag,
            part,
            seen: true,
            side: Side::default(),
        });
        self.pool.len() - 1
    }

    pub(crate) fn apply_main(
        &mut self,
        spec: &mut [f32],
        ics: &IcsInfo,
        fs: u8,
        right: bool,
    ) -> Result<()> {
        if self.aot != 1 {
            return Ok(());
        }
        let side = if right {
            &mut self.right
        } else {
            &mut self.left
        };
        side.pred.apply(spec, ics, fs)
    }

    pub(crate) fn apply_ltp(
        &mut self,
        spec: &mut [f32],
        ics: &IcsInfo,
        fs: u8,
        tns: Option<&super::tns::TnsData>,
        right: bool,
    ) -> Result<()> {
        if self.aot != 4 {
            return Ok(());
        }
        let side = if right {
            &mut self.right
        } else {
            &mut self.left
        };
        side.ltp.apply(spec, ics, fs, tns)
    }

    pub(crate) fn update_ltp(
        &mut self,
        pcm: &[f32],
        overlap: &[f32],
        shape: super::ics::WindowShape,
        right: bool,
    ) {
        if self.aot != 4 {
            return;
        }
        let side = if right {
            &mut self.right
        } else {
            &mut self.left
        };
        side.ltp.update(pcm, overlap, shape);
    }

    pub(crate) fn synthesize_ssr(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        gain: Option<&GainControl>,
        dst: &mut Vec<f32>,
        right: bool,
    ) -> Result<()> {
        let side = if right {
            &mut self.right
        } else {
            &mut self.left
        };
        side.ssr.synthesize(spec, ics, gain, dst)
    }
}
