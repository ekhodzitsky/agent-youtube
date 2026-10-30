//! f64 sin/cos/atan/acos/exp2 from exactly-rounded arithmetic.
//!
//! Decoder tables and per-frame TNS / QMF / PS angles go through here so
//! the planar PCM does not depend on the host libm. Arguments may be any
//! finite f64. `|a| < 1e6` reduces with a double-word π/2; larger finite
//! values use an integer Payne–Hanek step against the same π bits. The
//! polynomials are plain Horner schemes (no `mul_add`, so no FMA).
//! `exp2` splits with `floor` and a degree-16 series for `e^(f·ln 2)`.

const INV_PIO2: f64 = f64::from_bits(0x3FE4_5F30_6DC9_C883);
const PIO2_1: f64 = f64::from_bits(0x3FF9_21FB_5440_0000);
const PIO2_1T: f64 = f64::from_bits(0x3DD0_B461_1A62_6331);
/// 2/π after the binary point, most-significant limb first (1280 bits).
const TWO_OVER_PI: [u64; 20] = [
    0xA2F9_836E_4E44_1529,
    0xFC27_57D1_F534_DDC0,
    0xDB62_9599_3C43_9041,
    0xFE51_63AB_DEBB_C561,
    0xB724_6E3A_424D_D2E0,
    0x0649_2EEA_09D1_921C,
    0xFE1D_EB1C_B129_A73E,
    0xE882_35F5_2EBB_4484,
    0xE99C_7026_B45F_7E41,
    0x3991_D639_8353_39F4,
    0x9C84_5F8B_BDF9_283B,
    0x1FF8_97FF_DE05_980F,
    0xEF2F_118B_5A0A_6D1F,
    0x6D36_7ECF_27CB_09B7,
    0x4F46_3F66_9E5F_EA2D,
    0x7527_BAC7_EBE5_F17B,
    0x3D07_39F7_8A52_92EA,
    0x6BFB_5FB1_1F8D_5D08,
    0x5603_3046_FC7B_6BAB,
    0xF0CF_BC20_9AF4_361D,
];
const SQRT_3: f64 = 1.732_050_807_568_877_2;
/// tan(π/12) = 2 − √3. Sterbenz makes the subtraction exact.
const TAN_PI_12: f64 = 2.0 - SQRT_3;
/// Extra fraction bits in the large-argument reduction.
const GUARD: i32 = 80;

fn trunc_to_zero(x: f64) -> f64 {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let e = exp - 1023;
    if exp == 0x7ff {
        return x;
    }
    if e < 0 {
        return 0.0;
    }
    if e >= 52 {
        return x;
    }
    let mask = !((1u64 << (52 - e)) - 1);
    f64::from_bits(bits & mask)
}

/// Round half away from zero. Exact for `|x| < 2^52`, which covers the
/// medium reduction (`|a| < 1e6`).
fn round_half_away(x: f64) -> f64 {
    let sign = x.to_bits() & 0x8000_0000_0000_0000;
    let ax = f64::from_bits(x.to_bits() & !sign);
    let t = trunc_to_zero(ax + 0.5);
    f64::from_bits(t.to_bits() | sign)
}

fn shr_limbs(le: &mut [u64], bits: usize) {
    if bits == 0 {
        return;
    }
    let n = bits / 64;
    if n > 0 {
        for i in 0..le.len() {
            le[i] = if i + n < le.len() { le[i + n] } else { 0 };
        }
    }
    let b = bits % 64;
    if b == 0 {
        return;
    }
    let mut carry = 0u64;
    for limb in le.iter_mut().rev() {
        let spilled = *limb << (64 - b);
        *limb = (*limb >> b) | carry;
        carry = spilled;
    }
}

/// Top `w` fraction bits of 2/π as a little-endian integer.
fn pi_inv_window(w: usize) -> ([u64; 20], usize) {
    let src_limbs = w.div_ceil(64).min(TWO_OVER_PI.len());
    let extra = src_limbs * 64 - w.min(src_limbs * 64);
    let mut le = [0u64; 20];
    for i in 0..src_limbs {
        le[src_limbs - 1 - i] = TWO_OVER_PI[i];
    }
    shr_limbs(&mut le[..src_limbs], extra);
    let mut n = src_limbs;
    while n > 1 && le[n - 1] == 0 {
        n -= 1;
    }
    (le, n)
}

