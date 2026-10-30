//! One fixed ratio, fed in chunks.
//!
//! Converting [`Stream::feed`] keeps finite samples and returns nothing.
//! [`Stream::finish`] is one one-shot of those samples and returns
//! only what feed has not already returned. A second finish is empty.
//! Equal rates copy in `feed` and do not build a filter.
//!
//! [`Stream::write`] appends into a caller-owned buffer. The coefficient
//! table and the delay line are built on the first converting write. Later
//! writes do not allocate when that buffer has spare capacity.

use crate::{Error, Rate, Ratio, group_delay, oneshot, output_len};

use crate::coefficients::{PHASES, STAGE_192_16, Table, phase_count, taps_for};
#[cfg(test)]
const TAP_COUNT: usize = crate::coefficients::TAPS as usize;
const PHASE_COUNT: usize = PHASES as usize;
const CASCADE_BATCH: usize = 256;

/// One fixed-ratio stream of planar `f32`.
///
/// [`Stream::feed`] accepts any chunk length on one instance.
/// [`Stream::finish`] drains the tail once. [`Stream::write`] appends
/// samples as they are ready. Equal rates copy each chunk and do not build
/// a filter.
pub struct Stream {
    from: Rate,
    to: Rate,
    kept: Vec<f32>,
    /// Converting output samples already returned to the caller.
    emitted: usize,
    finished: bool,
    /// `feed` stored a converting chunk. `write` then returns overflow.
    via_feed: bool,
    /// `write` stored a converting chunk. `feed` then returns overflow.
    via_write: bool,
    engine: Option<Engine>,
}

impl Stream {
    /// Accepts any positive rates.
    ///
    /// The coefficient table is not allocated.
    #[must_use = "a rejected stream is an error"]
    pub fn new(from: Rate, to: Rate) -> Result<Self, Error> {
        Ok(Self {
            from,
            to,
            kept: Vec::new(),
            emitted: 0,
            finished: false,
            via_feed: false,
            via_write: false,
            engine: None,
        })
    }

    /// Accepts one chunk.
    ///
    /// Equal rates copy `input`, including NaN and infinity. An empty chunk
    /// returns an empty buffer. Unequal rates append finite samples and
    /// return an empty buffer; the tail stays until [`Stream::finish`]. The
    /// first non-finite sample in an unequal chunk is [`Error::NonFinite`]
    /// and none of that chunk is kept. A converting feed after `finish` is
    /// [`Error::Overflow`] and keeps nothing from that chunk.
    #[must_use = "a rejected chunk is an error"]
    pub fn feed(&mut self, input: &[f32]) -> Result<Vec<f32>, Error> {
        if self.from.hz() == self.to.hz() {
            return Ok(input.to_vec());
        }
        if self.finished || self.via_write {
            return Err(Error::Overflow);
        }
        for (index, sample) in input.iter().enumerate() {
            if !sample.is_finite() {
                return Err(Error::NonFinite { index });
            }
        }
        self.kept.extend_from_slice(input);
        if !input.is_empty() {
            self.via_feed = true;
        }
        Ok(Vec::new())
    }

    /// Appends converted samples from `input` to `output`.
    ///
    /// The caller owns `output`. Equal rates append `input`, including NaN
    /// and infinity. Unequal rates append every finite sample whose taps lie
    /// inside the input received so far, without the integer group delay.
    /// Samples that still need a later input stay until [`Stream::finish`].
    ///
    /// The first converting call builds the coefficient table and the delay
    /// line. A later call does not allocate when those already exist and
    /// `output` has spare capacity for every sample this chunk produces. If
    /// that spare room is short, nothing is appended, the stream keeps none
    /// of `input`, and the result is [`Error::Overflow`]. `output`'s
    /// capacity is left unchanged either way. A non-finite unequal sample is
    /// [`Error::NonFinite`] and keeps nothing. A convolution that is not a
    /// finite `f32` is [`Error::Range`]: that sample is not appended, and the
    /// delay line is not rolled back. A converting write after `finish` is
    /// [`Error::Overflow`] and keeps nothing.
    #[must_use = "a rejected chunk is an error"]
    pub fn write(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        if self.from.hz() == self.to.hz() {
            return self.write_copy(input, output);
        }
        self.write_convert(input, output)
    }

    /// Returns converting samples not already returned.
    ///
    /// Unequal [`Stream::feed`] emits one one-shot of the samples kept
    /// so far, without the integer group delay. Unequal [`Stream::write`]
    /// emits the tail that still needed zeros past the last input. A second
    /// call is empty and does not append zeros. Equal rates return an empty
    /// buffer and do not build a filter.
    #[must_use = "a rejected drain is an error"]
    pub fn finish(&mut self) -> Result<Vec<f32>, Error> {
        if self.from.hz() == self.to.hz() {
            return Ok(Vec::new());
        }
        if self.via_write {
            return self.drain();
        }
        if self.finished {
            return Ok(Vec::new());
        }
        if self.kept.is_empty() {
            if self.emitted != 0 {
                return Err(Error::Overflow);
            }
            self.finished = true;
            return Ok(Vec::new());
        }
        // Buffered feed samples are resampled only when finishing.
        let mut full = oneshot::resample(&self.kept, self.from, self.to)?;
        if self.emitted > full.len() {
            return Err(Error::Overflow);
        }
        // `split_off` panics when the index is past the end. The check above
        // keeps that off the production path.
        let suffix = if self.emitted == 0 {
            full
        } else {
            full.split_off(self.emitted)
        };
        self.emitted = match self.emitted.checked_add(suffix.len()) {
            Some(emitted) => emitted,
            None => return Err(Error::Overflow),
        };
        self.finished = true;
        Ok(suffix)
    }

    fn write_copy(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        let len = output.len();
        let cap = output.capacity();
        if len > cap {
            return Err(Error::Overflow);
        }
        if input.len() > cap - len {
            return Err(Error::Overflow);
        }
        // Spare capacity was checked. `extend_from_slice` does not allocate.
        output.extend_from_slice(input);
        Ok(())
    }

