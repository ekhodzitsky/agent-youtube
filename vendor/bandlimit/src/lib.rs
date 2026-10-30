//! Bandlimited rational resampling to planar `f32`.
//!
//! The public contract is not frozen. [`Rate`] is a positive integer sample
//! rate. [`Ratio`] is one
//! fixed fraction of two rates, stored in lowest terms. [`convert`] copies an
//! identity rate, rejects non-finite samples when the rates differ, and
//! resamples finite unequal rates as one [`Stream::feed`] plus one
//! [`Stream::finish`]. [`convert_planes`] is that call once per plane.
//! [`group_delay`] and [`output_len`] are the raw length
//! before that delay is removed.
//! [`table_bytes`] counts one polyphase table and rejects a size above
//! [`TABLE_BUDGET_BYTES`]. [`coefficient_blob`] rebuilds one Blackman-Harris
//! 2 table from a seed.
//!
//! # Guide
//!
//! 48_000 Hz to 16_000 Hz reduces to 3/1. Each input length is a multiple
//! of three. [`Rate::new`] rejects zero and every negative count. These
//! examples pass positive counts only.
//!
//! ## One-shot
//!
//! [`convert`] resamples one slice.
//!
//! ```
//! fn main() -> Result<(), bandlimit::Error> {
//!     fn rate(hz: i64) -> bandlimit::Rate {
//!         match bandlimit::Rate::new(hz) {
//!             Ok(rate) => rate,
//!             Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
//!         }
//!     }
//!
//!     let from = rate(48_000);
//!     let to = rate(16_000);
//!     let output = bandlimit::convert(&[0.0, 0.25, -0.25], from, to)?;
//!     assert_eq!(output.len(), 1);
//!     Ok(())
//! }
//! ```
//!
//! ## Feed and finish
//!
//! [`Stream::new`] fixes the ratio. [`Stream::feed`] takes a chunk.
//! [`Stream::finish`] returns what that feed has not already returned.
//!
//! ```
//! fn main() -> Result<(), bandlimit::Error> {
//!     fn rate(hz: i64) -> bandlimit::Rate {
//!         match bandlimit::Rate::new(hz) {
//!             Ok(rate) => rate,
//!             Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
//!         }
//!     }
//!
//!     let from = rate(48_000);
//!     let to = rate(16_000);
//!     let mut stream = bandlimit::Stream::new(from, to)?;
//!     let mut output = stream.feed(&[0.0, 0.25, -0.25])?;
//!     output.extend(stream.finish()?);
//!     assert_eq!(output.len(), 1);
//!     Ok(())
//! }
//! ```
//!
//! ## Two planes
//!
//! [`convert_planes`] resamples each plane on its own. The planes share
//! one length.
//!
//! ```
//! fn main() -> Result<(), bandlimit::Error> {
//!     fn rate(hz: i64) -> bandlimit::Rate {
//!         match bandlimit::Rate::new(hz) {
//!             Ok(rate) => rate,
//!             Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
//!         }
//!     }
//!
//!     let from = rate(48_000);
//!     let to = rate(16_000);
//!     let left = [0.0, 0.25, -0.25];
//!     let right = [0.5, 0.0, -0.5];
//!     let planes = bandlimit::convert_planes(&[&left, &right], from, to)?;
//!     assert_eq!(planes.len(), 2);
//!     assert_eq!(planes[0].len(), 1);
//!     assert_eq!(planes[1].len(), 1);
//!     Ok(())
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

use std::num::NonZeroU64;

mod coefficients;
mod oneshot;
mod stream;

#[cfg(test)]
mod length_grid;

pub use coefficients::coefficient_blob;
pub use stream::Stream;

/// A positive integer sample rate in hertz.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rate(NonZeroU64);

/// A sample rate that was zero or negative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateError {
    /// The count of hertz was zero or negative.
    NotPositive,
}

impl Rate {
    /// Accepts one positive count of hertz.
    ///
    /// Zero and every negative value return [`RateError::NotPositive`].
    #[must_use = "a rejected rate is an error"]
    pub fn new(hz: i64) -> Result<Self, RateError> {
        let hz = match u64::try_from(hz) {
            Ok(hz) => hz,
            Err(_) => return Err(RateError::NotPositive),
        };
        match NonZeroU64::new(hz) {
            Some(hz) => Ok(Self(hz)),
            None => Err(RateError::NotPositive),
        }
    }

    /// The rate in hertz.
    #[must_use]
    pub fn hz(self) -> u64 {
        self.0.get()
    }
}

/// One fixed rational ratio, in lowest terms.
///
/// `numerator / denominator` is the reduced form of the two rates passed to
/// [`Ratio::reduce`]. The first rate is the numerator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ratio {
    numerator: NonZeroU64,
    denominator: NonZeroU64,
}

impl Ratio {
    /// Reduces `numerator / denominator` by their greatest common divisor.
    #[must_use]
    pub fn reduce(numerator: Rate, denominator: Rate) -> Self {
        let (numerator, denominator) = reduced(numerator.0, denominator.0);
        Self {
            numerator,
            denominator,
        }
    }

