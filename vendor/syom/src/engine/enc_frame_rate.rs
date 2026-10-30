//! Rate loop + `raw_data_block` emission for [`LcEncoder`] — split from
//! `enc_frame.rs` for the line cap. A child module of `enc_frame`, so the
//! `impl` below keeps using the encoder's private fields; no new
//! abstraction boundary.

use super::{LcEncoder, MAX_BITS_PER_CHANNEL};
use crate::engine::enc_quant::{self, MAX_BANDS};
use crate::engine::enc_section;
use crate::engine::enc_short;
use crate::engine::swb::LONG_WINDOW_LEN;

/// Hard LC frame bit ceiling for `channels` (1 or 2).
#[must_use]
pub fn max_frame_bits(channels: usize) -> usize {
    MAX_BITS_PER_CHANNEL.saturating_mul(channels)
}

/// Highest target bitrate that still fits [`MAX_BITS_PER_CHANNEL`].
#[must_use]
pub fn max_bitrate_bps(sample_rate: u32, channels: usize) -> u32 {
    let bits = (MAX_BITS_PER_CHANNEL as u64)
        .saturating_mul(channels as u64)
        .saturating_mul(u64::from(sample_rate))
        / 1024;
    bits.min(u64::from(u32::MAX)) as u32
}

/// Rate-loop search bounds for the allowed-noise offset (0.75 dB of energy per step,
/// [`crate::engine::enc_alloc::noise_targets`]): negative refines every
/// band uniformly, positive raises the water level over the quietest
/// band; bits fall monotonically with the offset.
pub(super) const OFFSET_LO: i32 = -60;
const OFFSET_HI: i32 = 240;
/// How far a warm start walks before falling back to bisection.
const NEIGHBOR: i32 = 8;