fn limb_or0(prod: &[u64], i: usize) -> u64 {
    if i < prod.len() { prod[i] } else { 0 }
}

fn bits_at(prod: &[u64], lo: usize, len: usize) -> u64 {
    let i = lo / 64;
    let off = lo % 64;
    let mut v = u128::from(limb_or0(prod, i)) >> off;
    if off != 0 {
        v |= u128::from(limb_or0(prod, i + 1)) << (64 - off);
    }
    (v & ((1u128 << len) - 1)) as u64
}

fn mul_mant(b: &[u64], m: u64, prod: &mut [u64]) {
    let mut carry = 0u128;
    for (i, &limb) in b.iter().enumerate() {
        let t = u128::from(limb) * u128::from(m) + carry;
        prod[i] = t as u64;
        carry = t >> 64;
    }
    prod[b.len()] = carry as u64;
}

fn rem_pio2_large(ax: f64) -> (i32, f64) {
    let bits = ax.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0x7ff {
        return (0, f64::NAN);
    }
    let mant = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let e = exp - 1023;
    let w = (e + GUARD) as usize;
    let sh = (GUARD + 52) as usize;
    let (window, nlimbs) = pi_inv_window(w);
    let mut prod = [0u64; 21];
    mul_mant(&window[..nlimbs], mant, &mut prod);
    let mut n = bits_at(&prod, sh, 2) as i32;
    let ge_half = bits_at(&prod, sh - 1, 1) != 0;
    let top = bits_at(&prod, sh - 54, 54) as f64;
    let mut frac = top * f64::from_bits(0x3C90_0000_0000_0000); // 2^-54
    if ge_half {
        n = (n + 1) & 3;
        frac -= 1.0;
    }
    (n, frac * PIO2_1 + frac * PIO2_1T)
}

/// `a = n·(π/2) + r` with `|r| ≤ π/4` aside from a 1 ulp boundary.
fn rem_pio2(x: f64) -> (i32, f64) {
    if !x.is_finite() {
        return (0, f64::NAN);
    }
    let ax = x.abs();
    if ax <= std::f64::consts::FRAC_PI_4 {
        return (0, x);
    }
    let (mut n, mut r) = if ax < 1.0e6 {
        let nf = round_half_away(x * INV_PIO2);
        let n = nf as i32;
        let r = (x - nf * PIO2_1) - nf * PIO2_1T;
        (n, r)
    } else {
        let (n, r) = rem_pio2_large(ax);
        if x.is_sign_negative() {
            (-n, -r)
        } else {
            (n, r)
        }
    };
    // A quadrant that is off by one lands near ±π/2, not near ±π/4.
    if r > 0.8 {
        n += 1;
        r = (r - PIO2_1) - PIO2_1T;
    } else if r < -0.8 {
        n -= 1;
        r = (r + PIO2_1) + PIO2_1T;
    }
    (n, r)
}

fn sin_poly(r: f64) -> f64 {
    let z = r * r;
    r * (1.0
        + z * (-1.0 / 6.0
            + z * (1.0 / 120.0
                + z * (-1.0 / 5040.0
                    + z * (1.0 / 362_880.0
                        + z * (-1.0 / 39_916_800.0
                            + z * (1.0 / 6_227_020_800.0
                                + z * (-1.0 / 1_307_674_368_000.0
                                    + z / 355_687_428_096_000.0))))))))
}

fn cos_poly(r: f64) -> f64 {
    let z = r * r;
    1.0 + z
        * (-0.5
            + z * (1.0 / 24.0
                + z * (-1.0 / 720.0
                    + z * (1.0 / 40_320.0
                        + z * (-1.0 / 3_628_800.0
                            + z * (1.0 / 479_001_600.0
                                + z * (-1.0 / 87_178_291_200.0 + z / 20_922_789_888_000.0)))))))
}

