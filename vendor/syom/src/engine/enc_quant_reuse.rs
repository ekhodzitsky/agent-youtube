//! Per-frame long-band reuse keyed by the actual normalized scalefactor.
use super::*;
#[derive(Clone)]
pub(crate) struct BandReuse {
    valid: [bool; MAX_BANDS],
    sf: [i32; MAX_BANDS],
}
impl BandReuse {
    pub(crate) const fn new() -> Self {
        Self {
            valid: [false; MAX_BANDS],
            sf: [0; MAX_BANDS],
        }
    }
    /// Start a different immutable spectrum, even after reset or MC reuse.
    pub(crate) fn reset(&mut self) {
        self.valid.fill(false);
    }
    /// A caller changed quantized values or costs outside this walk.
    pub(crate) fn invalidate(&mut self, band: usize) {
        self.valid[band] = false;
    }
    pub(crate) fn quantize(&mut self, spec: &[f32; 1024], offsets: &[u16], out: &mut QuantChannel) {
        let n = out.n_bands.min(offsets.len() - 1);
        out.quant[..usize::from(offsets[0])].fill(0);
        out.quant[usize::from(offsets[n])..].fill(0);
        self.valid[n..].fill(false);
        for (b, w) in offsets.windows(2).enumerate().take(n) {
            let (lo, hi) = (usize::from(w[0]), usize::from(w[1]));
            if !out.coded[b] {
                out.quant[lo..hi].fill(0);
                out.zero[b] = false;
                out.bits[b] = [UNREPRESENTABLE; BOOKS];
                self.valid[b] = false;
                continue;
            }
            if self.valid[b] && self.sf[b] == out.sf[b] {
                continue;
            }
            bits::write_quants(
                &mut out.quant[lo..hi],
                &spec[lo..hi],
                &out.mag[lo..hi],
                quant_gain(out.sf[b]),
            );
            out.zero[b] = out.quant[lo..hi].iter().all(|&v| v == 0);
            fill_bits(&mut out.bits[b], &out.quant[lo..hi]);
            self.sf[b] = out.sf[b];
            self.valid[b] = true;
        }
    }
}
#[cfg(test)]
#[path = "enc_quant_reuse_tests.rs"]
mod tests;