#[cfg(test)]
thread_local! {
    static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Quantizations since the last [`take_builds`] (test threads only).
#[cfg(test)]
pub(super) fn take_builds() -> u32 {
    BUILDS.with(|c| c.replace(0))
}

#[cfg(test)]
fn note_build() {
    BUILDS.with(|c| c.set(c.get().saturating_add(1)));
}

impl LcEncoder {
    /// Per-band M/S (and optional IS skip) once per frame before the rate loop.
    pub(super) fn decide_stereo(
        &mut self,
        specs: &mut [[f32; LONG_WINDOW_LEN]; 2],
        psy_specs: &mut [[f32; LONG_WINDOW_LEN]; 2],
        long: bool,
    ) {
        self.ms = if self.channels == 2 {
            #[cfg(test)]
            let per_band = self.ms_per_band;
            #[cfg(not(test))]
            let per_band = true;
            if self.seq.is_eight_short() {
                self.is_band.fill(false);
                crate::engine::enc_ms::decide_short(
                    specs,
                    self.short_offsets,
                    per_band,
                    self.grouping,
                )
            } else {
                if self.intensity {
                    crate::engine::enc_is::plan(
                        specs,
                        self.offsets,
                        self.sample_rate,
                        &mut self.is_band,
                    );
                } else {
                    self.is_band.fill(false);
                }
                crate::engine::enc_ms::decide_long_skip(
                    specs,
                    self.offsets,
                    per_band,
                    &self.is_band,
                )
            }
        } else {
            crate::engine::enc_ms::MsBands::off()
        };
        if long && self.channels == 2 {
            self.ms.apply_long_to(psy_specs, self.offsets);
        }
    }

    pub(super) fn apply_tns(
        &mut self,
        specs: &mut [[f32; LONG_WINDOW_LEN]; 2],
        coded: &[[bool; crate::engine::enc_quant::MAX_BANDS]; 2],
        enabled: bool,
    ) {
        let offsets = if self.seq.is_eight_short() {
            self.short_offsets
        } else {
            self.offsets
        };
        self.tns = crate::engine::enc_tns::decide_frame(
            specs,
            self.channels,
            self.seq,
            offsets,
            self.fs_index,
            coded,
            enabled,
            self.short_tns,
        );
    }

    /// Noise offset for this frame. Starts at the previous frame's offset.
    pub(super) fn search_offset(
        &mut self,
        specs: &[[f32; LONG_WINDOW_LEN]; 2],
        spend: usize,
    ) -> i32 {
        self.search_offset_from(OFFSET_LO, specs, spend)
    }

    /// [`Self::search_offset`] over `[lo, OFFSET_HI]`.
    ///
    /// The previous offset is tried first. A hit walks down a short
    /// neighborhood; a miss walks up, then the remaining side is bisected.
    /// Bit counts dip by a few bits, so a start that is not the previous
    /// frame's offset can stop at a different crossing than a cold
    /// bisection. The first frame starts at [`OFFSET_LO`], which is that
    /// cold search.
    pub(super) fn search_offset_from(
        &mut self,
        lo_bound: i32,
        specs: &[[f32; LONG_WINDOW_LEN]; 2],
        spend: usize,
    ) -> i32 {
        let budget = spend.saturating_sub(32);
        let lo = lo_bound.clamp(OFFSET_LO, OFFSET_HI);
        let guess = self.rate_guess;
        let offset = search_monotonic(lo, OFFSET_HI, guess, |off| self.build(specs, off) <= budget);
        self.rate_guess = offset;
        offset
    }

    /// Test hook: force the next search to ignore the warm start.
    #[cfg(test)]
    pub(super) fn pin_rate_guess(&mut self, v: i32) {
        self.rate_guess = v;
    }

    /// Quantize + plan both channels at `offset`; returns total frame bits.
    /// Long windows draw their targets from the per-frame allocation cache
    /// (`enc_frame_alloc.rs`); `specs` are the coded, TNS-filtered spectra.
    pub(super) fn build(&mut self, specs: &[[f32; LONG_WINDOW_LEN]; 2], offset: i32) -> usize {
        #[cfg(test)]
        note_build();
        let standalone = self.channels == 1;
        let mut total = 3 + 4 + 3 + self.fill_bits(); // id + tag + FIL + END
        if self.seq.is_eight_short() {
            if self.channels == 2 {
                total += 1 + 15 + self.ms.overhead_bits(); // cw + ics_info + ms_mask
            }
            for (ch, spec) in specs.iter().enumerate().take(self.channels) {
                total += enc_short::channel_build(
                    &mut self.psy_short,
                    self.short_offsets,
                    spec,
                    offset,
                    &mut self.chans_s[ch],
                    &mut self.target_q_s[ch],
                    &mut self.books_s[ch],
                    &mut self.gains[ch],
                    standalone,
                    &self.tns[ch],
                    self.grouping,
                );
            }
            return total + 7; // byte-align pad ceiling
        }
        if self.channels == 2 {
            // common_window + ics_info + ms_mask
            total += 1 + 11 + self.ms.overhead_bits();
        }
        for (ch, spec) in specs.iter().enumerate().take(self.channels) {
            let q = &mut self.chans[ch];
            q.pns[..q.n_bands].fill(false);
            q.intensity[..q.n_bands].fill(false);
            let tq = &mut self.target_q[ch];
            let a = &self.alloc[ch];
            q.coded = a.coded;
            crate::engine::enc_alloc::noise_targets(
                &a.peaks,
                &a.energy,
                &a.sum_sqrt,
                &a.noise,
                &a.width,
                &a.cap,
                offset,
                q.n_bands,
                &mut q.coded,
                tq,
            );
            // TNS span bands are coded or dropped by the water level like
            // any other band. Forcing the span at target 1.0 blew the frame
            // budget where TNS fired hardest. Peaks were measured once.
            let peaks = a.peaks;
            Self::quant_one_long(
                self.offsets,
                q,
                tq,
                spec,
                &peaks,
                &mut self.books[ch],
                &mut self.gains[ch],
            );
        }
        if self.pns {
            crate::engine::enc_pns::apply_frame(
                specs,
                self.offsets,
                self.sample_rate,
                self.channels,
                &mut self.chans[..],
                &mut self.books,
                &self.gains,
                &self.ms,
                &self.tns,
            );
        }
        if self.intensity && self.channels == 2 {
            crate::engine::enc_is::stamp(
                specs,
                self.offsets,
                &self.is_band,
                &mut self.chans[..],
                &mut self.books,
            );
        }
        for ch in 0..self.channels {
            total += enc_section::channel_body_bits(
                &self.books[ch],
                &self.chans[ch],
                self.gains[ch],
                standalone,
                &self.tns[ch],
            );
        }
        total + 7 // byte-align pad ceiling
    }

    /// Quantize + section-plan one long channel from existing `coded` / `target_q`.
    fn quant_one_long(
        offsets: &[u16],
        q: &mut crate::engine::enc_quant::QuantChannel,
        tq: &[f32; MAX_BANDS],
        spec: &[f32; LONG_WINDOW_LEN],
        peaks: &[f32; MAX_BANDS],
        books: &mut [u8; MAX_BANDS],
        gain: &mut u8,
    ) {
        enc_quant::raw_scalefactors(peaks, tq, 0, q);
        *gain = enc_quant::normalize_sf(q);
        enc_quant::quantize_cached(spec, offsets, q);
        *books = enc_section::plan_books(q);
    }

    /// Emit the `raw_data_block` from the built state into `out`.
    /// Returns the bit length before `END`.
    pub(super) fn emit_into(&self, out: &mut Vec<u8>) -> usize {
        if self.seq.is_eight_short() {
            enc_short::emit_frame(
                self.short_offsets,
                &self.chans_s[..],
                &self.books_s[..],
                &self.gains,
                &self.ms,
                &self.tns,
                self.channels,
                &self.fill,
                self.pad_fill,
                out,
            )
        } else {
            enc_section::emit_frame(
                self.offsets,
                self.seq,
                &self.chans[..],
                &self.books[..],
                &self.gains,
                &self.ms,
                &self.tns,
                self.channels,
                &self.fill,
                self.pad_fill,
                out,
            )
        }
    }
}

/// Offset in `[lo_bound, hi_bound]` that `fits`, trying `guess` first.
///
/// When `fits` stays true once it becomes true, the result is the
/// smallest such offset (or `hi_bound` when nothing fits). A real bit
/// curve dips by a few bits, so a guess other than the previous frame's
/// offset can stop at a local crossing. `guess == lo_bound` is a full
/// bisection.
pub(crate) fn search_monotonic(
    lo_bound: i32,
    hi_bound: i32,
    guess: i32,
    mut fits: impl FnMut(i32) -> bool,
) -> i32 {
    let guess = guess.clamp(lo_bound, hi_bound);
    if guess > lo_bound {
        if fits(guess) {
            return search_down(lo_bound, guess, &mut fits);
        }
        return search_up(guess, hi_bound, &mut fits);
    }
    if fits(lo_bound) {
        return lo_bound;
    }
    if !fits(hi_bound) {
        return hi_bound;
    }
    bisect(lo_bound, hi_bound, &mut fits)
}

fn search_down(lo_bound: i32, guess: i32, fits: &mut impl FnMut(i32) -> bool) -> i32 {
    let mut hi = guess;
    let floor = lo_bound.max(guess - NEIGHBOR);
    while hi > floor {
        if !fits(hi - 1) {
            return hi;
        }
        hi -= 1;
    }
    if hi == lo_bound || fits(lo_bound) {
        return lo_bound;
    }
    bisect(lo_bound, hi, fits)
}

fn search_up(guess: i32, hi_bound: i32, fits: &mut impl FnMut(i32) -> bool) -> i32 {
    let mut lo = guess;
    let ceil = hi_bound.min(guess + NEIGHBOR);
    while lo < ceil {
        if fits(lo + 1) {
            return lo + 1;
        }
        lo += 1;
    }
    if lo >= hi_bound || !fits(hi_bound) {
        return hi_bound;
    }
    bisect(lo, hi_bound, fits)
}

fn bisect(mut lo: i32, mut hi: i32, fits: &mut impl FnMut(i32) -> bool) -> i32 {
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[cfg(test)]
#[path = "enc_frame_rate_tests.rs"]
mod enc_frame_rate_tests;