/// `(sin a, cos a)` for any finite `a`.
pub fn sincos_f64(a: f64) -> (f64, f64) {
    if !a.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let (q, r) = rem_pio2(a);
    let (s, c) = (sin_poly(r), cos_poly(r));
    match q.rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// `sin a` for any finite `a`.
pub fn sin_f64(a: f64) -> f64 {
    sincos_f64(a).0
}

fn atan_poly(t: f64) -> f64 {
    let u = t * t;
    let mut p = -1.0 / 31.0;
    let mut n = 14i32;
    while n >= 0 {
        let den = f64::from(2 * n + 1);
        let c = if n % 2 == 0 { 1.0 / den } else { -1.0 / den };
        p = c + u * p;
        n -= 1;
    }
    t * p
}

/// `atan(x)` for any finite `x` (infinities map to ±π/2).
pub fn atan_f64(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 0.0 {
        return -atan_f64(-x);
    }
    if x == 0.0 {
        return 0.0;
    }
    if !x.is_finite() || x > 1.0e16 {
        return std::f64::consts::FRAC_PI_2;
    }
    if x > 1.0 {
        return std::f64::consts::FRAC_PI_2 - atan_f64(1.0 / x);
    }
    let (t, shift) = if x > TAN_PI_12 {
        (
            (x * SQRT_3 - 1.0) / (x + SQRT_3),
            std::f64::consts::FRAC_PI_6,
        )
    } else {
        (x, 0.0)
    };
    shift + atan_poly(t)
}

/// `atan2(y, x)`. `(0, 0)` is `0`.
pub fn atan2_f64(y: f64, x: f64) -> f64 {
    if y.is_nan() || x.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 {
        if y > 0.0 {
            return std::f64::consts::FRAC_PI_2;
        }
        if y < 0.0 {
            return -std::f64::consts::FRAC_PI_2;
        }
        return 0.0;
    }
    if y == 0.0 {
        return if x > 0.0 {
            0.0
        } else if y.is_sign_negative() {
            -std::f64::consts::PI
        } else {
            std::f64::consts::PI
        };
    }
    let a = atan_f64(y / x);
    if x > 0.0 {
        a
    } else if y >= 0.0 {
        a + std::f64::consts::PI
    } else {
        a - std::f64::consts::PI
    }
}

/// `acos(x)` for `x` in `[-1, 1]`.
pub fn acos_f64(x: f64) -> f64 {
    if x.is_nan() || !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    if x == 1.0 {
        return 0.0;
    }
    if x == -1.0 {
        return std::f64::consts::PI;
    }
    atan2_f64(((1.0 - x) * (1.0 + x)).sqrt(), x)
}

/// log2(10), correctly rounded. `10^x = 2^(x·log2(10))`.
const LOG2_10: f64 = f64::from_bits(0x400A_934F_0979_A371);
/// ln(2), correctly rounded.
const LN_2: f64 = f64::from_bits(0x3FE6_2E42_FEFA_39EF);

/// Exact `2^n` on the normal and subnormal range.
const fn pow2i(n: i32) -> f64 {
    if n >= 1024 {
        return f64::INFINITY;
    }
    if n < -1074 {
        return 0.0;
    }
    if n >= -1022 {
        return f64::from_bits(((n as i64 + 1023) as u64) << 52);
    }
    f64::from_bits(1u64 << (n + 1074))
}

/// `e^r` for `0 ≤ r < ln 2`. Taylor through degree 16; the next term
/// at `r = ln 2` is under half an ulp of 1.
const fn exp_small(r: f64) -> f64 {
    let mut p = 1.0;
    let mut k = 16i32;
    while k >= 1 {
        p = 1.0 + r * p / (k as f64);
        k -= 1;
    }
    p
}

/// `floor(x)` from the bit pattern (`f64::floor` is not const on the MSRV).
const fn floor_f64(x: f64) -> f64 {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0x7ff || (bits << 1) == 0 {
        return x;
    }
    let e = exp - 1023;
    if e < 0 {
        return if bits & 0x8000_0000_0000_0000 == 0 {
            0.0
        } else {
            -1.0
        };
    }
    if e >= 52 {
        return x;
    }
    let trunc = f64::from_bits(bits & !((1u64 << (52 - e)) - 1));
    if bits & 0x8000_0000_0000_0000 != 0 && trunc != x {
        trunc - 1.0
    } else {
        trunc
    }
}

/// `2^x` for any finite `x`, from `+ - * /` and an exact power of two.
pub const fn exp2_f64(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x >= 1024.0 {
        return f64::INFINITY;
    }
    if x <= -1075.0 {
        return 0.0;
    }
    let n = floor_f64(x) as i32;
    let f = x - (n as f64);
    pow2i(n) * exp_small(f * LN_2)
}

/// `10^x` via [`exp2_f64`] and the rounded `log2(10)` literal.
pub const fn exp10_f64(x: f64) -> f64 {
    exp2_f64(x * LOG2_10)
}