    fn write_convert(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        if self.finished || self.via_feed {
            return Err(Error::Overflow);
        }
        if let Some(index) = crate::first_non_finite(input) {
            return Err(Error::NonFinite { index });
        }
        self.ensure_engine()?;
        let mark = !input.is_empty();
        let Some(engine) = self.engine.as_mut() else {
            return Err(Error::Overflow);
        };
        if engine.filled != 0 {
            engine.ready_tail = None;
        }
        let count = engine.outputs_for(input.len())?;
        let len = output.len();
        let cap = output.capacity();
        if len > cap || count > cap - len {
            return Err(Error::Overflow);
        }
        // A fresh stream emits the ready span with the one-shot kernels.
        // The walk below then keeps only the history `finish` still needs.
        if engine.filled == 0 && engine.delay.is_empty() && count > 0 {
            engine.emit_prefix(input, output, self.from, self.to, count)?;
        }
        engine.accept(input, output)?;
        if mark {
            self.via_write = true;
        }
        Ok(())
    }

    fn ensure_engine(&mut self) -> Result<(), Error> {
        if self.engine.is_some() {
            return Ok(());
        }
        self.engine = Some(Engine::build(self.from, self.to)?);
        Ok(())
    }

    fn drain(&mut self) -> Result<Vec<f32>, Error> {
        if self.finished {
            return Ok(Vec::new());
        }
        let from = self.from;
        let to = self.to;
        let (accepted, next_n) = match self.engine.as_ref() {
            Some(engine) => (engine.accepted, engine.next_n),
            None => {
                self.finished = true;
                return Ok(Vec::new());
            }
        };
        let raw = raw_output_len(accepted, from, to)?;
        if next_n > raw {
            return Err(Error::Overflow);
        }
        let count = match usize::try_from(raw - next_n) {
            Ok(count) => count,
            Err(_) => return Err(Error::Overflow),
        };
        let stashed = match self.engine.as_ref() {
            Some(engine) => match engine.ready_tail.as_ref() {
                Some(tail) => Some(tail.len()),
                None => None,
            },
            None => None,
        };
        if let Some(stashed) = stashed {
            if stashed != count {
                return Err(Error::Overflow);
            }
            let Some(engine) = self.engine.as_mut() else {
                return Err(Error::Overflow);
            };
            let Some(tail) = engine.ready_tail.take() else {
                return Err(Error::Overflow);
            };
            engine.next_n = raw;
            self.finished = true;
            return Ok(tail);
        }
        let mut tail = Vec::new();
        if count > 0 && tail.try_reserve_exact(count).is_err() {
            return Err(Error::Overflow);
        }
        self.pull_tail(raw, &mut tail)?;
        self.finished = true;
        Ok(tail)
    }

    fn pull_tail(&mut self, raw: u64, tail: &mut Vec<f32>) -> Result<(), Error> {
        let Some(engine) = self.engine.as_mut() else {
            return Err(Error::Overflow);
        };
        if engine.cascade().is_some() {
            return engine.pull_cascade(raw, tail);
        }
        while engine.next_n < raw {
            let place = placement(
                engine.next_n,
                engine.in_rate,
                engine.out_rate,
                engine.coeffs.taps,
            )?;
            engine.discard_before(place.origin.max(0));
            let sample = compute(engine, &place)?;
            if tail.len() == tail.capacity() {
                return Err(Error::Overflow);
            }
            tail.push(sample);
            engine.next_n = match engine.next_n.checked_add(1) {
                Some(next) => next,
                None => return Err(Error::Overflow),
            };
        }
        Ok(())
    }
}

struct Engine {
    coeffs: Table,
    delay: Vec<f32>,
    /// Absolute input index of `delay[0]`.
    delay_pos: i64,
    /// Samples accepted so far. Also the exclusive end of `delay`.
    filled: i64,
    /// Next raw output index. Starts at the group delay, which is not returned.
    next_n: u64,
    /// Tail from a fresh chunk's one-shot, including zeros past that chunk.
    /// [`Stream::finish`] returns it. A later write drops it.
    ready_tail: Option<Vec<f32>>,
    accepted: u64,
    in_rate: u64,
    out_rate: u64,
}

impl Engine {
    fn build(from: Rate, to: Rate) -> Result<Self, Error> {
        let ratio = Ratio::reduce(from, to);
        let taps = taps_for(from, to)?;
        let group = group_delay(taps, PHASES, ratio)?;
        let coeffs = Table::new(from, to)?;
        let reserve = match (from.hz(), to.hz()) {
            (96_000, 16_000) => oneshot::cascade_history(oneshot::CascadeKind::Hz96) + 1,
            (192_000, 16_000) => oneshot::cascade_history(oneshot::CascadeKind::Hz192) + 1,
            _ => taps as usize + 1,
        };
        let mut delay = Vec::new();
        if delay.try_reserve_exact(reserve).is_err() {
            return Err(Error::Overflow);
        }
        Ok(Self {
            coeffs,
            delay,
            delay_pos: 0,
            filled: 0,
            next_n: group,
            ready_tail: None,
            accepted: 0,
            in_rate: ratio.numerator(),
            out_rate: ratio.denominator(),
        })
    }

    fn cascade(&self) -> Option<oneshot::CascadeKind> {
        oneshot::cascade_classify(self.in_rate, self.out_rate, self.coeffs.taps)
    }

    /// Ready outputs of a fresh stream, from the one-shot kernels.
    ///
    /// When `output` has room for the whole convert length, that call also
    /// keeps the zero tail for `finish`. Otherwise only the ready prefix is
    /// written and `finish` dots the tail. The cursor advances only after the
    /// ready samples are in `output`. History is left for [`Engine::accept`].
    fn emit_prefix(
        &mut self,
        input: &[f32],
        output: &mut Vec<f32>,
        from: Rate,
        to: Rate,
        count: usize,
    ) -> Result<(), Error> {
        let room = match output.capacity().checked_sub(output.len()) {
            Some(room) => room,
            None => return Err(Error::Overflow),
        };
        if count > room {
            return Err(Error::Overflow);
        }
        let first = match usize::try_from(self.next_n) {
            Ok(first) => first,
            Err(_) => return Err(Error::Overflow),
        };
        let add = match u64::try_from(count) {
            Ok(add) => add,
            Err(_) => return Err(Error::Overflow),
        };
        let next = match self.next_n.checked_add(add) {
            Some(next) => next,
            None => return Err(Error::Overflow),
        };
        let input_len = match u64::try_from(input.len()) {
            Ok(len) => len,
            Err(_) => return Err(Error::Overflow),
        };
        let ratio = Ratio::reduce(from, to);
        // A chunk whose length the ratio does not divide has no single raw
        // length. Finish still dots that tail. An exact length can keep it.
        let exact = match input_len.checked_mul(ratio.denominator()) {
            Some(scaled) => scaled % ratio.numerator() == 0,
            None => false,
        };
        let raw = if exact {
            match raw_output_len(input_len, from, to) {
                Ok(raw) => match usize::try_from(raw) {
                    Ok(raw) => Some(raw),
                    Err(_) => return Err(Error::Overflow),
                },
                Err(err) => return Err(err),
            }
        } else {
            None
        };
        let span = raw.and_then(|raw| raw.checked_sub(first));
        let start = output.len();
        if let (Some(raw), Some(span)) = (raw, span)
            && count <= span
            && span <= room
        {
            output.resize(start + span, 0.0);
            if let Err(err) =
                oneshot::fill_prefix_into(input, from, to, first, raw, &mut output[start..])
            {
                output.truncate(start);
                return Err(err);
            }
            self.ready_tail = Some(output.split_off(start + count));
        } else {
            let end = match first.checked_add(count) {
                Some(end) => end,
                None => return Err(Error::Overflow),
            };
            output.resize(start + count, 0.0);
            if let Err(err) =
                oneshot::fill_prefix_into(input, from, to, first, end, &mut output[start..])
            {
                output.truncate(start);
                return Err(err);
            }
        }
        self.next_n = next;
        Ok(())
    }

