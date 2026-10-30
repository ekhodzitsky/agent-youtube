//! Per-band Huffman cost under every spectral book (TASK-83 hot path).
//!
//! One pass over the band: the band's largest magnitude decides which
//! books can represent it (LAV 1 / 2 / 4 / 7 / 12, book 11 escapes), and
//! book pairs that share a tuple ordinal (1–2, 3–4, 5–6, 7–8, 9–10) are
//! costed from one index. Bit-for-bit the same table as walking
//! [`crate::engine::enc_huff::spectral_bits`] book by book.
//!
//! The quantizer gain `2^(−3/16·(sf−100))` depends only on the scalefactor.
//! On the wire `sf` is 0..=255 and `16·x` is an integer, so
//! [`det_math::exp2`](crate::engine::det_math::exp2) is one table product.
//! The 256-entry table is that function, filled once.

use std::sync::LazyLock;

use super::{BOOKS, UNREPRESENTABLE};
use crate::engine::det_math;
use crate::engine::enc_huff::esc_bits;
use crate::engine::spectrum::SF_OFFSET;
use crate::engine::{huff_esc, huff_pair, huff_quad};

/// `2^(−0.1875·(sf−100))` for scalefactors 0..=255.
static QUANT_GAIN: LazyLock<[f32; 256]> = LazyLock::new(|| {
    let mut t = [0.0f32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = det_math::exp2(-0.1875 * (i as i32 - SF_OFFSET) as f32);
    }
    t
});

/// `2^(−0.1875·(sf−100))`, bit-identical to [`det_math::exp2`].
#[inline]
pub(super) fn quant_gain(sf: i32) -> f32 {
    if let Ok(i) = usize::try_from(sf)
        && i < QUANT_GAIN.len()
    {
        QUANT_GAIN[i]
    } else {
        det_math::exp2(-0.1875 * (sf - SF_OFFSET) as f32)
    }
}

/// Fill `bits[1..=11]` for the quantized band `vals` (width a multiple of
/// 4, as every AAC scalefactor band is). Slot 0 is left untouched.
pub(super) fn fill_bits(bits: &mut [u32; BOOKS], vals: &[i32]) {
    let max = vals.iter().fold(0u32, |m, v| m.max(v.unsigned_abs()));
    if max == 0 {
        fill_zero(bits, vals.len());
        return;
    }
    if max > 12 {
        bits[1..11].fill(UNREPRESENTABLE);
        bits[11] = pairs_esc(vals);
        return;
    }
    let mut t = [0u32; BOOKS];
    if max <= 2 {
        quads(&mut t, vals, max == 1);
    }
    match max {
        1..=4 => pairs_through_4(&mut t, vals),
        5..=7 => pairs_through_7(&mut t, vals),
        _ => pairs_through_12(&mut t, vals),
    }
    const LAV: [u32; BOOKS] = [0, 1, 1, 2, 2, 4, 4, 7, 7, 12, 12, u32::MAX];
    for cb in 1..BOOKS {
        bits[cb] = if max <= LAV[cb] {
            t[cb]
        } else {
            UNREPRESENTABLE
        };
    }
}

/// All-zero band: every tuple is the zero index, so the cost is
/// `n_tuples · length` with no sign bits and no escapes.
fn fill_zero(bits: &mut [u32; BOOKS], n: usize) {
    let nq = (n / 4) as u32;
    let np = (n / 2) as u32;
    // Signed zero tuple `(v+1)` is index 40; unsigned zero is index 0.
    bits[1] = nq * u32::from(huff_quad::H1_LEN[40]);
    bits[2] = nq * u32::from(huff_quad::H2_LEN[40]);
    bits[3] = nq * u32::from(huff_quad::H3_LEN[0]);
    bits[4] = nq * u32::from(huff_quad::H4_LEN[0]);
    bits[5] = np * u32::from(huff_pair::H5_LEN[40]);
    bits[6] = np * u32::from(huff_pair::H6_LEN[40]);
    bits[7] = np * u32::from(huff_pair::H7_LEN[0]);
    bits[8] = np * u32::from(huff_pair::H8_LEN[0]);
    bits[9] = np * u32::from(huff_pair::H9_LEN[0]);
    bits[10] = np * u32::from(huff_pair::H10_LEN[0]);
    bits[11] = np * u32::from(huff_esc::H11_LEN[0]);
}

