//! Frame finish: LTP add, TNS, LC or SSR synthesis, then channel routing.

use super::cce::{apply_cce_to_pending, apply_independent_pcm};
use super::channel_map::ElemKind;
use super::decode::StreamDecoder;
use super::error::{Error, Result};
use super::ics::IcsInfo;
use super::pns;
use super::tns;

impl StreamDecoder {
    fn apply_ltp_pending(&mut self, fs: u8, multichannel: bool) -> Result<()> {
        let n = self.pending.len();
        let mut i = 0usize;
        while i < n {
            let cpe =
                self.pending[i].kind == ElemKind::Cpe && self.pending[i].part == 0 && i + 1 < n;
            if cpe {
                let tag = self.pending[i].tag;
                if multichannel {
                    self.tools.swap_pair(tag);
                }
                for (slot, right) in [(i, false), (i + 1, true)] {
                    let mut spec = std::mem::take(&mut self.pending[slot].spec);
                    let ics = self.pending[slot].ics;
                    let tns = self.pending[slot].tns;
                    self.tools
                        .apply_ltp(&mut spec, &ics, fs, tns.as_ref(), right)?;
                    self.pending[slot].spec = spec;
                }
                if multichannel {
                    self.tools.swap_pair(tag);
                }
                i += 2;
            } else {
                let kind = self.pending[i].kind;
                let tag = self.pending[i].tag;
                if multichannel {
                    self.tools.swap(kind, tag, 0);
                }
                let mut spec = std::mem::take(&mut self.pending[i].spec);
                let ics = self.pending[i].ics;
                let tns = self.pending[i].tns;
                self.tools
                    .apply_ltp(&mut spec, &ics, fs, tns.as_ref(), false)?;
                self.pending[i].spec = spec;
                if multichannel {
                    self.tools.swap(kind, tag, 0);
                }
                i += 1;
            }
        }
        Ok(())
    }

    fn synth_one(
        &mut self,
        spec: &[f32],
        ics: &IcsInfo,
        gain: Option<&super::ssr::GainControl>,
        right: bool,
    ) -> Result<()> {
        if self.tools.aot == super::asc::AOT_SSR {
            return if right {
                self.tools
                    .synthesize_ssr(spec, ics, gain, &mut self.pcm_r, true)
            } else {
                self.tools
                    .synthesize_ssr(spec, ics, gain, &mut self.pcm_l, false)
            };
        }
        if right {
            self.fb_r.synthesize_into(spec, ics, &mut self.pcm_r)?;
        } else {
            self.fb_l.synthesize_into(spec, ics, &mut self.pcm_l)?;
        }
        if self.tools.aot == super::asc::AOT_LTP {
            let ov = if right {
                self.fb_r.saved_overlap().to_vec()
            } else {
                self.fb_l.saved_overlap().to_vec()
            };
            let shape = ics.window_shape;
            let pcm = if right {
                self.pcm_r.clone()
            } else {
                self.pcm_l.clone()
            };
            self.tools.update_ltp(&pcm, &ov, shape, right);
        }
        Ok(())
    }

