//! Inverse polyphase quadrature filter for AAC SSR.
//!
//! The 96-tap prototype is the normative half-band table; coefficients are
//! mirrored and modulated once. Each call consumes one sample from each of
//! the four bands and emits four full-band samples.

#![allow(clippy::excessive_precision)]

use super::ssr::{BAND, BANDS, BUF, KK};

/// Half of the 96-tap SSR prototype. `proto[j] = proto[95 - j] = HALF[j]`.
const PROTO_HALF: [f64; 48] = [
    1.2206911375946939E-05,
    1.7261986723798209E-05,
    1.2300093657077942E-05,
    -1.0833943097791965E-05,
    -5.7772498639901686E-05,
    -1.2764767618947719E-04,
    -2.0965186675013334E-04,
    -2.8166673689263850E-04,
    -3.1234860429017460E-04,
    -2.6738519958452353E-04,
    -1.1949424681824722E-04,
    1.3965139412648678E-04,
    4.8864136409185725E-04,
    8.7044629275148344E-04,
    1.1949430269934793E-03,
    1.3519708175026700E-03,
    1.2346314373964412E-03,
    7.6953209114159191E-04,
    -5.2242432579537141E-05,
    -1.1516092887213454E-03,
    -2.3538469841711277E-03,
    -3.4033123072127277E-03,
    -4.0028551071986133E-03,
    -3.8745415659693259E-03,
    -2.8321073426874310E-03,
    -8.5038892323704195E-04,
    1.8856751185350931E-03,
    4.9688741735340923E-03,
    7.8056704536795926E-03,
    9.7027909685901654E-03,
    9.9960423120166159E-03,
    8.2019366335594487E-03,
    4.1642072876103365E-03,
    -1.8364453822737758E-03,
    -9.0384863094167686E-03,
    -1.6241528177129844E-02,
    -2.1939551286300665E-02,
    -2.4533179947088161E-02,
    -2.2591663337768787E-02,
    -1.5122066420044672E-02,
    -1.7971713448186293E-03,
    1.6903413428575379E-02,
    3.9672315874127042E-02,
    6.4487527248102796E-02,
    8.8850025474701726E-02,
    0.1101132906105560,
    0.1258540205143761,
    0.1342239368467012,
];

struct Pqf {
    q0: [[f32; BANDS]; BANDS],
    t0: [[f32; KK]; BANDS],
    t1: [[f32; KK]; BANDS],
}

#[allow(clippy::needless_range_loop)]
fn coeffs() -> Pqf {
    let mut proto = [0.0f64; 96];
    for j in 0..48 {
        proto[j] = PROTO_HALF[j];
        proto[95 - j] = PROTO_HALF[j];
    }
    let mut q0 = [[0.0f32; BANDS]; BANDS];
    let mm = BANDS as f64;
    let scale = std::f64::consts::PI / (4.0 * mm);
    for n in 0..BANDS / 2 {
        for i in 0..BANDS {
            let base = (2 * i + 1) as f64 * scale;
            let w = base * ((2 * n + 1) as f64 - mm);
            q0[n][i] = (2.0 * w.cos()) as f32;
            let w2 = base * (2.0 * (mm + n as f64) + 1.0 - mm);
            q0[n + BANDS / 2][i] = (2.0 * w2.cos()) as f32;
        }
    }
    let mut t0 = [[0.0f32; KK]; BANDS];
    let mut t1 = [[0.0f32; KK]; BANDS];
    for n in 0..BANDS {
        for k in 0..KK {
            let mut a = mm * proto[2 * k * BANDS + n];
            let mut b = mm * proto[(2 * k + 1) * BANDS + n];
            if k % 2 != 0 {
                a = -a;
                b = -b;
            }
            t0[n][k] = a as f32;
            t1[n][k] = b as f32;
        }
    }
    Pqf { q0, t0, t1 }
}

#[allow(clippy::needless_range_loop)]
pub(super) fn ipqf(mem: &mut [[f32; BUF]; BANDS], bands: &[[f32; BAND]; BANDS], dst: &mut [f32]) {
    let pqf = coeffs_once();
    for i in 0..BAND {
        for row in mem.iter_mut() {
            for k in 0..BUF - 1 {
                row[k] = row[k + 1];
            }
        }
        for n in 0..BANDS {
            let mut acc = 0.0f32;
            for l in 0..BANDS {
                acc += pqf.q0[n][l] * bands[l][i];
            }
            mem[n][BUF - 1] = acc;
        }
        for n in 0..BANDS / 2 {
            let mut acc = 0.0f32;
            for k in 0..KK {
                acc += pqf.t0[n][k] * mem[n][BUF - 1 - 2 * k];
            }
            for k in 0..KK {
                acc += pqf.t1[n][k] * mem[n + BANDS / 2][BUF - 2 - 2 * k];
            }
            dst[i * BANDS + n] = acc;
            let mut acc = 0.0f32;
            for k in 0..KK {
                acc += pqf.t0[BANDS - 1 - n][k] * mem[n][BUF - 1 - 2 * k];
            }
            for k in 0..KK {
                acc -= pqf.t1[BANDS - 1 - n][k] * mem[n + BANDS / 2][BUF - 2 - 2 * k];
            }
            dst[i * BANDS + BANDS - 1 - n] = acc;
        }
    }
}

fn coeffs_once() -> &'static Pqf {
    use std::sync::OnceLock;
    static PQF: OnceLock<Pqf> = OnceLock::new();
    PQF.get_or_init(coeffs)
}
