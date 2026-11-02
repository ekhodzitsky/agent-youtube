//! Per-frame short-window analysis, invariant across rate-loop offsets.
use super::*;

#[derive(Clone)]
pub(crate) struct ShortAlloc {
    pub(super) coded: [bool; MAX_FLAT_SHORT],
    pub(super) cap: [f32; MAX_FLAT_SHORT],
    pub(super) peaks: [f32; MAX_FLAT_SHORT],
    pub(super) energy: [f32; MAX_FLAT_SHORT],
    pub(super) noise: [f32; MAX_FLAT_SHORT],
    pub(super) sum_sqrt: [f32; MAX_FLAT_SHORT],
    pub(super) width: [f32; MAX_FLAT_SHORT],
    pub(super) forced: [bool; MAX_FLAT_SHORT],
    pub(super) grouping: Grouping,
}
impl ShortAlloc {
    pub(crate) fn new() -> Self {
        Self {
            coded: [false; MAX_FLAT_SHORT],
            cap: [0.0; MAX_FLAT_SHORT],
            peaks: [0.0; MAX_FLAT_SHORT],
            energy: [0.0; MAX_FLAT_SHORT],
            noise: [0.0; MAX_FLAT_SHORT],
            sum_sqrt: [0.0; MAX_FLAT_SHORT],
            width: [0.0; MAX_FLAT_SHORT],
            forced: [false; MAX_FLAT_SHORT],
            grouping: Grouping::ungrouped(),
        }
    }
    /// Fresh analysis of this frame's post-TNS spectrum, once per channel.
    /// Psy has no temporal state: each analysis overwrites its band outputs.
    pub(crate) fn prepare(
        &mut self,
        psy: &mut Psy,
        offsets: &[u16],
        spec: &[f32; LONG_WINDOW_LEN],
        n_sfb: usize,
        tns: &EncTns,
        grouping: Grouping,
    ) {
        let mut win_coded = [false; MAX_FLAT_SHORT];
        let mut win_tq = [0.0f32; MAX_FLAT_SHORT];
        let mut win_forced = [false; MAX_FLAT_SHORT];
        // Per-window energy / allowed noise / Σ√|x| for the noise targets.
        let mut win_e = [0.0f32; MAX_FLAT_SHORT];
        let mut win_n = [0.0f32; MAX_FLAT_SHORT];
        let mut win_s = [0.0f32; MAX_FLAT_SHORT];
        for w in 0..MAX_GROUPS {
            let (lo, hi) = (w * n_sfb, (w + 1) * n_sfb);
            psy.analyze(
                &spec[w * SHORT_WINDOW_LEN..(w + 1) * SHORT_WINDOW_LEN],
                offsets,
                crate::engine::enc_frame::TARGET_Q,
                &mut win_coded[lo..hi],
                &mut win_tq[lo..hi],
            );
            let (e, n, s) = psy.bands();
            win_e[lo..hi].copy_from_slice(&e[..n_sfb]);
            win_n[lo..hi].copy_from_slice(&n[..n_sfb]);
            win_s[lo..hi].copy_from_slice(&s[..n_sfb]);
            if tns.short().window_on(w) {
                for b in 0..n_sfb.min(14) {
                    win_forced[lo + b] = !win_coded[lo + b];
                    win_coded[lo + b] = true;
                    win_tq[lo + b] = crate::engine::enc_frame::TARGET_Q;
                }
            }
        }
        let mut peaks = [0.0f32; MAX_FLAT_SHORT];
        enc_quant::band_peaks_short(spec, offsets, n_sfb, &mut peaks);
        let mut gpeaks = [0.0f32; MAX_FLAT_SHORT];
        enc_group::fold_windows(
            grouping,
            n_sfb,
            &win_coded,
            &win_tq,
            &peaks,
            &mut self.coded,
            &mut self.cap,
            &mut gpeaks,
        );
        // Fold energy / noise / Σ√|x| (sums) and the TNS forcing (any) per group.
        let n_groups = usize::from(grouping.n_groups);
        let (mut ge, mut gn, mut gs) = (
            [0.0f32; MAX_FLAT_SHORT],
            [0.0f32; MAX_FLAT_SHORT],
            [0.0f32; MAX_FLAT_SHORT],
        );
        let mut gw = [0.0f32; MAX_FLAT_SHORT];
        let mut gforced = [false; MAX_FLAT_SHORT];
        let mut wbase = 0usize;
        for g in 0..n_groups {
            for k in 0..grouping.group_len[g] as usize {
                for b in 0..n_sfb {
                    let (o, i) = (g * n_sfb + b, (wbase + k) * n_sfb + b);
                    ge[o] += win_e[i];
                    gn[o] += win_n[i];
                    gs[o] += win_s[i];
                    gw[o] += f32::from(offsets[b + 1] - offsets[b]);
                    gforced[o] |= win_forced[i];
                }
            }
            wbase += grouping.group_len[g] as usize;
        }
        self.peaks = gpeaks;
        self.energy = ge;
        self.noise = gn;
        self.sum_sqrt = gs;
        self.width = gw;
        self.forced = gforced;
        self.grouping = grouping;
    }
}