    /// Outputs this extra input would release. Does not touch the delay line.
    fn outputs_for(&self, extra: usize) -> Result<usize, Error> {
        if let Some(kind) = self.cascade() {
            return self.cascade_outputs(kind, extra);
        }
        let extra = match i64::try_from(extra) {
            Ok(extra) => extra,
            Err(_) => return Err(Error::Overflow),
        };
        let available = match self.filled.checked_add(extra) {
            Some(available) => available,
            None => return Err(Error::Overflow),
        };
        let taps = self.coeffs.taps;
        let first = placement(self.next_n, self.in_rate, self.out_rate, taps)?;
        if last_index(&first, taps as usize)? >= available {
            return Ok(0);
        }
        if extra <= 1 && self.in_rate >= self.out_rate {
            // A caught-up downsampler releases at most one sample here.
            // Check the endpoint too, so malformed cursors retain the
            // same rejection and single-sample calls avoid inverse division.
            let next = self.next_n.checked_add(1).ok_or(Error::Overflow)?;
            let terminal = placement(next, self.in_rate, self.out_rate, taps)?;
            if last_index(&terminal, taps as usize)? >= available {
                return Ok(1);
            }
        }
        let half = i128::from((taps - 1) / 2);
        let input = i128::from(self.in_rate);
        let output = i128::from(self.out_rate);
        if input == 0 || output == 0 {
            return Err(Error::Overflow);
        }
        // For odd support, z = (n - half) * input / output. Reading the
        // wrapped interpolation row adds one input sample exactly when
        // frac(z) > (phases - 1) / phases. An exact phase grid never wraps.
        // Its last tap is origin + taps - 1, so ready outputs satisfy
        // (n - half) * input <= (available - half) * output - 1, which is
        // the whole term below plus (output - 1). Otherwise ready outputs
        // satisfy
        // (n - half) * input <= (available - half - 1) * output
        //                         + floor((phases - 1) * output / phases).
        let whole = i128_or_overflow((i128::from(available) - half - 1).checked_mul(output))?;
        let phases = i128::from(phase_count(self.in_rate, self.out_rate));
        let fraction = if phases == output * 2 {
            output - 1
        } else {
            output * (phases - 1) / phases
        };
        let limit = i128_or_overflow(whole.checked_add(fraction))?;
        let stop = i128_or_overflow(limit.div_euclid(input).checked_add(half + 1))?;
        let stop = u64::try_from(stop).map_err(|_| Error::Overflow)?;
        // The previous walk also checked the first non-ready placement.
        // Preserve its overflow rejection without walking every output.
        let terminal = placement(stop, self.in_rate, self.out_rate, taps)?;
        last_index(&terminal, taps as usize)?;
        let count = stop.checked_sub(self.next_n).ok_or(Error::Overflow)?;
        usize::try_from(count).map_err(|_| Error::Overflow)
    }

    fn cascade_outputs(&self, kind: oneshot::CascadeKind, extra: usize) -> Result<usize, Error> {
        let extra = i64::try_from(extra).map_err(|_| Error::Overflow)?;
        let available = self.filled.checked_add(extra).ok_or(Error::Overflow)?;
        let largest = oneshot::cascade_largest_ready(kind, available)?;
        let next = i64::try_from(self.next_n).map_err(|_| Error::Overflow)?;
        if largest < next {
            return Ok(0);
        }
        let count = largest
            .checked_sub(next)
            .and_then(|gap| gap.checked_add(1))
            .ok_or(Error::Overflow)?;
        usize::try_from(count).map_err(|_| Error::Overflow)
    }