    /// Numerator of the reduced fraction.
    #[must_use]
    pub fn numerator(self) -> u64 {
        self.numerator.get()
    }

    /// Denominator of the reduced fraction.
    #[must_use]
    pub fn denominator(self) -> u64 {
        self.denominator.get()
    }
}

/// A call failed before it returned an output buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The sample at `index` was NaN or an infinity.
    NonFinite { index: usize },
    /// A finite input produced a sample that is not a finite `f32`.
    ///
    /// [`convert`] returns this variant and no buffer. Equal rates copy NaN
    /// and infinity and do not use this variant. [`Stream::write`] returns
    /// it instead of appending that sample and does not roll the delay line
    /// back.
    Range,
    /// `planes` was empty, or the planes did not share one length.
    /// Nothing is returned.
    Planes,
    /// No output was written. A length that does not fit in `usize` uses
    /// this variant. [`group_delay`] and [`output_len`] use it for a zero
    /// phase count, a zero or even tap count, a product that does not fit
    /// in `u64`, and an input length the ratio does not divide.
    /// [`table_bytes`] uses it for zero taps, a product that does not fit
    /// in `u64`, and a table larger than [`TABLE_BUDGET_BYTES`]. [`convert`]
    /// and [`Stream::finish`] use it when the ideal output count is not exact
    /// or a length, origin, or allocation does not fit. [`coefficient_blob`]
    /// uses it when
    /// `taps * phases * 4` does not fit in `u64` or exceeds
    /// [`TABLE_BUDGET_BYTES`], and allocates nothing in that case.
    Overflow,
    /// The coefficient seed is not a Blackman-Harris 2 design this generator
    /// builds, `alpha` is not positive and finite, or a coefficient was not
    /// finite.
    Seed,
}

/// Integer group delay of the linear-phase stand-in, in output samples.
///
/// The delay is `(taps - 1) / 2`. Phase count and the ratio do not change
/// it. A zero phase count, a zero tap count, or an even tap count is
/// [`Error::Overflow`].
#[must_use = "a rejected delay is an error"]
pub fn group_delay(taps: u32, phase_count: u32, ratio: Ratio) -> Result<u64, Error> {
    // Phase count and the ratio do not change this stand-in. The design
    // task may replace the body and must keep these tests.
    let _ratio = ratio;
    if phase_count == 0 || taps == 0 || taps.is_multiple_of(2) {
        return Err(Error::Overflow);
    }
    Ok(u64::from((taps - 1) / 2))
}

/// Raw output length before the group delay is removed.
///
/// Ideal length is `input_len * denominator / numerator`, and only when
/// `numerator` divides that product exactly. [`group_delay`] is then added.
/// A remainder or a product that does not fit in `u64` is [`Error::Overflow`].
#[must_use = "a rejected length is an error"]
pub fn output_len(taps: u32, phase_count: u32, ratio: Ratio, input_len: u64) -> Result<u64, Error> {
    let delay = group_delay(taps, phase_count, ratio)?;
    let numerator = ratio.numerator();
    let scaled = match input_len.checked_mul(ratio.denominator()) {
        Some(scaled) => scaled,
        None => return Err(Error::Overflow),
    };
    let ideal = match scaled.checked_rem(numerator) {
        Some(0) => match scaled.checked_div(numerator) {
            Some(ideal) => ideal,
            None => return Err(Error::Overflow),
        },
        Some(_) | None => return Err(Error::Overflow),
    };
    match ideal.checked_add(delay) {
        Some(len) => Ok(len),
        None => Err(Error::Overflow),
    }
}

/// Largest polyphase coefficient table this crate will size, in bytes.
pub const TABLE_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

/// Bytes one polyphase table of `taps` would occupy for `ratio`.
///
/// The phase count is the larger of the reduced numerator and denominator.
/// Each coefficient is four bytes. Zero taps, a product that does not fit in
/// `u64`, or a product above [`TABLE_BUDGET_BYTES`] is [`Error::Overflow`].
/// Nothing is allocated.
#[must_use = "a rejected table size is an error"]
pub fn table_bytes(taps: u32, ratio: Ratio) -> Result<u64, Error> {
    if taps == 0 {
        return Err(Error::Overflow);
    }
    let phases = ratio.numerator().max(ratio.denominator());
    match u64::from(taps)
        .checked_mul(phases)
        .and_then(|coefficients| coefficients.checked_mul(4))
    {
        Some(bytes) if bytes <= TABLE_BUDGET_BYTES => Ok(bytes),
        Some(_) | None => Err(Error::Overflow),
    }
}

