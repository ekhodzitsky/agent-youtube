//! In-place [`super::StreamDecoder`] reset: keep Vec capacity, drop signal.

use super::StreamDecoder;
use crate::engine::filterbank::Filterbank;
use crate::engine::pns::Lcg;
use crate::layout::FrameMeta;

impl StreamDecoder {
    /// Core samples per frame from the ASC (1024, 960, 512, or 480).
    pub(crate) fn set_frame_samples(&mut self, n: u16) {
        self.frame_samples = n;
    }

    /// Zero overlap, HE/PS/PCE, LCG, and counters. Spectral/PCM/element
    /// `Vec` capacity is kept. [`Self::mix_down_mono`] is unchanged.
    pub(crate) fn reset(&mut self) {
        self.fb_l = Filterbank::new();
        self.fb_r = Filterbank::new();
        self.rng = Lcg::new();
        self.sbr_pool.reset();
        self.pending_sbrs.clear();
        self.sbr_active = false;
        self.sbr_declared = false;
        self.ps_declared = false;
        self.sbr_out_rate = None;
        self.pcm_l.clear();
        self.pcm_r.clear();
        for p in &mut self.frame_ch {
            p.clear();
        }
        self.n_ch = 0;
        self.quant.clear();
        self.spec_l.clear();
        self.spec_r.clear();
        self.fast_mono = false;
        self.elems.clear();
        self.pce = None;
        self.fb_pool.reset();
        self.pending.clear();
        self.cces.clear();
        for s in &mut self.spec_pool {
            s.clear();
        }
        self.plane_order.clear();
        self.cce_pcm.clear();
        self.tools.reset();
        self.pns_shared.clear();
        self.last_rdb_bytes = 0;
        self.last_meta = FrameMeta::EMPTY;
        self.frame_samples = 0;
    }
}