    fn accept(&mut self, mut input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        if self.cascade().is_some() {
            return self.accept_cascade(input, output);
        }
        // Nothing ready: keep the tail and return. Packing the 48 kHz row
        // first would do the work and then discard it.
        let place = placement(self.next_n, self.in_rate, self.out_rate, self.coeffs.taps)?;
        let end = last_index(&place, self.coeffs.taps as usize)?;
        if let Ok(pending) = i64::try_from(input.len())
            && let Some(available) = self.filled.checked_add(pending)
            && end >= available
        {
            self.store_remaining(&mut input, place.origin.max(0))?;
            self.debug_window();
            return Ok(());
        }
        let chunk = input;
        let chunk_start = self.filled;
        // One pack of this chunk. Eight outputs then use the same residue
        // kernel as the one-shot. A window that starts before the chunk, or
        // runs past it, stays on the four-lane dot below.
        let block = if self.in_rate == 3
            && self.out_rate == 1
            && self.coeffs.taps == 443
            && !chunk.is_empty()
        {
            match usize::try_from(chunk_start) {
                Ok(start) => {
                    let row = self.coeffs.row(0)?;
                    oneshot::fast_48(chunk, start, row).ok()
                }
                Err(_) => None,
            }
        } else {
            None
        };
        let stride = usize::try_from(self.in_rate).ok();
        let batch_window = stride
            .and_then(|step| step.checked_mul(3))
            .and_then(|span| span.checked_add(self.coeffs.taps as usize));
        loop {
            let place = placement(self.next_n, self.in_rate, self.out_rate, self.coeffs.taps)?;
            let end = last_index(&place, self.coeffs.taps as usize)?;
            let pending = match i64::try_from(input.len()) {
                Ok(pending) => pending,
                Err(_) => return Err(Error::Overflow),
            };
            let available = match self.filled.checked_add(pending) {
                Some(available) => available,
                None => return Err(Error::Overflow),
            };
            if end >= available {
                self.store_remaining(&mut input, place.origin.max(0))?;
                break;
            }
            if self.out_rate == 1
                && place.phase == 0
                && place.frac == 0.0
                && place.origin >= chunk_start
                && output.capacity() - output.len() >= 8
                && let Some(block) = block.as_ref()
                && let Ok(n) = usize::try_from(self.next_n)
                && let Ok(cursor) = i64::try_from(n)
                && let Some(origin) = cursor
                    .checked_mul(3)
                    .and_then(|scaled| scaled.checked_sub(884))
                && place.origin == origin
                && let Some(samples) = block.eight(n)?
            {
                let next = self.next_n.checked_add(8).ok_or(Error::Overflow)?;
                output.extend_from_slice(&samples);
                self.next_n = next;
                continue;
            }
            // Read complete integer windows directly from caller storage. The
            // history is updated afterwards for the first incomplete window.
            if self.out_rate == 1
                && place.origin >= chunk_start
                && let Ok(start) = usize::try_from(place.origin - chunk_start)
                && let Some(end) = batch_window.and_then(|len| start.checked_add(len))
                && let Some(window) = chunk.get(start..end)
                && let Some(stride) = stride
            {
                let next = self.next_n.checked_add(4).ok_or(Error::Overflow)?;
                let row = self.coeffs.row(0)?;
                if output.capacity() - output.len() < 4 {
                    return Err(Error::Overflow);
                }
                let samples = oneshot::four_outputs(window, row, stride)?;
                output.extend_from_slice(&samples);
                self.next_n = next;
                continue;
            }
            let until = match end.checked_add(1) {
                Some(until) => until,
                None => return Err(Error::Overflow),
            };
            self.skip_before(&mut input, place.origin.max(0))?;
            self.discard_before(place.origin.max(0));
            self.ingest_until(&mut input, until)?;
            if self.filled <= end {
                return Err(Error::Overflow);
            }
            let sample = compute(self, &place)?;
            if output.len() == output.capacity() {
                return Err(Error::Overflow);
            }
            output.push(sample);
            self.next_n = match self.next_n.checked_add(1) {
                Some(next) => next,
                None => return Err(Error::Overflow),
            };
        }
        self.debug_window();
        Ok(())
    }

    fn accept_cascade(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        let kind = self.cascade().ok_or(Error::Overflow)?;
        let mut stage1_buf = [0.0_f32; STAGE_192_16 as usize];
        let stage1_len = copy_stage1(&self.coeffs, &mut stage1_buf)?;
        let stage2_owned = oneshot::cascade_stage2_table()?;
        let stage2 = stage2_owned.row(0)?;
        let stage1 = &stage1_buf[..stage1_len];
        let chunk_start = self.filled;
        let pending = i64::try_from(input.len()).map_err(|_| Error::Overflow)?;
        let available = chunk_start.checked_add(pending).ok_or(Error::Overflow)?;
        let largest = oneshot::cascade_largest_ready(kind, available)?;
        let mut next = i64::try_from(self.next_n).map_err(|_| Error::Overflow)?;
        if largest >= next {
            let count = largest
                .checked_sub(next)
                .and_then(|gap| gap.checked_add(1))
                .ok_or(Error::Overflow)?;
            let total = usize::try_from(count).map_err(|_| Error::Overflow)?;
            let mut done = 0_usize;
            while done < total {
                let take = (total - done).min(CASCADE_BATCH);
                let room = output
                    .capacity()
                    .checked_sub(output.len())
                    .ok_or(Error::Overflow)?;
                if take > room {
                    return Err(Error::Overflow);
                }
                let n0 = usize::try_from(next).map_err(|_| Error::Overflow)?;
                let mut batch = [0.0_f32; CASCADE_BATCH];
                let delay_pos = self.delay_pos;
                {
                    let delay = self.delay.as_slice();
                    oneshot::cascade_write(
                        kind,
                        stage1,
                        stage2,
                        n0,
                        take,
                        &mut batch[..take],
                        |index| span_sample(delay, delay_pos, input, chunk_start, index),
                    )?;
                }
                output.extend_from_slice(&batch[..take]);
                let step = i64::try_from(take).map_err(|_| Error::Overflow)?;
                next = next.checked_add(step).ok_or(Error::Overflow)?;
                self.next_n = u64::try_from(next).map_err(|_| Error::Overflow)?;
                done += take;
            }
        }
        // Read every ready window first. One store then counts the chunk once.
        let keep = oneshot::cascade_first_input(kind, self.next_n)?.max(0);
        let mut rest = input;
        self.store_remaining(&mut rest, keep)?;
        self.debug_window();
        Ok(())
    }

    fn pull_cascade(&mut self, raw: u64, tail: &mut Vec<f32>) -> Result<(), Error> {
        let kind = self.cascade().ok_or(Error::Overflow)?;
        let mut stage1_buf = [0.0_f32; STAGE_192_16 as usize];
        let stage1_len = copy_stage1(&self.coeffs, &mut stage1_buf)?;
        let stage2_owned = oneshot::cascade_stage2_table()?;
        let stage2 = stage2_owned.row(0)?;
        let stage1 = &stage1_buf[..stage1_len];
        while self.next_n < raw {
            let n0 = usize::try_from(self.next_n).map_err(|_| Error::Overflow)?;
            let remain = raw.checked_sub(self.next_n).ok_or(Error::Overflow)?;
            let take = usize::try_from(remain)
                .map_err(|_| Error::Overflow)?
                .min(CASCADE_BATCH);
            let keep = oneshot::cascade_first_input(kind, self.next_n)?.max(0);
            self.discard_before(keep);
            let delay_pos = self.delay_pos;
            let filled = self.filled;
            let mut batch = [0.0_f32; CASCADE_BATCH];
            {
                let delay = self.delay.as_slice();
                oneshot::cascade_write(
                    kind,
                    stage1,
                    stage2,
                    n0,
                    take,
                    &mut batch[..take],
                    |index| delay_sample(delay, delay_pos, filled, index),
                )?;
            }
            let room = tail
                .capacity()
                .checked_sub(tail.len())
                .ok_or(Error::Overflow)?;
            if take > room {
                return Err(Error::Overflow);
            }
            tail.extend_from_slice(&batch[..take]);
            self.next_n = self
                .next_n
                .checked_add(u64::try_from(take).map_err(|_| Error::Overflow)?)
                .ok_or(Error::Overflow)?;
        }
        Ok(())
    }

