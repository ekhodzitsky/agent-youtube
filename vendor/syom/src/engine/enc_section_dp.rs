//! Exact section-partition DP (TASK-73). Not the production planner:
//! measured savings were under the 2% gate (`lab/quality/SECTION.md`).

use super::super::enc_quant::{BOOKS, MAX_BANDS, UNREPRESENTABLE};
use super::{best_book, range_book_bits};

/// Exact min section+spectral cost of `sfb_cb[..n]`.
pub fn section_spectral_cost(
    sfb_cb: &[u8],
    coded: &[bool],
    bits: &[[u32; BOOKS]],
    n: usize,
    header_bits: fn(usize) -> usize,
) -> u64 {
    let mut total = 0u64;
    let mut k = 0usize;
    while k < n {
        let mut len = 1usize;
        while k + len < n && sfb_cb[k + len] == sfb_cb[k] {
            len += 1;
        }
        total += header_bits(len) as u64;
        let cb = usize::from(sfb_cb[k]);
        if cb != 0 {
            let spec = range_book_bits(coded, bits, k, k + len, cb);
            total += u64::from(if spec == UNREPRESENTABLE {
                u32::MAX / 4
            } else {
                spec
            });
        }
        k += len;
    }
    total
}

/// Exact min partition of each coded stretch.
pub fn plan_books_dp(
    coded: &[bool],
    bits: &[[u32; BOOKS]],
    n: usize,
    sfb_cb: &mut [u8],
    header_bits: fn(usize) -> usize,
) {
    let mut lo = 0usize;
    while lo < n {
        if !coded[lo] {
            sfb_cb[lo] = 0;
            lo += 1;
            continue;
        }
        let mut hi = lo + 1;
        while hi < n && coded[hi] {
            hi += 1;
        }
        plan_stretch_dp(coded, bits, sfb_cb, lo, hi, header_bits);
        lo = hi;
    }
}

fn plan_stretch_dp(
    coded: &[bool],
    bits: &[[u32; BOOKS]],
    sfb_cb: &mut [u8],
    lo: usize,
    hi: usize,
    header_bits: fn(usize) -> usize,
) {
    let n = hi - lo;
    if n == 0 {
        return;
    }
    let mut dp = [u32::MAX; MAX_BANDS + 1];
    let mut pred = [0u8; MAX_BANDS + 1];
    let mut pred_cb = [11u8; MAX_BANDS + 1];
    dp[0] = 0;
    for i in 1..=n {
        for j in 0..i {
            let Some((cb, spec)) = best_book(coded, bits, lo + j, lo + i) else {
                continue;
            };
            let c = dp[j]
                .saturating_add(spec)
                .saturating_add(header_bits(i - j) as u32);
            if c < dp[i] {
                dp[i] = c;
                pred[i] = j as u8;
                pred_cb[i] = cb;
            }
        }
    }
    let mut i = n;
    while i > 0 {
        let j = usize::from(pred[i]);
        sfb_cb[lo + j..lo + i].fill(pred_cb[i]);
        i = j;
    }
}
