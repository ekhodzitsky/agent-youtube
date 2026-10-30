//! Autocorrelation and the TNS analysis filter. Same f64 products and the
//! same left-to-right add order as the straight loops; the inner indexes
//! are slice iterators so the rate path does not bounds-check each tap.

use super::super::swb::LONG_WINDOW_LEN;
use super::MAX_ORDER;

/// Autocorrelation of `x` at lags 0..=MAX_ORDER (f64 sums, fixed order).
pub(super) fn autocorrelate(x: &[f32]) -> [f64; MAX_ORDER + 1] {
    if x.len() > LONG_WINDOW_LEN {
        return autocorrelate_long(x);
    }
    let n = x.len();
    let mut xd = [0.0f64; LONG_WINDOW_LEN];
    for (d, &v) in xd[..n].iter_mut().zip(x.iter()) {
        *d = f64::from(v);
    }
    let mut r = [0.0f64; MAX_ORDER + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        if lag >= n {
            break;
        }
        let mut acc = 0.0f64;
        for (&u, &v) in xd[..n - lag].iter().zip(xd[lag..n].iter()) {
            acc += u * v;
        }
        *slot = acc;
    }
    r
}

fn autocorrelate_long(x: &[f32]) -> [f64; MAX_ORDER + 1] {
    let mut r = [0.0f64; MAX_ORDER + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        let mut acc = 0.0f64;
        for i in 0..x.len() - lag {
            acc += f64::from(x[i]) * f64::from(x[i + lag]);
        }
        *slot = acc;
    }
    r
}

/// Analysis (whitening) filter, upward: `e[n] = x[n] + Σ aₖ·x[n−k]`.
/// f64 accumulator, fixed op order. `a` is at most [`MAX_ORDER`] taps.
pub(super) fn analysis_filter(x: &mut [f32], orig: &[f32], a: &[f64]) {
    let n = x.len();
    let order = a.len().min(MAX_ORDER);
    let mut ak = [0.0f64; MAX_ORDER];
    ak[..order].copy_from_slice(&a[..order]);
    match order {
        0 => {
            for (slot, &s) in x.iter_mut().zip(orig.iter()).take(n) {
                *slot = f64::from(s) as f32;
            }
        }
        1 => fir::<1>(x, orig, &ak),
        2 => fir::<2>(x, orig, &ak),
        3 => fir::<3>(x, orig, &ak),
        4 => fir::<4>(x, orig, &ak),
        5 => fir::<5>(x, orig, &ak),
        6 => fir::<6>(x, orig, &ak),
        7 => fir::<7>(x, orig, &ak),
        8 => fir::<8>(x, orig, &ak),
        9 => fir::<9>(x, orig, &ak),
        10 => fir::<10>(x, orig, &ak),
        11 => fir::<11>(x, orig, &ak),
        _ => fir::<12>(x, orig, &ak),
    }
}

fn fir<const ORDER: usize>(x: &mut [f32], orig: &[f32], ak: &[f64; MAX_ORDER]) {
    let n = x.len().min(orig.len());
    let head = ORDER.min(n);
    for i in 0..head {
        let mut e = f64::from(orig[i]);
        for j in 0..i {
            e += ak[j] * f64::from(orig[i - 1 - j]);
        }
        x[i] = e as f32;
    }
    if n <= ORDER {
        return;
    }
    let mut hist = [0.0f64; MAX_ORDER];
    for j in 0..ORDER {
        hist[j] = f64::from(orig[ORDER - 1 - j]);
    }
    for i in ORDER..n {
        let mut e = f64::from(orig[i]);
        for j in 0..ORDER {
            e += ak[j] * hist[j];
        }
        x[i] = e as f32;
        for j in (1..ORDER).rev() {
            hist[j] = hist[j - 1];
        }
        hist[0] = f64::from(orig[i]);
    }
}