    fn store_remaining(&mut self, input: &mut &[f32], keep: i64) -> Result<(), Error> {
        self.skip_before(input, keep)?;
        if input.is_empty() {
            return Ok(());
        }
        self.discard_before(keep);
        self.append(input)?;
        *input = &[];
        Ok(())
    }

    /// Advance across input before the next tap window without storing it.
    fn skip_before(&mut self, input: &mut &[f32], keep: i64) -> Result<(), Error> {
        if keep <= self.filled {
            return Ok(());
        }
        let gap = keep.checked_sub(self.filled).ok_or(Error::Overflow)?;
        let skip = usize::try_from(gap)
            .map_err(|_| Error::Overflow)?
            .min(input.len());
        let filled = self
            .filled
            .checked_add(skip as i64)
            .ok_or(Error::Overflow)?;
        let accepted = self
            .accepted
            .checked_add(skip as u64)
            .ok_or(Error::Overflow)?;
        self.delay.clear();
        self.delay_pos = filled;
        self.filled = filled;
        self.accepted = accepted;
        *input = &input[skip..];
        Ok(())
    }

    fn ingest_until(&mut self, input: &mut &[f32], until: i64) -> Result<(), Error> {
        if self.filled >= until {
            return Ok(());
        }
        let need = match usize::try_from(until - self.filled) {
            Ok(need) => need,
            Err(_) => return Err(Error::Overflow),
        };
        if need > input.len() {
            return Err(Error::Overflow);
        }
        self.append(&input[..need])?;
        *input = &input[need..];
        Ok(())
    }

    fn append(&mut self, samples: &[f32]) -> Result<(), Error> {
        if samples.is_empty() {
            return Ok(());
        }
        let room = match self.delay.capacity().checked_sub(self.delay.len()) {
            Some(room) => room,
            None => return Err(Error::Overflow),
        };
        if samples.len() > room {
            return Err(Error::Overflow);
        }
        let add_i = match i64::try_from(samples.len()) {
            Ok(add) => add,
            Err(_) => return Err(Error::Overflow),
        };
        let add_u = match u64::try_from(samples.len()) {
            Ok(add) => add,
            Err(_) => return Err(Error::Overflow),
        };
        let filled = match self.filled.checked_add(add_i) {
            Some(filled) => filled,
            None => return Err(Error::Overflow),
        };
        let accepted = match self.accepted.checked_add(add_u) {
            Some(accepted) => accepted,
            None => return Err(Error::Overflow),
        };
        if self.delay.is_empty() {
            self.delay_pos = self.filled;
        }
        // `room` is spare capacity, so this copy does not allocate.
        self.delay.extend_from_slice(samples);
        self.filled = filled;
        self.accepted = accepted;
        Ok(())
    }

    fn discard_before(&mut self, keep: i64) {
        if keep <= self.delay_pos {
            return;
        }
        if keep >= self.filled || self.delay.is_empty() {
            self.delay.clear();
            self.delay_pos = self.filled;
            return;
        }
        let Ok(drop) = usize::try_from(keep - self.delay_pos) else {
            self.delay.clear();
            self.delay_pos = self.filled;
            return;
        };
        let len = self.delay.len();
        if drop >= len {
            self.delay.clear();
            self.delay_pos = self.filled;
            return;
        }
        self.delay.copy_within(drop.., 0);
        self.delay.truncate(len - drop);
        self.delay_pos = keep;
    }

    fn sample_at(&self, index: i64) -> Option<f32> {
        if index < 0 || index >= self.filled {
            return None;
        }
        // A sample still inside the input and before the window was discarded
        // too early. Debug builds say so; release treats it as a zero, which
        // the bit-identity tests reject.
        debug_assert!(
            index >= self.delay_pos,
            "sample {index} was discarded; delay starts at {}",
            self.delay_pos
        );
        if index < self.delay_pos {
            return None;
        }
        let Ok(offset) = usize::try_from(index - self.delay_pos) else {
            return None;
        };
        self.delay.get(offset).copied()
    }

    fn debug_window(&self) {
        let Ok(len) = i64::try_from(self.delay.len()) else {
            debug_assert!(self.delay.is_empty());
            return;
        };
        debug_assert_eq!(self.delay_pos.saturating_add(len), self.filled);
        debug_assert!(self.delay.len() <= self.delay.capacity());
    }
}

fn copy_stage1(coeffs: &Table, buf: &mut [f32]) -> Result<usize, Error> {
    let row = coeffs.row(0)?;
    if row.len() > buf.len() {
        return Err(Error::Overflow);
    }
    buf[..row.len()].copy_from_slice(row);
    Ok(row.len())
}

fn span_sample(
    delay: &[f32],
    delay_pos: i64,
    chunk: &[f32],
    chunk_start: i64,
    index: i64,
) -> Option<f32> {
    if index < 0 {
        return None;
    }
    if index < chunk_start {
        debug_assert!(
            index >= delay_pos,
            "sample {index} was discarded; delay starts at {delay_pos}"
        );
        if index < delay_pos {
            return None;
        }
        let offset = usize::try_from(index - delay_pos).ok()?;
        return delay.get(offset).copied();
    }
    let offset = usize::try_from(index - chunk_start).ok()?;
    chunk.get(offset).copied()
}

fn delay_sample(delay: &[f32], delay_pos: i64, filled: i64, index: i64) -> Option<f32> {
    if index < 0 || index >= filled {
        return None;
    }
    debug_assert!(
        index >= delay_pos,
        "sample {index} was discarded; delay starts at {delay_pos}"
    );
    if index < delay_pos {
        return None;
    }
    let offset = usize::try_from(index - delay_pos).ok()?;
    delay.get(offset).copied()
}

struct Place {
    origin: i64,
    phase: usize,
    frac: f64,
}

fn raw_output_len(input_len: u64, from: Rate, to: Rate) -> Result<u64, Error> {
    output_len(
        taps_for(from, to)?,
        PHASES,
        Ratio::reduce(from, to),
        input_len,
    )
}