fn quads(t: &mut [u32; BOOKS], vals: &[i32], signed: bool) {
    for v in vals.chunks_exact(4) {
        let a = [v[0].abs(), v[1].abs(), v[2].abs(), v[3].abs()];
        let nz = a.iter().filter(|&&x| x != 0).count() as u32;
        let u = (a[0] * 27 + a[1] * 9 + a[2] * 3 + a[3]) as usize;
        t[3] += u32::from(huff_quad::H3_LEN[u]) + nz;
        t[4] += u32::from(huff_quad::H4_LEN[u]) + nz;
        if signed {
            let s = ((v[0] + 1) * 27 + (v[1] + 1) * 9 + (v[2] + 1) * 3 + (v[3] + 1)) as usize;
            t[1] += u32::from(huff_quad::H1_LEN[s]);
            t[2] += u32::from(huff_quad::H2_LEN[s]);
        }
    }
}

#[inline(always)]
fn tbl(table: &[u8], idx: u32) -> u32 {
    let i = idx as usize;
    // Callers clamp `idx` into the table (Huffman LAV arithmetic).
    unsafe {
        std::hint::assert_unchecked(i < table.len());
        u32::from(*table.get_unchecked(i))
    }
}

#[inline]
fn esc_pair(a: u32, b: u32) -> u32 {
    let nz = u32::from(a != 0) + u32::from(b != 0);
    let u = a.min(16) * 17 + b.min(16);
    let mut bits = tbl(&huff_esc::H11_LEN, u) + nz;
    if a >= 16 {
        bits += esc_bits(a) as u32;
    }
    if b >= 16 {
        bits += esc_bits(b) as u32;
    }
    bits
}

fn pairs_esc(vals: &[i32]) -> u32 {
    let mut bits = 0u32;
    for v in vals.chunks_exact(2) {
        bits += esc_pair(v[0].unsigned_abs(), v[1].unsigned_abs());
    }
    bits
}

fn pairs_through_4(t: &mut [u32; BOOKS], vals: &[i32]) {
    for v in vals.chunks_exact(2) {
        let (a, b) = (v[0].unsigned_abs().min(4), v[1].unsigned_abs().min(4));
        let nz = u32::from(a != 0) + u32::from(b != 0);
        let s0 = v[0].clamp(-4, 4);
        let s1 = v[1].clamp(-4, 4);
        let s = ((s0 + 4) * 9 + (s1 + 4)) as u32;
        t[5] += tbl(&huff_pair::H5_LEN, s);
        t[6] += tbl(&huff_pair::H6_LEN, s);
        let u7 = a * 8 + b;
        t[7] += tbl(&huff_pair::H7_LEN, u7) + nz;
        t[8] += tbl(&huff_pair::H8_LEN, u7) + nz;
        let u9 = a * 13 + b;
        t[9] += tbl(&huff_pair::H9_LEN, u9) + nz;
        t[10] += tbl(&huff_pair::H10_LEN, u9) + nz;
        t[11] += esc_pair(a, b);
    }
}

fn pairs_through_7(t: &mut [u32; BOOKS], vals: &[i32]) {
    for v in vals.chunks_exact(2) {
        let (a, b) = (v[0].unsigned_abs().min(7), v[1].unsigned_abs().min(7));
        let nz = u32::from(a != 0) + u32::from(b != 0);
        let u7 = a * 8 + b;
        t[7] += tbl(&huff_pair::H7_LEN, u7) + nz;
        t[8] += tbl(&huff_pair::H8_LEN, u7) + nz;
        let u9 = a * 13 + b;
        t[9] += tbl(&huff_pair::H9_LEN, u9) + nz;
        t[10] += tbl(&huff_pair::H10_LEN, u9) + nz;
        t[11] += esc_pair(a, b);
    }
}

