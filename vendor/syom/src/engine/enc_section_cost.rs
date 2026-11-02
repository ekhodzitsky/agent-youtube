//! Integer prefix costs for the unchanged greedy section merge decisions.
use super::{BOOKS, MAX_BANDS, UNREPRESENTABLE};

// At most 51 finite u32 costs fit below this marker. Its upper bits count
// unrepresentable bands; prefix subtraction preserves both parts exactly.
const MISSING: u64 = 1 << 40;

pub(super) struct CostPrefix {
    values: [[u64; BOOKS]; MAX_BANDS + 1],
}

impl CostPrefix {
    pub(super) fn new(coded: &[bool], bits: &[[u32; BOOKS]], n: usize) -> Self {
        assert!(n <= MAX_BANDS);
        let mut values = [[0; BOOKS]; MAX_BANDS + 1];
        let mut totals = [0; BOOKS];
        for b in 0..n {
            if !coded[b] {
                values[b + 1][1..].copy_from_slice(&totals[1..]);
                continue;
            }
            for ((total, value), &cost) in totals
                .iter_mut()
                .zip(values[b + 1].iter_mut())
                .zip(bits[b].iter())
                .skip(1)
            {
                *total += if cost == UNREPRESENTABLE {
                    MISSING
                } else {
                    u64::from(cost)
                };
                *value = *total;
            }
        }
        Self { values }
    }

    fn raw(&self, lo: usize, hi: usize, cb: usize) -> u64 {
        self.values[hi][cb] - self.values[lo][cb]
    }

    pub(super) fn range(&self, lo: usize, hi: usize, cb: usize) -> u32 {
        self.raw(lo, hi, cb).min(u64::from(u32::MAX)) as u32
    }

    pub(super) fn best(&self, lo: usize, hi: usize) -> Option<(u8, u32)> {
        let mut best = None;
        for cb in 1..BOOKS {
            let raw = self.raw(lo, hi, cb);
            if raw >= MISSING {
                continue;
            }
            let cost = raw.min(u64::from(u32::MAX)) as u32;
            if best.is_none_or(|(_, value)| cost < value) {
                best = Some((cb as u8, cost));
            }
        }
        best
    }
}

#[cfg(test)]
#[path = "enc_section_cost_tests.rs"]
mod tests;