/// Resamples planar `input` from `from` to `to`.
///
/// An empty slice returns an empty buffer. Equal rates copy every sample,
/// including NaN and infinity. Unequal rates return [`Error::NonFinite`] for
/// the first non-finite sample and write nothing. A finite unequal pair is
/// one [`Stream::feed`] of `input` and one [`Stream::finish`]: the scalar
/// one-shot, with the integer group delay removed. The buffer length is the
/// ideal count when `from` divides `input_len * to` exactly. A length that
/// is not exact, or that does not fit, is [`Error::Overflow`]. So is a filter
/// whose retained coefficient rows exceed [`TABLE_BUDGET_BYTES`]. A
/// convolution that is not a finite `f32` is [`Error::Range`], and no buffer
/// is returned. The tap count is `2 * max(186, ceil(62 * from / to)) + 1`,
/// except 8_000 Hz to 16_000 Hz, which uses 33 taps, 32_000 Hz to 16_000 Hz,
/// which uses 339 taps, 48_000 Hz to 16_000 Hz, which uses 443 taps,
/// 44_100 Hz to 16_000 Hz, which uses 467 taps, 96_000 Hz to 16_000 Hz, which
/// uses 365 taps, and 192_000 Hz to 16_000 Hz, which uses 391 taps.
#[must_use = "a rejected conversion is an error"]
pub fn convert(input: &[f32], from: Rate, to: Rate) -> Result<Vec<f32>, Error> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if from.hz() == to.hz() {
        return Ok(input.to_vec());
    }
    if let Some(index) = first_non_finite(input) {
        return Err(Error::NonFinite { index });
    }
    oneshot::resample(input, from, to)
}

/// First sample that is not finite, in input order.
///
/// NaN and infinity are the `f32` values whose exponent field is all ones.
/// [`convert`] and [`Stream::write`] reject those before any output write.
/// Eight-wide loads keep that scan off the scalar path; a hit falls back to
/// [`f32::is_finite`] so the reported index is the first one.
#[cfg(target_arch = "aarch64")]
pub(crate) fn first_non_finite(input: &[f32]) -> Option<usize> {
    use std::arch::aarch64::{vandq_u32, vceqq_u32, vdupq_n_u32, vld1q_u32, vmaxvq_u32, vorrq_u32};
    let ptr = input.as_ptr();
    let len = input.len();
    let mut index = 0;
    while index + 8 <= len {
        // SAFETY: `index + 8` is inside `input`. NEON loads may be unaligned.
        // The mask keeps the eight exponent fields. A lane of all ones is
        // non-finite, which is the same test as `f32::is_finite`.
        let any = unsafe {
            let low = vld1q_u32(ptr.add(index).cast());
            let high = vld1q_u32(ptr.add(index + 4).cast());
            let exponent = vdupq_n_u32(0x7f80_0000);
            let bad_low = vceqq_u32(vandq_u32(low, exponent), exponent);
            let bad_high = vceqq_u32(vandq_u32(high, exponent), exponent);
            vmaxvq_u32(vorrq_u32(bad_low, bad_high))
        };
        if any != 0 {
            return input[index..index + 8]
                .iter()
                .position(|sample| !sample.is_finite())
                .map(|at| index + at);
        }
        index += 8;
    }
    input[index..]
        .iter()
        .position(|sample| !sample.is_finite())
        .map(|at| index + at)
}

#[cfg(not(target_arch = "aarch64"))]
pub(crate) fn first_non_finite(input: &[f32]) -> Option<usize> {
    input.iter().position(|sample| !sample.is_finite())
}

/// Resamples each plane from `from` to `to`.
///
/// Each plane is one independent [`convert`]. Samples stay in that plane.
/// An empty `planes` slice, or planes that do not share one length, is
/// [`Error::Planes`] and nothing is returned. One empty plane is an empty
/// buffer, as [`convert`] is for an empty slice. An error from [`convert`]
/// is returned unchanged, and nothing is returned.
#[must_use = "a rejected conversion is an error"]
pub fn convert_planes(planes: &[&[f32]], from: Rate, to: Rate) -> Result<Vec<Vec<f32>>, Error> {
    let len = match planes.first() {
        Some(plane) => plane.len(),
        None => return Err(Error::Planes),
    };
    for plane in planes.iter().skip(1) {
        if plane.len() != len {
            return Err(Error::Planes);
        }
    }
    let mut output = Vec::with_capacity(planes.len());
    for plane in planes {
        output.push(convert(plane, from, to)?);
    }
    Ok(output)
}

/// Greatest common divisor. A zero argument yields the other argument.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn reduced(numerator: NonZeroU64, denominator: NonZeroU64) -> (NonZeroU64, NonZeroU64) {
    // Two positive integers have a positive gcd that divides both. If a
    // quotient were ever zero, the original pair is still the same ratio.
    let divisor = match NonZeroU64::new(gcd(numerator.get(), denominator.get())) {
        Some(divisor) => divisor,
        None => return (numerator, denominator),
    };
    match (
        NonZeroU64::new(numerator.get() / divisor.get()),
        NonZeroU64::new(denominator.get() / divisor.get()),
    ) {
        (Some(numerator), Some(denominator)) => (numerator, denominator),
        _ => (numerator, denominator),
    }
}