fn compute(engine: &Engine, place: &Place) -> Result<f32, Error> {
    let first = engine.coeffs.row(place.phase)?;
    if engine.in_rate == 3 && engine.out_rate == 1 && place.frac == 0.0 && first.len() == 443 {
        let acc = oneshot::dot_48(place.origin, first, |index| engine.sample_at(index));
        return oneshot::finite_sample(acc);
    }
    let mut acc = (1.0 - place.frac) * dot(engine, place.origin, first);
    if place.frac != 0.0 {
        let next = match place.phase.checked_add(1) {
            Some(next) => next,
            None => return Err(Error::Overflow),
        };
        let phases = phase_count(engine.in_rate, engine.out_rate) as usize;
        if next == phases {
            let origin = match place.origin.checked_add(1) {
                Some(origin) => origin,
                None => return Err(Error::Overflow),
            };
            let second = engine.coeffs.row(0)?;
            acc += place.frac * dot(engine, origin, second);
        } else {
            let second = engine.coeffs.row(next)?;
            acc += place.frac * dot(engine, place.origin, second);
        }
    }
    oneshot::finite_sample(acc)
}

/// Inclusive last input index read for `place`.
fn last_index(place: &Place, taps: usize) -> Result<i64, Error> {
    let taps = match i64::try_from(taps) {
        Ok(taps) => taps,
        Err(_) => return Err(Error::Overflow),
    };
    let span = match taps.checked_sub(1) {
        Some(span) => span,
        None => return Err(Error::Overflow),
    };
    let mut last = match place.origin.checked_add(span) {
        Some(last) => last,
        None => return Err(Error::Overflow),
    };
    let wrapped = match place.phase.checked_add(1) {
        Some(next) => next == PHASE_COUNT,
        None => return Err(Error::Overflow),
    };
    if place.frac != 0.0 && wrapped {
        last = match place.origin.checked_add(taps) {
            Some(last) => last,
            None => return Err(Error::Overflow),
        };
    }
    Ok(last)
}

/// Euclidean placement from `docs/design.md`, in lowest terms.
fn placement(n: u64, in_rate: u64, out_rate: u64, taps: u32) -> Result<Place, Error> {
    let n = i128::from(n);
    let taps = i128::from(taps);
    let phases = i128::from(phase_count(in_rate, out_rate));
    let in_rate = i128::from(in_rate);
    let out_rate = i128::from(out_rate);
    let two_n = i128_or_overflow(n.checked_mul(2))?;
    let shifted = i128_or_overflow(i128_or_overflow(two_n.checked_sub(taps))?.checked_add(1))?;
    let left = i128_or_overflow(shifted.checked_mul(in_rate))?;
    let span = i128_or_overflow(taps.checked_sub(1))?;
    let right = i128_or_overflow(span.checked_mul(out_rate))?;
    let num = i128_or_overflow(left.checked_sub(right))?;
    let den = i128_or_overflow(out_rate.checked_mul(2))?;
    if den <= 0 {
        return Err(Error::Overflow);
    }
    let origin = num.div_euclid(den);
    let rem = num.rem_euclid(den);
    let scaled = i128_or_overflow(rem.checked_mul(phases))?;
    let phase = scaled.div_euclid(den);
    let phase_rem = scaled.rem_euclid(den);
    let origin = match i64::try_from(origin) {
        Ok(origin) => origin,
        Err(_) => return Err(Error::Overflow),
    };
    let phase = match usize::try_from(phase) {
        Ok(phase) if phase < phases as usize => phase,
        Ok(_) | Err(_) => return Err(Error::Overflow),
    };
    Ok(Place {
        origin,
        phase,
        frac: phase_rem as f64 / den as f64,
    })
}

fn i128_or_overflow(value: Option<i128>) -> Result<i128, Error> {
    match value {
        Some(value) => Ok(value),
        None => Err(Error::Overflow),
    }
}