fn pairs_through_12(t: &mut [u32; BOOKS], vals: &[i32]) {
    for v in vals.chunks_exact(2) {
        let (a, b) = (v[0].unsigned_abs().min(12), v[1].unsigned_abs().min(12));
        let nz = u32::from(a != 0) + u32::from(b != 0);
        let u9 = a * 13 + b;
        t[9] += tbl(&huff_pair::H9_LEN, u9) + nz;
        t[10] += tbl(&huff_pair::H10_LEN, u9) + nz;
        t[11] += esc_pair(a, b);
    }
}

/// Sign `spec` onto `floor(|mag| * gain + 0.4054)` clamped at [`QUANT_MAX`].
/// Non-negative products use trunc-toward-zero, which matches `floor`.
pub(super) fn write_quants(quant: &mut [i32], spec: &[f32], mag: &[f32], gain: f32) {
    let n = quant.len().min(spec.len()).min(mag.len());
    #[cfg(target_arch = "x86_64")]
    let mut i = write_quants_sse(quant, spec, mag, gain, n);
    #[cfg(not(target_arch = "x86_64"))]
    let mut i = 0;
    while i < n {
        let m = super::quant_mag(mag[i], gain);
        quant[i] = if spec[i] < 0.0 { -m } else { m };
        i += 1;
    }
}

#[cfg(target_arch = "x86_64")]
fn write_quants_sse(quant: &mut [i32], spec: &[f32], mag: &[f32], gain: f32, n: usize) -> usize {
    use std::arch::x86_64::{
        _mm_add_ps, _mm_castps_si128, _mm_cmpge_ps, _mm_cmplt_ps, _mm_cvttps_epi32, _mm_loadu_ps,
        _mm_min_ps, _mm_movemask_ps, _mm_mul_ps, _mm_set1_ps, _mm_setzero_ps, _mm_storeu_si128,
        _mm_sub_epi32, _mm_xor_si128,
    };
    let mut i = 0;
    // x86-64 baseline is SSE2. Lanes stay in-range: `i + 4 <= n` and both
    // slices are at least `n` long. `mulps` then `addps` (no FMA). A lane
    // that is negative or NaN falls back to scalar `floor`.
    unsafe {
        let gain_v = _mm_set1_ps(gain);
        let round_v = _mm_set1_ps(super::QUANT_ROUND);
        let cap_v = _mm_set1_ps(super::QUANT_MAX as f32);
        let zero = _mm_setzero_ps();
        while i + 4 <= n {
            let m = _mm_loadu_ps(mag.as_ptr().add(i));
            let x = _mm_loadu_ps(spec.as_ptr().add(i));
            let v = _mm_add_ps(_mm_mul_ps(m, gain_v), round_v);
            if _mm_movemask_ps(_mm_cmpge_ps(v, zero)) != 0x0F {
                break;
            }
            let qi = _mm_cvttps_epi32(_mm_min_ps(v, cap_v));
            let neg = _mm_castps_si128(_mm_cmplt_ps(x, zero));
            let q = _mm_sub_epi32(_mm_xor_si128(qi, neg), neg);
            _mm_storeu_si128(quant.as_mut_ptr().add(i).cast(), q);
            i += 4;
        }
    }
    i
}

/// `true` if consecutive coded scalefactors are in `[0, 255]` with DPCM ±60.
#[must_use]
pub fn dpcm_ok(sf: &[i32], coded: &[bool], n: usize) -> bool {
    let mut prev: Option<i32> = None;
    for b in 0..n {
        if !coded[b] {
            continue;
        }
        let v = sf[b];
        if !(0..=255).contains(&v) {
            return false;
        }
        if let Some(p) = prev
            && (v - p).abs() > 60
        {
            return false;
        }
        prev = Some(v);
    }
    true
}
