//! HE-AAC v2 encoder (TASK-93): stereo in, one mono HE v1 core plus a
//! parametric-stereo payload per access unit.
//!
//! [`PsAnalysis`] turns each 2048-sample stereo block into a mono block
//! and one parameter frame; the mono goes through the unchanged
//! [`HeEncoder`] and the frame rides in the SBR `bs_extended_data` block
//! of the access unit whose decoder-side PS frame it covers (frame `k` →
//! unit `k + 1`, see `PS_FRAME_LEAD`). The downmix adds no delay, so the
//! timeline (priming, remainder, one unit per 2048 samples) is HE v1's.

use super::enc_he::{HeEncoder, HeInfo};
use super::enc_ps_est::{PS_BLOCK, PsAnalysis};
use super::error::{Error, Result};

pub(crate) struct PsHeEncoder {
    he: HeEncoder,
    ana: PsAnalysis,
    pend: [Vec<f32>; 2],
}

impl PsHeEncoder {
    /// `bitrate_bps` is whole-stream (mono core + SBR + PS).
    /// `fine_iid` selects Table 8.26 / `iid_mode` 4; `false` is mode 1.
    pub(crate) fn new(
        out_rate: u32,
        bitrate_bps: u32,
        lookahead: bool,
        fine_iid: bool,
    ) -> Result<Self> {
        let mut he = HeEncoder::new(out_rate, 1, bitrate_bps, lookahead)?;
        he.enable_ps(fine_iid)?;
        let mut ana = PsAnalysis::new();
        ana.set_iid_fine(fine_iid);
        Ok(Self {
            he,
            ana,
            pend: [Vec::with_capacity(PS_BLOCK), Vec::with_capacity(PS_BLOCK)],
        })
    }

    pub(crate) fn ps_side_info(&self) -> (u64, u64) {
        self.he.ps_side_info()
    }

    pub(crate) fn fs_index(&self) -> u8 {
        self.he.fs_index()
    }

    pub(crate) fn reset(&mut self) {
        self.he.reset();
        self.ana.reset();
        self.pend.iter_mut().for_each(Vec::clear);
    }

    /// Analyse the pending block (zero-padded) and push `keep` mono samples.
    fn block<F>(&mut self, keep: usize, on_au: &mut F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let mut l = [0.0f32; PS_BLOCK];
        let mut r = [0.0f32; PS_BLOCK];
        l[..self.pend[0].len()].copy_from_slice(&self.pend[0]);
        r[..self.pend[1].len()].copy_from_slice(&self.pend[1]);
        self.pend.iter_mut().for_each(Vec::clear);
        let mut mono = [0.0f32; PS_BLOCK];
        if let Some(p) = self.ana.block(&l, &r, &mut mono)? {
            self.he.push_ps(p);
        }
        self.he.push(&[&mono[..keep]], on_au)
    }

    pub(crate) fn push<F>(&mut self, planes: &[&[f32]], mut on_au: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if planes.len() != 2 || planes[0].len() != planes[1].len() {
            return Err(Error::Format("HE v2 encoder: two equal planes"));
        }
        let mut off = 0usize;
        while off < planes[0].len() {
            let take = (PS_BLOCK - self.pend[0].len()).min(planes[0].len() - off);
            self.pend[0].extend_from_slice(&planes[0][off..off + take]);
            self.pend[1].extend_from_slice(&planes[1][off..off + take]);
            off += take;
            if self.pend[0].len() == PS_BLOCK {
                self.block(PS_BLOCK, &mut on_au)?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish<F>(&mut self, mut on_au: F) -> Result<HeInfo>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let tail = self.pend[0].len();
        if tail > 0 {
            self.block(tail, &mut on_au)?;
        }
        self.he.finish(on_au)
    }
}

/// HE v1 or HE v2 behind one push / finish surface.
pub(crate) enum AnyHe {
    V1(Box<HeEncoder>),
    V2(Box<PsHeEncoder>),
}

impl AnyHe {
    pub(crate) fn fs_index(&self) -> u8 {
        match self {
            AnyHe::V1(e) => e.fs_index(),
            AnyHe::V2(e) => e.fs_index(),
        }
    }

    /// Core channels = the ADTS `channel_configuration` (v2: mono).
    pub(crate) fn core_channels(&self, planes: usize) -> usize {
        match self {
            AnyHe::V1(_) => planes,
            AnyHe::V2(_) => 1,
        }
    }

    pub(crate) fn reset(&mut self) {
        match self {
            AnyHe::V1(e) => e.reset(),
            AnyHe::V2(e) => e.reset(),
        }
    }

    pub(crate) fn push<F>(&mut self, planes: &[&[f32]], on_au: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        match self {
            AnyHe::V1(e) => e.push(planes, on_au),
            AnyHe::V2(e) => e.push(planes, on_au),
        }
    }

    pub(crate) fn finish<F>(&mut self, on_au: F) -> Result<HeInfo>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        match self {
            AnyHe::V1(e) => e.finish(on_au),
            AnyHe::V2(e) => e.finish(on_au),
        }
    }

    pub(crate) fn ps_side_info(&self) -> (u64, u64) {
        match self {
            AnyHe::V1(_) => (0, 0),
            AnyHe::V2(e) => e.ps_side_info(),
        }
    }
}

#[cfg(test)]
#[path = "enc_he_ps_tests.rs"]
mod enc_he_ps_tests;