fn dot(engine: &Engine, origin: i64, coeffs: &[f32]) -> f64 {
    if origin >= engine.delay_pos
        && let Ok(start) = usize::try_from(origin - engine.delay_pos)
        && let Some(end) = start.checked_add(coeffs.len())
        && let Some(input) = engine.delay.get(start..end)
    {
        return input
            .iter()
            .zip(coeffs)
            .fold(0.0_f64, |acc, (sample, coeff)| {
                acc + f64::from(*sample) * f64::from(*coeff)
            });
    }
    let mut acc = 0.0_f64;
    let mut index = origin;
    for coeff in coeffs {
        if let Some(sample) = engine.sample_at(index) {
            acc += f64::from(sample) * f64::from(*coeff);
        }
        index = match index.checked_add(1) {
            Some(next) => next,
            None => break,
        };
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, Place, Stream, TAP_COUNT, compute, dot, i128_or_overflow, last_index, placement,
    };
    use crate::coefficients::Table;
    use crate::{Error, Rate};
    use std::num::NonZeroU64;

    fn hz(value: u64) -> Rate {
        match NonZeroU64::new(value) {
            Some(value) => Rate(value),
            None => Rate(NonZeroU64::MIN),
        }
    }

    fn open() -> Stream {
        Stream {
            from: hz(48_000),
            to: hz(16_000),
            kept: Vec::new(),
            emitted: 0,
            finished: false,
            via_feed: false,
            via_write: false,
            engine: None,
        }
    }

    fn bare() -> Engine {
        Engine {
            coeffs: Table::empty(),
            delay: Vec::new(),
            delay_pos: 0,
            filled: 0,
            next_n: 0,
            ready_tail: None,
            accepted: 0,
            in_rate: 3,
            out_rate: 1,
        }
    }

    fn built() -> Engine {
        match Engine::build(hz(48_000), hz(16_000)) {
            Ok(engine) => engine,
            Err(err) => panic!("48 kHz to 16 kHz engine: {err:?}"),
        }
    }

    #[test]
    fn zero_constructor_stays_a_positive_rate() {
        assert_eq!(hz(0).hz(), 1);
        assert_eq!(hz(48_000).hz(), 48_000);
    }

    #[test]
    fn finish_rejects_an_emitted_cursor_past_the_buffer() {
        let mut stream = open();
        stream.emitted = usize::MAX;
        assert_eq!(stream.finish(), Err(Error::Overflow));
    }

    #[test]
    fn drain_without_an_engine_is_empty() {
        let mut stream = open();
        assert_eq!(stream.drain(), Ok(Vec::new()));
        assert!(stream.finished);
    }

    #[test]
    fn drain_rejects_a_cursor_past_the_raw_length() {
        let mut stream = open();
        let mut engine = bare();
        engine.next_n = 10_000;
        stream.engine = Some(engine);
        assert_eq!(stream.drain(), Err(Error::Overflow));
        assert!(!stream.finished);
    }

    #[test]
    fn drain_rejects_a_tail_that_does_not_fit() {
        let mut stream = open();
        let mut engine = bare();
        engine.accepted = (u64::MAX - 186) / 3 * 3;
        stream.engine = Some(engine);
        assert_eq!(stream.drain(), Err(Error::Overflow));
        assert!(!stream.finished);
    }

    #[test]
    fn pull_tail_rejects_a_missing_engine_and_a_full_buffer() {
        let mut stream = open();
        let mut tail = Vec::new();
        assert_eq!(stream.pull_tail(1, &mut tail), Err(Error::Overflow));
        let engine = built();
        let one_past_delay = match engine.next_n.checked_add(1) {
            Some(raw) => raw,
            None => panic!("group delay cursor overflowed"),
        };
        stream.engine = Some(engine);
        assert_eq!(
            stream.pull_tail(one_past_delay, &mut tail),
            Err(Error::Overflow)
        );
        assert!(tail.is_empty());
    }

    #[test]
    fn engine_rejects_counts_that_do_not_fit() {
        let mut engine = bare();
        assert_eq!(engine.outputs_for(usize::MAX), Err(Error::Overflow));
        engine.filled = i64::MAX;
        assert_eq!(engine.outputs_for(1), Err(Error::Overflow));
        let mut output = Vec::new();
        assert_eq!(engine.accept(&[0.0], &mut output), Err(Error::Overflow));
        assert!(output.is_empty());

        let mut engine = bare();
        let mut input: &[f32] = &[0.0];
        assert_eq!(engine.ingest_until(&mut input, 10), Err(Error::Overflow));
        assert_eq!(engine.append(&[]), Ok(()));
        assert_eq!(engine.append(&[0.0]), Err(Error::Overflow));
        engine.delay = Vec::with_capacity(4);
        engine.filled = i64::MAX;
        assert_eq!(engine.append(&[0.0]), Err(Error::Overflow));
        engine.filled = 0;
        engine.accepted = u64::MAX;
        assert_eq!(engine.append(&[0.0]), Err(Error::Overflow));
        assert!(engine.delay.is_empty());
    }

    #[test]
    fn output_capacity_count_matches_the_placement_walk() {
        for (input, output) in [
            (3, 1),
            (6, 1),
            (1, 2),
            (1, 3),
            (441, 160),
            (441, 320),
            (257, 258),
            (1, 256),
            (1, 257),
        ] {
            for taps in [373, 745, 1489] {
                let mut engine = bare();
                engine.in_rate = input;
                engine.out_rate = output;
                engine.coeffs.taps = taps;
                let half = i64::from((taps - 1) / 2);
                for filled in [0, half, half + 1, half + 2, half + 373] {
                    engine.filled = filled;
                    for advance in [0, 1, 255, 256, 257, 513] {
                        engine.next_n = half as u64 + advance;
                        for extra in [0, 1, 2, 31] {
                            let available = filled + extra as i64;
                            let mut n = engine.next_n;
                            loop {
                                let place = placement(n, input, output, taps)
                                    .unwrap_or_else(|e| panic!("placement {n}: {e:?}"));
                                let end = last_index(&place, taps as usize)
                                    .unwrap_or_else(|e| panic!("last index {n}: {e:?}"));
                                if end >= available {
                                    break;
                                }
                                n += 1;
                            }
                            assert_eq!(
                                engine.outputs_for(extra),
                                Ok((n - engine.next_n) as usize),
                                "{input}/{output}, taps={taps}, filled={filled}, advance={advance}, extra={extra}"
                            );
                        }
                    }
                }
            }
        }
        let mut engine = bare();
        engine.next_n = u64::MAX;
        assert_eq!(engine.outputs_for(0), Err(Error::Overflow));
    }

    #[test]
    fn output_capacity_count_rejects_unrepresentable_future_cursors() {
        let mut engine = bare();
        engine.filled = i64::MAX;
        // The next non-ready placement extends past the signed input index.
        assert_eq!(engine.outputs_for(0), Err(Error::Overflow));
        engine.in_rate = 1;
        engine.out_rate = i64::MAX as u64;
        // The ready count itself would not fit in an output cursor.
        assert_eq!(engine.outputs_for(0), Err(Error::Overflow));
        engine.out_rate = 0;
        assert_eq!(engine.outputs_for(0), Err(Error::Overflow));
        engine.in_rate = 0;
        engine.out_rate = 1;
        assert_eq!(engine.outputs_for(0), Err(Error::Overflow));
    }

    #[test]
    fn accept_rejects_a_full_output() {
        let mut engine = built();
        let ready = engine.coeffs.taps as usize;
        let samples = vec![0.0_f32; ready];
        assert_eq!(engine.append(&samples), Ok(()));
        let mut output = Vec::new();
        assert_eq!(engine.accept(&[], &mut output), Err(Error::Overflow));
        assert!(output.is_empty());
        assert_eq!(
            engine.filled,
            i64::try_from(ready).unwrap_or_else(|_| panic!("tap count"))
        );
    }

    #[test]
    fn discard_drops_a_window_that_starts_too_late() {
        let mut engine = bare();
        engine.delay = vec![1.0, 2.0];
        engine.filled = 2;
        engine.discard_before(2);
        assert!(engine.delay.is_empty());
        assert_eq!(engine.delay_pos, 2);

        engine.delay = vec![1.0];
        engine.delay_pos = 0;
        engine.filled = 100;
        engine.discard_before(5);
        assert!(engine.delay.is_empty());
        assert_eq!(engine.delay_pos, 100);
    }

    #[test]
    fn placement_rejects_a_zero_denominator_and_a_huge_cursor() {
        assert!(matches!(
            placement(0, 1, 0, TAP_COUNT as u32),
            Err(Error::Overflow)
        ));
        assert!(matches!(
            placement(u64::MAX, 3, 1, TAP_COUNT as u32),
            Err(Error::Overflow)
        ));
        assert_eq!(i128_or_overflow(None), Err(Error::Overflow));
        assert_eq!(
            last_index(
                &Place {
                    origin: i64::MAX,
                    phase: 0,
                    frac: 0.0,
                },
                TAP_COUNT
            ),
            Err(Error::Overflow)
        );
        assert_eq!(
            last_index(
                &Place {
                    origin: 0,
                    phase: usize::MAX,
                    frac: 0.0,
                },
                TAP_COUNT
            ),
            Err(Error::Overflow)
        );
        assert_eq!(
            last_index(
                &Place {
                    origin: 0,
                    phase: 255,
                    frac: 0.5,
                },
                TAP_COUNT
            ),
            Ok(i64::from(TAP_COUNT as u32))
        );
        assert_eq!(
            last_index(
                &Place {
                    origin: i64::MAX - i64::from(TAP_COUNT as u32 - 1),
                    phase: 255,
                    frac: 0.5,
                },
                TAP_COUNT
            ),
            Err(Error::Overflow)
        );
    }

    #[test]
    fn wrapped_phase_reads_the_first_row() {
        let mut engine = built();
        engine.coeffs = match Table::new(hz(44_100), hz(16_000)) {
            Ok(table) => table,
            Err(err) => panic!("fractional coefficient table: {err:?}"),
        };
        let wrapped = compute(
            &engine,
            &Place {
                origin: 0,
                phase: 255,
                frac: 0.5,
            },
        );
        assert!(matches!(wrapped, Ok(sample) if sample.is_finite()));
        assert_eq!(
            compute(
                &engine,
                &Place {
                    origin: i64::MAX,
                    phase: 255,
                    frac: 0.5,
                },
            ),
            Err(Error::Overflow)
        );
    }

    #[test]
    fn row_and_dot_reject_an_index_that_does_not_fit() {
        assert_eq!(Table::empty().row(usize::MAX), Err(Error::Overflow));
        assert_eq!(Table::empty().row(0), Err(Error::Overflow));
        assert_eq!(
            Table::empty().row(usize::MAX / TAP_COUNT),
            Err(Error::Overflow)
        );
        let engine = bare();
        assert_eq!(dot(&engine, i64::MAX, &[1.0]), 0.0);
    }
}

