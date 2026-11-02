//! Two bounded u16 book costs in one u32 accumulator.
use super::*;

const fn pair<const N: usize>(lo: &[u8; N], hi: &[u8; N]) -> [u32; N] {
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        assert!((lo[i] as u32 + 4) * 512 < 65536);
        assert!((hi[i] as u32 + 4) * 512 < 65536);
        out[i] = lo[i] as u32 | ((hi[i] as u32) << 16);
        i += 1;
    }
    out
}

static H12: [u32; 81] = pair(&huff_quad::H1_LEN, &huff_quad::H2_LEN);
static H34: [u32; 81] = pair(&huff_quad::H3_LEN, &huff_quad::H4_LEN);
static H56: [u32; 81] = pair(&huff_pair::H5_LEN, &huff_pair::H6_LEN);
static H78: [u32; 64] = pair(&huff_pair::H7_LEN, &huff_pair::H8_LEN);
static H910: [u32; 169] = pair(&huff_pair::H9_LEN, &huff_pair::H10_LEN);

fn both(value: u32) -> u32 {
    value | (value << 16)
}

fn unpack(out: &mut [u32; BOOKS], cb: usize, value: u32) {
    out[cb] = value & 0xffff;
    out[cb + 1] = value >> 16;
}

/// Only <=1024 coefficients with magnitudes <=7 enter this path. There
/// are at most 512 pairs or 256 quads. Table construction verifies that
/// 512 times any entry plus four signs fits a u16, so the low cost lane
/// cannot carry into the high one. Larger bands retain the scalar path.
pub(super) fn fill_small(out: &mut [u32; BOOKS], vals: &[i32], max: u32) {
    debug_assert!(vals.len() <= 1024 && (1..=7).contains(&max));
    let mut t = [0; BOOKS];
    if max <= 2 {
        let (mut signed, mut unsigned) = (0u32, 0u32);
        for v in vals.chunks_exact(4) {
            let a = [v[0].abs(), v[1].abs(), v[2].abs(), v[3].abs()];
            let nz = a.iter().filter(|&&x| x != 0).count() as u32;
            let u = (a[0] * 27 + a[1] * 9 + a[2] * 3 + a[3]) as usize;
            unsigned += H34[u] + both(nz);
            if max == 1 {
                let s = ((v[0] + 1) * 27 + (v[1] + 1) * 9 + (v[2] + 1) * 3 + v[3] + 1) as usize;
                signed += H12[s];
            }
        }
        unpack(&mut t, 1, signed);
        unpack(&mut t, 3, unsigned);
    }
    if max <= 4 {
        pairs4(&mut t, vals);
    } else {
        pairs7(&mut t, vals);
    }
    const LAV: [u32; BOOKS] = [0, 1, 1, 2, 2, 4, 4, 7, 7, 12, 12, u32::MAX];
    for cb in 1..BOOKS {
        out[cb] = if max <= LAV[cb] {
            t[cb]
        } else {
            UNREPRESENTABLE
        };
    }
}

fn pairs4(t: &mut [u32; BOOKS], vals: &[i32]) {
    let (mut p56, mut p78, mut p910, mut p11) = (0u32, 0u32, 0u32, 0u32);
    for v in vals.chunks_exact(2) {
        let (a, b) = (v[0].unsigned_abs().min(4), v[1].unsigned_abs().min(4));
        let nz = both(u32::from(a != 0) + u32::from(b != 0));
        let s = ((v[0].clamp(-4, 4) + 4) * 9 + v[1].clamp(-4, 4) + 4) as usize;
        p56 += H56[s];
        p78 += H78[(a * 8 + b) as usize] + nz;
        p910 += H910[(a * 13 + b) as usize] + nz;
        p11 += esc_pair(a, b);
    }
    unpack(t, 5, p56);
    unpack(t, 7, p78);
    unpack(t, 9, p910);
    t[11] = p11;
}

fn pairs7(t: &mut [u32; BOOKS], vals: &[i32]) {
    let (mut p78, mut p910, mut p11) = (0u32, 0u32, 0u32);
    for v in vals.chunks_exact(2) {
        let (a, b) = (v[0].unsigned_abs().min(7), v[1].unsigned_abs().min(7));
        let nz = both(u32::from(a != 0) + u32::from(b != 0));
        p78 += H78[(a * 8 + b) as usize] + nz;
        p910 += H910[(a * 13 + b) as usize] + nz;
        p11 += esc_pair(a, b);
    }
    unpack(t, 7, p78);
    unpack(t, 9, p910);
    t[11] = p11;
}

#[cfg(test)]
#[path = "enc_quant_packed_tests.rs"]
mod tests;