    pub(crate) fn finish_pending(
        &mut self,
        fs_index: u8,
        multichannel: bool,
        skip_downmix: bool,
    ) -> Result<()> {
        for cce in &mut self.cces {
            pns::apply(
                &mut cce.spec,
                &cce.ics,
                &cce.sections,
                &cce.sf,
                fs_index,
                &mut self.rng,
                None,
            )?;
        }
        for i in 0..self.cces.len() {
            if !self.cces[i].independent && self.cces[i].domain == 0 {
                apply_cce_to_pending(&mut self.pending, &self.cces[i], fs_index)?;
            }
        }
        if self.tools.aot == super::asc::AOT_LTP {
            self.apply_ltp_pending(fs_index, multichannel)?;
        }
        for p in &mut self.pending {
            if let Some(t) = p.tns.as_ref() {
                tns::apply(&mut p.spec, t, &p.ics, fs_index)?;
            }
        }
        for cce in &mut self.cces {
            if let Some(t) = cce.tns {
                tns::apply(&mut cce.spec, &t, &cce.ics, fs_index)?;
            }
        }
        for i in 0..self.cces.len() {
            if !self.cces[i].independent && self.cces[i].domain == 1 {
                apply_cce_to_pending(&mut self.pending, &self.cces[i], fs_index)?;
            }
        }
        let mut pending = std::mem::take(&mut self.pending);
        self.n_ch = 0;
        let mut ids = [(ElemKind::Sce, 0u8, 0u8); 8];
        let mut n_ids = 0usize;
        let mut i = 0usize;
        while i < pending.len() {
            if pending[i].kind == ElemKind::Cpe && pending[i].part == 0 {
                if i + 1 >= pending.len() {
                    self.pending = pending;
                    return Err(Error::Format("CPE missing right"));
                }
                if multichannel {
                    self.fb_pool
                        .swap_pair(pending[i].tag, &mut self.fb_l, &mut self.fb_r);
                    self.tools.swap_pair(pending[i].tag);
                }
                self.synth_one(
                    &pending[i].spec,
                    &pending[i].ics,
                    pending[i].gain.as_deref(),
                    false,
                )?;
                self.synth_one(
                    &pending[i + 1].spec,
                    &pending[i + 1].ics,
                    pending[i + 1].gain.as_deref(),
                    true,
                )?;
                if self.fast_mono {
                    for (l, r) in self.pcm_l.iter_mut().zip(self.pcm_r.iter()) {
                        *l = 0.5 * (*l + *r);
                    }
                }
                if multichannel {
                    self.fb_pool
                        .swap_pair(pending[i].tag, &mut self.fb_l, &mut self.fb_r);
                    self.tools.swap_pair(pending[i].tag);
                }
                self.push_pcm(false);
                if n_ids < 8 {
                    ids[n_ids] = (pending[i].kind, pending[i].tag, 0);
                    n_ids += 1;
                }
                self.push_pcm(true);
                if n_ids < 8 {
                    ids[n_ids] = (pending[i + 1].kind, pending[i + 1].tag, 1);
                    n_ids += 1;
                }
                i += 2;
            } else {
                if multichannel {
                    self.fb_pool
                        .swap_in(pending[i].kind, pending[i].tag, &mut self.fb_l);
                    self.tools.swap(pending[i].kind, pending[i].tag, 0);
                }
                self.synth_one(
                    &pending[i].spec,
                    &pending[i].ics,
                    pending[i].gain.as_deref(),
                    false,
                )?;
                if multichannel {
                    self.fb_pool
                        .swap_in(pending[i].kind, pending[i].tag, &mut self.fb_l);
                    self.tools.swap(pending[i].kind, pending[i].tag, 0);
                }
                self.push_pcm(false);
                if n_ids < 8 {
                    ids[n_ids] = (pending[i].kind, pending[i].tag, pending[i].part);
                    n_ids += 1;
                }
                i += 1;
            }
        }
        if !self.fast_mono {
            let cces = std::mem::take(&mut self.cces);
            for cce in &cces {
                if !cce.independent {
                    continue;
                }
                self.fb_pool.swap_in(ElemKind::Cce, cce.tag, &mut self.fb_l);
                self.cce_pcm.clear();
                self.fb_l
                    .synthesize_into(&cce.spec, &cce.ics, &mut self.cce_pcm)?;
                self.fb_pool.swap_in(ElemKind::Cce, cce.tag, &mut self.fb_l);
                apply_independent_pcm(&mut self.frame_ch, &ids[..n_ids], &self.cce_pcm, cce)?;
            }
            self.cces = cces;
            if !skip_downmix && !multichannel && self.mix_down_mono && self.frame_ch.len() >= 2 {
                let n = self.frame_ch[0].len().min(self.frame_ch[1].len());
                for i in 0..n {
                    self.frame_ch[0][i] = 0.5 * (self.frame_ch[0][i] + self.frame_ch[1][i]);
                }
                self.frame_ch.truncate(1);
                self.pcm_l.clone_from(&self.frame_ch[0]);
                self.n_ch = 1;
            }
        }
        for p in &mut pending {
            let spec = std::mem::take(&mut p.spec);
            self.recycle_spec(spec);
        }
        pending.clear();
        self.pending = pending;
        self.refill_spec_bufs();
        Ok(())
    }
}
