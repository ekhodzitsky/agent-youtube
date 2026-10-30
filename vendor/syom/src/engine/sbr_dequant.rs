//! SBR envelope / noise-floor dequantization — ISO/IEC 14496-3
//! §4.6.18.3.5 "Dequantization and stereo decoding".
//!
//! Converts the reconstructed *quantized* scalefactors
//! ([`crate::engine::sbr_reconstruct`]'s `E_Q(k,l)` / `Q(k,l)`) into the linear
//! energy values `EOrig(k,l)` / `QOrig(k,l)` the envelope adjuster
//! (§4.6.18.7) consumes:
//!
//! * Single channel (or an uncoupled pair, `bs_coupling == 0`):
//!   `EOrig = 64 · 2^(E/a)` with `a = 2` for `bs_amp_res = 0` (1.5 dB
//!   steps) and `a = 1` for `bs_amp_res = 1` (3.0 dB steps);
//!   `QOrig = 2^(NOISE_FLOOR_OFFSET − Q)` with
//!   `NOISE_FLOOR_OFFSET = 6` (§4.6.18.2.5).
//! * Coupled pair (`bs_coupling == 1`): channel 0 carries the
//!   level average and channel 1 the pan ratio;
//!   `panOffset = [24, 12]` (§4.6.18.2.6) recentres the ratio. The
//!   left / right split divides the doubled average
//!   `64·2^(E0/a + 1)` by `1 + 2^(±(panOffset − E1)/a)` (and the
//!   noise analogue with `panOffset(1) = 12`), which preserves
//!   `ELeft + ERight = 2 · (64·2^(E0/a))`.
//!
//! ## Provenance
//!
//! Every formula and constant is from the §4.6.18.3.5 text and the
//! §4.6.18.2.5 / §4.6.18.2.6 constant lists of the staged spec. No part
//! of this implementation is derived from any external decoder.

use crate::engine::sbr_reconstruct::{EnvelopeScalefactors, NoiseScalefactors};

/// `NOISE_FLOOR_OFFSET = 6` (§4.6.18.2.5).
pub const NOISE_FLOOR_OFFSET: f64 = 6.0;

/// `panOffset = [24, 12]` indexed by `bs_amp_res` (§4.6.18.2.6).
#[inline]
#[must_use]
pub fn pan_offset(amp_res: bool) -> f64 {
    if amp_res { 12.0 } else { 24.0 }
}

/// The §4.6.18.3.5 amplitude-resolution divisor `a`: `2` for
/// `bs_amp_res = 0` (1.5 dB), `1` for `bs_amp_res = 1` (3.0 dB).
#[inline]
#[must_use]
pub fn amp_divisor(amp_res: bool) -> f64 {
    if amp_res { 1.0 } else { 2.0 }
}

/// Dequantized (linear-energy) envelope and noise-floor scalefactors
/// for one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct DequantizedSbr {
    /// `EOrig[l][k]` — linear envelope energies, one band vector per
    /// envelope (band count follows the envelope's frequency
    /// resolution).
    pub e_orig: Vec<Vec<f64>>,
    /// `QOrig[l][k]` — linear noise-floor energies, one `NQ`-band
    /// vector per noise floor.
    pub q_orig: Vec<Vec<f64>>,
}

/// §4.6.18.3.5 single-channel dequantization:
/// `EOrig = 64·2^(E/a)`, `QOrig = 2^(NOISE_FLOOR_OFFSET − Q)`.
#[cfg(test)]
#[must_use]
pub fn dequant_single(
    env: &EnvelopeScalefactors,
    noise: &NoiseScalefactors,
    amp_res: bool,
) -> DequantizedSbr {
    let mut out = DequantizedSbr {
        e_orig: Vec::new(),
        q_orig: Vec::new(),
    };
    dequant_single_into(env, noise, amp_res, &mut out);
    out
}

