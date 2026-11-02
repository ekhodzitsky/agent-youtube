//! Hoist a band's fixed codebook choices out of the spectral tuple loop.
use super::*;

/// Emit all complete tuples in one band; invalid tuples emit nothing, as
/// [`spectral_emit`] does. AAC callers pass complete tuple-aligned bands.
pub fn spectral_emit_band(cb: u8, values: &[i32], writer: &mut BitWriter) {
    match cb {
        1 => emit::<1>(values, writer),
        2 => emit::<2>(values, writer),
        3 => emit::<3>(values, writer),
        4 => emit::<4>(values, writer),
        5 => emit::<5>(values, writer),
        6 => emit::<6>(values, writer),
        7 => emit::<7>(values, writer),
        8 => emit::<8>(values, writer),
        9 => emit::<9>(values, writer),
        10 => emit::<10>(values, writer),
        11 => emit::<11>(values, writer),
        _ => {}
    }
}

fn emit<const CB: u8>(values: &[i32], writer: &mut BitWriter) {
    let Some((lengths, codes)) = tables(CB) else {
        return;
    };
    let width = if CB <= 4 { 4 } else { 2 };
    for tuple in values.chunks_exact(width) {
        let Some(index) = index::<CB>(tuple) else {
            continue;
        };
        let Some((&length, &code)) = lengths.get(index).zip(codes.get(index)) else {
            continue;
        };
        let mut word = u32::from(code);
        let mut bits = u32::from(length);
        if unsigned_book(CB) {
            for &value in tuple {
                if value != 0 {
                    word = (word << 1) | u32::from(value < 0);
                    bits += 1;
                }
            }
        }
        // The longest code plus all tuple signs fits a u32 (tested against
        // every table). Emit that exact concatenation with one field write.
        debug_assert!(bits <= 32);
        writer.write(word, bits);
        if CB == 11 {
            for &value in tuple {
                let magnitude = value.unsigned_abs();
                if magnitude >= 16 {
                    let n = 31 - magnitude.leading_zeros();
                    if n <= 17 {
                        // (n-4) one bits, a stop zero, and n offset bits:
                        // 2*n-3 <= 31 for this fast path. Larger arbitrary
                        // magnitudes retain the generic sequence below.
                        let prefix = ((1u32 << (n - 4)) - 1) << (n + 1);
                        writer.write(prefix | (magnitude - (1 << n)), 2 * n - 3);
                    } else {
                        for _ in 4..n {
                            writer.write_bit(true);
                        }
                        writer.write_bit(false);
                        writer.write(magnitude - (1 << n), n);
                    }
                }
            }
        }
    }
}

/// Same checked domain and signed arithmetic as `tuple_index`, with the
/// fixed book available to constant propagation at each specialized loop.
#[inline]
fn index<const CB: u8>(tuple: &[i32]) -> Option<usize> {
    let limit = match CB {
        1 | 2 => 1,
        3 | 4 => 2,
        5 | 6 => 4,
        7 | 8 => 7,
        9 | 10 => 12,
        _ => i32::MAX,
    };
    if CB != 11 && tuple.iter().any(|value| value.abs() > limit) {
        return None;
    }
    Some(match CB {
        1 | 2 => {
            ((tuple[0] + 1) * 27 + (tuple[1] + 1) * 9 + (tuple[2] + 1) * 3 + tuple[3] + 1) as usize
        }
        3 | 4 => {
            (tuple[0].abs() * 27 + tuple[1].abs() * 9 + tuple[2].abs() * 3 + tuple[3].abs())
                as usize
        }
        5 | 6 => ((tuple[0] + 4) * 9 + tuple[1] + 4) as usize,
        7 | 8 => (tuple[0].abs() * 8 + tuple[1].abs()) as usize,
        9 | 10 => (tuple[0].abs() * 13 + tuple[1].abs()) as usize,
        11 => (tuple[0].abs().min(16) * 17 + tuple[1].abs().min(16)) as usize,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "enc_huff_band_tests.rs"]
mod tests;
