//! Per-frame allocation inputs for the long-window rate loop (TASK-113):
//! the psy model runs once on the pre-TNS L/R spectra, then the cache is
//! finished on the coded (post-TNS, post-M/S) spectra so every `build`
//! offset only re-derives targets. M/S bands take the smaller of the L
//! and R allowed noise (the de-matrixed noise lands in both channels);
//! a TNS channel scales allowed noise by its residual/original energy
//! ratio so the noise-to-mask ratio holds after the decoder's synthesis
//! filter restores the envelope.

use super::{LcEncoder, TARGET_Q};
use crate::engine::enc_quant::{self, MAX_BANDS};
use crate::engine::swb::LONG_WINDOW_LEN;

impl LcEncoder {
    /// Psy pass on the pre-TNS L/R spectra: coded masks, caps, allowed noise.
    pub(super) fn prepare_alloc(&mut self, specs: &[[f32; LONG_WINDOW_LEN]; 2]) {
        for (ch, spec) in specs.iter().enumerate().take(self.channels) {
            let a = &mut self.alloc[ch];
            self.psy
                .analyze_mask(spec, self.offsets, TARGET_Q, &mut a.coded, &mut a.cap);
            let (energy, noise, _) = self.psy.bands();
            a.noise = *noise;
            a.energy = *energy;
        }
    }

    /// Finish the cache on the spectra the rate loop quantizes.
    pub(super) fn finish_alloc(
        &mut self,
        specs: &[[f32; LONG_WINDOW_LEN]; 2],
        psy_specs: &[[f32; LONG_WINDOW_LEN]; 2],
    ) {
        let n = self.offsets.len() - 1;
        let stereo = self.channels == 2;
        let mut noise_lr = [[0.0f32; MAX_BANDS]; 2];
        let mut coded_lr = [[false; MAX_BANDS]; 2];
        for ch in 0..self.channels {
            noise_lr[ch] = self.alloc[ch].noise;
            coded_lr[ch] = self.alloc[ch].coded;
        }
        for ch in 0..self.channels {
            let tns_on = self.tns[ch].band_range().is_some();
            let a = &mut self.alloc[ch];
            enc_quant::band_peaks(&specs[ch], self.offsets, &mut a.peaks);
            for (b, w) in self.offsets.windows(2).enumerate().take(n) {
                let (lo, hi) = (usize::from(w[0]), usize::from(w[1]));
                let res = &specs[ch][lo..hi];
                a.width[b] = (hi - lo) as f32;
                a.energy[b] = res.iter().map(|&x| x * x).sum();
                a.sum_sqrt[b] = res.iter().map(|&x| x.abs().sqrt()).sum();
                if stereo && self.ms.used_at(b) {
                    a.noise[b] = noise_lr[0][b].min(noise_lr[1][b]);
                    a.coded[b] = coded_lr[0][b] || coded_lr[1][b];
                    a.cap[b] = TARGET_Q;
                }
                if tns_on {
                    let pre: f32 = psy_specs[ch][lo..hi].iter().map(|&x| x * x).sum();
                    if pre > 0.0 && a.energy[b] < pre {
                        a.noise[b] *= a.energy[b] / pre;
                    }
                }
            }
        }
    }
}