/// Fill `out` in place (reuses inner row capacity).
pub fn dequant_single_into(
    env: &EnvelopeScalefactors,
    noise: &NoiseScalefactors,
    amp_res: bool,
    out: &mut DequantizedSbr,
) {
    let a = amp_divisor(amp_res);
    fill_exp_rows(&mut out.e_orig, &env.eq, |e| {
        64.0 * (f64::from(e) / a).exp2()
    });
    fill_exp_rows(&mut out.q_orig, &noise.q, |q| {
        (NOISE_FLOOR_OFFSET - f64::from(q)).exp2()
    });
}

fn fill_exp_rows(dst: &mut Vec<Vec<f64>>, src: &[Vec<i32>], f: impl Fn(i32) -> f64) {
    if dst.len() < src.len() {
        dst.resize(src.len(), Vec::new());
    }
    dst.truncate(src.len());
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        d.clear();
        d.extend(s.iter().copied().map(&f));
    }
}

/// §4.6.18.3.5 coupled-pair dequantization.
///
/// `ch0` carries the level average (`E0` / `Q0`), `ch1` the pan ratio
/// (`E1` / `Q1`). Returns the `(left, right)` linear energies.
#[cfg(test)]
#[must_use]
pub fn dequant_coupled(
    env0: &EnvelopeScalefactors,
    noise0: &NoiseScalefactors,
    env1: &EnvelopeScalefactors,
    noise1: &NoiseScalefactors,
    amp_res: bool,
) -> (DequantizedSbr, DequantizedSbr) {
    let mut left = DequantizedSbr {
        e_orig: Vec::new(),
        q_orig: Vec::new(),
    };
    let mut right = DequantizedSbr {
        e_orig: Vec::new(),
        q_orig: Vec::new(),
    };
    dequant_coupled_into(env0, noise0, env1, noise1, amp_res, &mut left, &mut right);
    (left, right)
}

/// Fill coupled left/right energies in place.
pub fn dequant_coupled_into(
    env0: &EnvelopeScalefactors,
    noise0: &NoiseScalefactors,
    env1: &EnvelopeScalefactors,
    noise1: &NoiseScalefactors,
    amp_res: bool,
    left: &mut DequantizedSbr,
    right: &mut DequantizedSbr,
) {
    let a = amp_divisor(amp_res);
    let pan = pan_offset(amp_res);
    fill_coupled_rows(
        &mut left.e_orig,
        &mut right.e_orig,
        &env0.eq,
        &env1.eq,
        |e0, e1| {
            let avg2 = 64.0 * (f64::from(e0) / a + 1.0).exp2();
            let ratio = ((pan - f64::from(e1)) / a).exp2();
            (avg2 / (1.0 + ratio), avg2 / (1.0 + 1.0 / ratio))
        },
    );

    // Noise floors always use panOffset(1) = 12 (§4.6.18.3.5: the
    // noise formulas are written with panOffset(1) regardless of
    // bs_amp_res).
    let noise_pan = pan_offset(true);
    fill_coupled_rows(
        &mut left.q_orig,
        &mut right.q_orig,
        &noise0.q,
        &noise1.q,
        |q0, q1| {
            let avg2 = (NOISE_FLOOR_OFFSET - f64::from(q0) + 1.0).exp2();
            let ratio = (noise_pan - f64::from(q1)).exp2();
            (avg2 / (1.0 + ratio), avg2 / (1.0 + 1.0 / ratio))
        },
    );
}