// Exact write-mode state; coefficient tables are deterministic and rebuilt.
impl Stream {
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        if self.finished || self.via_feed || !self.kept.is_empty() {
            return Err(Error::Overflow);
        }
        let mut out = b"YTBL0001".to_vec();
        for n in [self.from.hz() as u64, self.to.hz() as u64] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.push(u8::from(self.via_write));
        out.push(u8::from(self.engine.is_some()));
        if let Some(engine) = &self.engine {
            for n in [
                engine.delay_pos as u64,
                engine.filled as u64,
                engine.next_n,
                engine.accepted,
            ] {
                out.extend_from_slice(&n.to_le_bytes());
            }
            for samples in [
                &engine.delay[..],
                engine.ready_tail.as_deref().unwrap_or(&[]),
            ] {
                out.extend_from_slice(&(samples.len() as u64).to_le_bytes());
                for sample in samples {
                    out.extend_from_slice(&sample.to_bits().to_le_bytes());
                }
            }
        }
        Ok(out)
    }
    pub fn checkpoint_position_matches(
        &self,
        from: Rate,
        to: Rate,
        accepted: u64,
        emitted: u64,
    ) -> bool {
        if self.from.hz() != from.hz() || self.to.hz() != to.hz() {
            return false;
        }
        match &self.engine {
            Some(engine) => {
                engine.accepted == accepted
                    && group_delay(engine.coeffs.taps, PHASES, Ratio::reduce(from, to))
                        .ok()
                        .and_then(|delay| engine.next_n.checked_sub(delay))
                        == Some(emitted)
            }
            None => from.hz() == to.hz() && accepted == emitted,
        }
    }
    pub fn restore_checkpoint(bytes: &[u8]) -> Result<Self, Error> {
        let mut input = bytes;
        fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], Error> {
            let bytes = input.get(..count).ok_or(Error::Overflow)?;
            *input = &input[count..];
            Ok(bytes)
        }
        fn number(input: &mut &[u8]) -> Result<u64, Error> {
            Ok(u64::from_le_bytes(
                take(input, 8)?.try_into().map_err(|_| Error::Overflow)?,
            ))
        }
        fn samples(input: &mut &[u8]) -> Result<Vec<f32>, Error> {
            let count = usize::try_from(number(input)?).map_err(|_| Error::Overflow)?;
            if count > 65536 {
                return Err(Error::Overflow);
            }
            let bytes = take(input, count * 4)?;
            let mut out = Vec::new();
            out.try_reserve_exact(count).map_err(|_| Error::Overflow)?;
            for bytes in bytes.chunks_exact(4) {
                let sample = f32::from_bits(u32::from_le_bytes(
                    bytes.try_into().map_err(|_| Error::Overflow)?,
                ));
                if !sample.is_finite() {
                    return Err(Error::Overflow);
                }
                out.push(sample);
            }
            Ok(out)
        }
        if take(&mut input, 8)? != b"YTBL0001" {
            return Err(Error::Overflow);
        }
        let from = Rate::new(i64::try_from(number(&mut input)?).map_err(|_| Error::Overflow)?)
            .map_err(|_| Error::Overflow)?;
        let to = Rate::new(i64::try_from(number(&mut input)?).map_err(|_| Error::Overflow)?)
            .map_err(|_| Error::Overflow)?;
        let mut state = Self::new(from, to)?;
        let flags = take(&mut input, 2)?;
        if flags.iter().any(|n| *n > 1) {
            return Err(Error::Overflow);
        }
        state.via_write = flags[0] == 1;
        if flags[1] == 1 {
            let mut engine = Engine::build(from, to)?;
            engine.delay_pos = i64::try_from(number(&mut input)?).map_err(|_| Error::Overflow)?;
            engine.filled = i64::try_from(number(&mut input)?).map_err(|_| Error::Overflow)?;
            engine.next_n = number(&mut input)?;
            engine.accepted = number(&mut input)?;
            let delay = samples(&mut input)?;
            if delay.len() > engine.delay.capacity() {
                return Err(Error::Overflow);
            }
            engine.delay.extend_from_slice(&delay);
            let tail = samples(&mut input)?;
            engine.ready_tail = (!tail.is_empty()).then_some(tail);
            if engine.delay_pos > engine.filled
                || engine.filled - engine.delay_pos != engine.delay.len() as i64
                || engine.accepted != engine.filled as u64
                || !state.via_write
            {
                return Err(Error::Overflow);
            }
            state.engine = Some(engine);
        }
        if !input.is_empty() {
            return Err(Error::Overflow);
        }
        Ok(state)
    }
}