fn fill_coupled_rows(
    left: &mut Vec<Vec<f64>>,
    right: &mut Vec<Vec<f64>>,
    a: &[Vec<i32>],
    b: &[Vec<i32>],
    f: impl Fn(i32, i32) -> (f64, f64),
) {
    let n = a.len().min(b.len());
    if left.len() < n {
        left.resize(n, Vec::new());
    }
    if right.len() < n {
        right.resize(n, Vec::new());
    }
    left.truncate(n);
    right.truncate(n);
    for i in 0..n {
        left[i].clear();
        right[i].clear();
        for (&x0, &x1) in a[i].iter().zip(b[i].iter()) {
            let (l, r) = f(x0, x1);
            left[i].push(l);
            right[i].push(r);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn env(eq: Vec<Vec<i32>>) -> EnvelopeScalefactors {
        let n = eq.len();
        EnvelopeScalefactors {
            eq,
            freq_res: vec![true; n],
        }
    }

    fn noise(q: Vec<Vec<i32>>) -> NoiseScalefactors {
        NoiseScalefactors { q }
    }

    /// `EOrig = 64·2^(E/a)`: exact powers for both amplitude
    /// resolutions.
    #[test]
    fn single_channel_envelope_powers() {
        let e = env(vec![vec![0, 2, 4]]);
        let q = noise(vec![vec![6]]);
        // bs_amp_res = 1 → a = 1: 64·2^E.
        let d = dequant_single(&e, &q, true);
        assert_eq!(d.e_orig[0], vec![64.0, 256.0, 1024.0]);
        // bs_amp_res = 0 → a = 2: 64·2^(E/2).
        let d = dequant_single(&e, &q, false);
        assert_eq!(d.e_orig[0], vec![64.0, 128.0, 256.0]);
    }

    /// `QOrig = 2^(6 − Q)`: Q = 6 is unity, each +1 halves.
    #[test]
    fn single_channel_noise_powers() {
        let e = env(vec![vec![0]]);
        let q = noise(vec![vec![0, 6, 8]]);
        let d = dequant_single(&e, &q, true);
        assert_eq!(d.q_orig[0], vec![64.0, 1.0, 0.25]);
    }

    /// A balanced pan (`E1 == panOffset`) splits the energy equally:
    /// both channels get exactly the mono dequantization.
    #[test]
    fn coupled_balanced_pan_is_symmetric() {
        for amp_res in [false, true] {
            let e0 = env(vec![vec![4, 8]]);
            let q0 = noise(vec![vec![3]]);
            let e1 = env(vec![vec![
                pan_offset(amp_res) as i32,
                pan_offset(amp_res) as i32,
            ]]);
            let q1 = noise(vec![vec![12]]);
            let (l, r) = dequant_coupled(&e0, &q0, &e1, &q1, amp_res);
            let mono = dequant_single(&e0, &q0, amp_res);
            for k in 0..2 {
                assert!((l.e_orig[0][k] - mono.e_orig[0][k]).abs() < 1e-12);
                assert!((r.e_orig[0][k] - mono.e_orig[0][k]).abs() < 1e-12);
            }
            assert!((l.q_orig[0][0] - mono.q_orig[0][0]).abs() < 1e-12);
            assert!((r.q_orig[0][0] - mono.q_orig[0][0]).abs() < 1e-12);
        }
    }

    /// The coupled split preserves the pair sum:
    /// `ELeft + ERight = 2·(64·2^(E0/a))` for every pan value, and the
    /// same for the noise floors.
    #[test]
    fn coupled_split_preserves_energy_sum() {
        for amp_res in [false, true] {
            for e1v in [0, 5, 11, 17, 24] {
                let e0 = env(vec![vec![6]]);
                let q0 = noise(vec![vec![4]]);
                let e1 = env(vec![vec![e1v]]);
                let q1 = noise(vec![vec![(e1v % 12) * 2]]);
                let (l, r) = dequant_coupled(&e0, &q0, &e1, &q1, amp_res);
                let mono = dequant_single(&e0, &q0, amp_res);
                let sum = l.e_orig[0][0] + r.e_orig[0][0];
                assert!(
                    (sum - 2.0 * mono.e_orig[0][0]).abs() < 1e-9,
                    "amp_res {amp_res} pan {e1v}: {sum}"
                );
                let qsum = l.q_orig[0][0] + r.q_orig[0][0];
                assert!((qsum - 2.0 * mono.q_orig[0][0]).abs() < 1e-9);
            }
        }
    }

    /// A pan below the offset weights the left channel heavier (E1
    /// counts down from left-dominant to right-dominant).
    #[test]
    fn coupled_pan_direction() {
        let e0 = env(vec![vec![6]]);
        let q0 = noise(vec![vec![4]]);
        let e1 = env(vec![vec![2]]);
        let q1 = noise(vec![vec![2]]);
        let (l, r) = dequant_coupled(&e0, &q0, &e1, &q1, true);
        assert!(l.e_orig[0][0] < r.e_orig[0][0]);
        assert!(l.q_orig[0][0] < r.q_orig[0][0]);
    }
}
