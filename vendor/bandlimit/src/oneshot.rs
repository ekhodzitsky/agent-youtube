//! One-shot resampling. 8 kHz to 16 kHz uses a 33-tap halfband. 32 kHz to 16 kHz
//! uses one 339-tap Kaiser row. 48 kHz to 16 kHz uses one 443-tap Kaiser row.
//! 96 kHz and 192 kHz to 16 kHz cascade a short Kaiser into that 32 kHz row.
//! The public delay is 182 and 195 output samples. 44.1 kHz to 16 kHz uses a
//! 467-tap Kaiser table with 320 phases. 22.05 kHz to 16 kHz uses a 235-tap
//! Kaiser table with 640 phases. Both ratios land on one stored row, so each
//! output is one dot. Every other ratio uses Blackman-Harris 2, cutoff 0.95
//! and 256 phases.
//! Tap support scales above 3:1 decimation; the rest retain the 373-tap kernel.
//!
//! The public output skips the integer group delay before evaluating samples.
//! The scalar f64 dot defines the sample. On x86_64, AVX evaluates that same
//! dot when the CPU has it; otherwise the scalar loop runs. Integer
//! decimation and the reduced ratio 1/2 use specialized loops, unless the scalar dot
//! is forced. 32 kHz to 16 kHz keeps the left-to-right dot. 96 kHz and 192 kHz
//! to 16 kHz keep that order in each stage of the cascade. 48 kHz to 16 kHz
//! adds the 443 taps by residue class. 24 kHz to 16 kHz blocks eight outputs
//! of the 3/2 kernel.

#[cfg(target_os = "macos")]
use std::cell::UnsafeCell;

use super::{Error, Rate, Ratio, group_delay, output_len};
use crate::coefficients::{
    PHASES, STAGE_96_16, STAGE_192_16, TAPS_32_16, Table, phase_count, taps_for,
};

#[cfg(test)]
use crate::coefficients::{TAPS, TAPS_8_16, TAPS_48_16};
pub(crate) use int2::{
    CascadeKind, cascade_classify, cascade_first_input, cascade_history, cascade_largest_ready,
    cascade_stage2_table, cascade_write,
};

#[cfg(target_arch = "x86_64")]
#[path = "simd_x86.rs"]
mod simd_x86;

#[path = "int2.rs"]
mod int2;

#[path = "rate44.rs"]
mod rate44;

#[path = "rate24.rs"]
mod rate24;

#[path = "rate22.rs"]
mod rate22;

pub(crate) use int2::{block_48, four_outputs};

/// 48 kHz chunk kernel. A chunk of at most 4096 samples keeps its residue
/// copy on the stack, so the stream allocation test still sees no heap traffic.
pub(crate) enum Fast48 {
    #[cfg(target_arch = "aarch64")]
    Stack(int2::StackPack),
    Heap(int2::Block48),
}

impl Fast48 {
    pub(crate) fn eight(&self, n: usize) -> Result<Option<[f32; 8]>, Error> {
        match self {
            #[cfg(target_arch = "aarch64")]
            Self::Stack(packed) => int2::eight_stack(packed, n),
            Self::Heap(packed) => packed.eight(n),
        }
    }
}

pub(crate) fn fast_48(input: &[f32], abs_start: usize, row: &[f32]) -> Result<Fast48, Error> {
    #[cfg(target_arch = "aarch64")]
    if input.len() <= int2::STACK_INPUT {
        let mut packed = int2::StackPack::new();
        int2::pack_stack(&mut packed, input, abs_start, row)?;
        return Ok(Fast48::Stack(packed));
    }
    Ok(Fast48::Heap(block_48(input, abs_start, row)?))
}

#[derive(Clone, Copy)]
enum Engine {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Avx,
}

const PHASE_COUNT: usize = PHASES as usize;

struct Place {
    origin: i64,
    phase: usize,
    frac: f64,
}

/// Writes outputs `[first, end)` of the same kernels [`resample`] uses into `dst`.
///
/// `end` is an exclusive raw index. `dst.len()` is `end - first`. A stream
/// write uses this for one fresh chunk, including the zeros past that chunk.
pub(crate) fn fill_prefix_into(
    input: &[f32],
    from: Rate,
    to: Rate,
    first: usize,
    end: usize,
    dst: &mut [f32],
) -> Result<(), Error> {
    let Some(span) = end.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    if dst.len() != span {
        return Err(Error::Overflow);
    }
    let forced = scalar_forced();
    let engine = if forced {
        Engine::Scalar
    } else {
        detected_engine()
    };
    note_engine(engine);
    let ratio = Ratio::reduce(from, to);
    let coeffs = Table::new(from, to)?;
    if !forced && split_worth(from, to, first, end) {
        let workers = if from.hz() == 22_050 {
            worker_count(span).min(7)
        } else {
            worker_count(span)
        };
        let (ranges, count) = split_ranges(first, end, workers);
        if count >= 2 && ranges_cover(first, end, &ranges[..count]) {
            start_pool(matches!(
                from.hz(),
                24_000 | 32_000 | 48_000 | 96_000 | 192_000
            ))?;
            return dispatch_ranges(input, from, engine, first, &ranges[..count], &coeffs, dst);
        }
    }
    let samples = fill_range(input, from, to, engine, !forced, first, end, &coeffs, ratio)?;
    if samples.len() != dst.len() {
        return Err(Error::Overflow);
    }
    dst.copy_from_slice(&samples);
    Ok(())
}

/// Finite unequal rates. The returned buffer drops the integer group delay.
pub(crate) fn resample(input: &[f32], from: Rate, to: Rate) -> Result<Vec<f32>, Error> {
    let forced = scalar_forced();
    let engine = if forced {
        Engine::Scalar
    } else {
        detected_engine()
    };
    note_engine(engine);
    let ratio = Ratio::reduce(from, to);
    let delay = usize::try_from(group_delay(taps_for(from, to)?, PHASES, ratio)?)
        .map_err(|_| Error::Overflow)?;
    raw_samples(input, from, to, engine, !forced, delay)
}

fn raw_samples(
    input: &[f32],
    from: Rate,
    to: Rate,
    engine: Engine,
    fast: bool,
    first: usize,
) -> Result<Vec<f32>, Error> {
    let ratio = Ratio::reduce(from, to);
    let input_len = match u64::try_from(input.len()) {
        Ok(len) => len,
        Err(_) => return Err(Error::Overflow),
    };
    let raw_len = match usize::try_from(output_len(taps_for(from, to)?, PHASES, ratio, input_len)?)
    {
        Ok(len) => len,
        Err(_) => return Err(Error::Overflow),
    };
    let coeffs = Table::new(from, to)?;
    // Independent outputs share the scalar dots. A short buffer, a forced
    // scalar engine, and every rate outside the live set stay on one thread.
    if fast && split_worth(from, to, first, raw_len) {
        return fill_parallel(input, from, to, engine, first, raw_len, &coeffs, ratio);
    }
    fill_range(
        input, from, to, engine, fast, first, raw_len, &coeffs, ratio,
    )
}

/// Live rates whose one-shot is long enough that several P-cores pay off.
fn split_worth(from: Rate, to: Rate, first: usize, raw_len: usize) -> bool {
    let Some(span) = raw_len.checked_sub(first) else {
        return false;
    };
    if span < 8_192 || to.hz() != 16_000 {
        return false;
    }
    if !matches!(
        from.hz(),
        22_050 | 24_000 | 32_000 | 44_100 | 48_000 | 96_000 | 192_000
    ) {
        return false;
    }
    match std::thread::available_parallelism() {
        Ok(count) => count.get() >= 2,
        Err(_) => false,
    }
}

fn worker_count(span: usize) -> usize {
    let cores = match std::thread::available_parallelism() {
        Ok(count) => count.get(),
        Err(_) => 1,
    };
    let cap = cores.min(8);
    let by_work = span / 4_096;
    let workers = cap.min(by_work);
    if workers < 2 { 1 } else { workers }
}

const MAX_WORKERS: usize = 8;

/// Outputs one 24 kHz or 48 kHz worker claims at a time. A fixed eighth of the
/// buffer stayed slow after one worker left a performance core. 4096 is long
/// enough for the input window to stream, and short enough that a slow worker
/// does not keep the rest of the buffer.
const STEAL_CHUNK: usize = 4_096;

/// Chunks of eight outputs, except the last, so each kernel stays on its block.
/// The pairs live on the stack. A parallel call does not allocate them.
fn split_ranges(
    first: usize,
    raw_len: usize,
    workers: usize,
) -> ([(usize, usize); MAX_WORKERS], usize) {
    let mut ranges = [(0, 0); MAX_WORKERS];
    let Some(span) = raw_len.checked_sub(first) else {
        return (ranges, 0);
    };
    if workers < 2 || span == 0 {
        ranges[0] = (first, raw_len);
        return (ranges, 1);
    }
    let workers = workers.min(MAX_WORKERS);
    let chunk = (span / workers) / 8 * 8;
    if chunk == 0 {
        ranges[0] = (first, raw_len);
        return (ranges, 1);
    }
    let mut start = first;
    let mut count = 0;
    for index in 0..workers {
        if start >= raw_len || count == MAX_WORKERS {
            break;
        }
        let end = if index + 1 == workers {
            raw_len
        } else {
            match start.checked_add(chunk) {
                Some(end) if end < raw_len => end,
                Some(_) | None => raw_len,
            }
        };
        if start < end {
            ranges[count] = (start, end);
            count += 1;
        }
        start = end;
    }
    (ranges, count)
}

fn fill_parallel(
    input: &[f32],
    from: Rate,
    to: Rate,
    engine: Engine,
    first: usize,
    raw_len: usize,
    coeffs: &Table,
    ratio: Ratio,
) -> Result<Vec<f32>, Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    // Workers stay alive for the process. A fresh thread was slower on the
    // timed 22.05 kHz path. Each worker writes a disjoint slice of one buffer.
    // 22.05 kHz at eight workers dropped into a slow mode after the peer ran.
    // Seven left one performance core free and cleared that gate.
    let workers = if from.hz() == 22_050 {
        worker_count(span).min(7)
    } else {
        worker_count(span)
    };
    let (ranges, count) = split_ranges(first, raw_len, workers);
    if count < 2 || !ranges_cover(first, raw_len, &ranges[..count]) {
        return fill_range(input, from, to, engine, true, first, raw_len, coeffs, ratio);
    }
    // Spawn before the output buffer exists. The workers stay for the process.
    // Their stacks are not malloc, so this call does not raise the probe peak.
    start_pool(matches!(
        from.hz(),
        24_000 | 32_000 | 48_000 | 96_000 | 192_000
    ))?;
    let mut samples = Vec::new();
    if samples.try_reserve_exact(span).is_err() {
        return Err(Error::Overflow);
    }
    samples.resize(span, 0.0);
    dispatch_ranges(
        input,
        from,
        engine,
        first,
        &ranges[..count],
        coeffs,
        &mut samples,
    )?;
    Ok(samples)
}

fn ranges_cover(first: usize, raw_len: usize, ranges: &[(usize, usize)]) -> bool {
    let mut cursor = first;
    for &(start, end) in ranges {
        if start != cursor || end <= start || end > raw_len {
            return false;
        }
        cursor = end;
    }
    cursor == raw_len && !ranges.is_empty()
}

/// One output allocation. Each worker writes its own slice of `samples`.
/// The workers and their locks already exist. This call does not spawn and
/// does not open a channel.
fn dispatch_ranges(
    input: &[f32],
    from: Rate,
    engine: Engine,
    first: usize,
    ranges: &[(usize, usize)],
    coeffs: &Table,
    samples: &mut [f32],
) -> Result<(), Error> {
    #[cfg(target_arch = "aarch64")]
    prefer_interactive();
    let row48 = if from.hz() == 48_000 {
        let row = coeffs.row(0)?;
        if row.len() != 443 {
            return Err(Error::Overflow);
        }
        Some((row.as_ptr(), row.len()))
    } else {
        None
    };
    let row32 = if from.hz() == 32_000 {
        let row = coeffs.row(0)?;
        if row.len() != TAPS_32_16 as usize {
            return Err(Error::Overflow);
        }
        Some((row.as_ptr(), row.len()))
    } else {
        None
    };
    let row96 = if from.hz() == 96_000 {
        let row = coeffs.row(0)?;
        if row.len() != STAGE_96_16 as usize {
            return Err(Error::Overflow);
        }
        Some((row.as_ptr(), row.len()))
    } else {
        None
    };
    let row192 = if from.hz() == 192_000 {
        let row = coeffs.row(0)?;
        if row.len() != STAGE_192_16 as usize {
            return Err(Error::Overflow);
        }
        Some((row.as_ptr(), row.len()))
    } else {
        None
    };
    // The first parallel rate in the process chooses the workers' park class.
    // A speed process runs one rate. 24, 32, 48, 96, and 192 kHz park
    // interactive before the first wait; 22.05 kHz and 44.1 kHz do not.
    let slots = start_pool(matches!(
        from.hz(),
        24_000 | 32_000 | 48_000 | 96_000 | 192_000
    ))?;
    if ranges.len() > slots.len() {
        return Err(Error::Overflow);
    }
    // 24, 32, 44.1, 48, 96, and 192 kHz claim `STEAL_CHUNK` outputs from
    // this cursor. It lives until `wait`. The posted ranges cover `first..raw_len`.
    let raw_len = match ranges.last() {
        Some((_, end)) => *end,
        None => return Err(Error::Overflow),
    };
    let cursor = std::sync::atomic::AtomicUsize::new(first);
    // 44.1 kHz uses the same 4096-output cursor. A fixed eighth stayed on a
    // slow core after the peer ran and missed the FFmpeg gate.
    let steal_chunk = matches!(
        from.hz(),
        24_000 | 32_000 | 44_100 | 48_000 | 96_000 | 192_000
    );
    // One parallel convert at a time. The slots are process-wide, so a second
    // caller must wait until the posted slices have been written and waited.
    // The macOS gate is a static pthread mutex. `std::sync::Mutex` would
    // allocate its lock on first use, and that byte stays in the warm peak.
    #[cfg(target_os = "macos")]
    let _gate = match lock_raw(&GATE) {
        Ok(guard) => guard,
        Err(()) => return Err(Error::Overflow),
    };
    #[cfg(not(target_os = "macos"))]
    let _gate = match GATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // The 32 kHz row is a process-wide `Once`. Workers that lose that race
    // call `thread::current`, and the thread handle stays for the life of the
    // pool. Publish the row on this thread before any job is posted.
    if matches!(from.hz(), 96_000 | 192_000) {
        let _stage2 = cascade_stage2_table()?;
    }
    let base = samples.as_mut_ptr();
    let mut posted = 0_usize;
    let mut post_error = None;
    for (index, &(start, end)) in ranges.iter().enumerate() {
        let Some(len) = end.checked_sub(start) else {
            post_error = Some(Error::Overflow);
            break;
        };
        let Some(offset) = start.checked_sub(first) else {
            post_error = Some(Error::Overflow);
            break;
        };
        match offset.checked_add(len) {
            Some(tail) if tail <= samples.len() => {}
            Some(_) | None => {
                post_error = Some(Error::Overflow);
                break;
            }
        }
        // QOS_CLASS_USER_INTERACTIVE. User-initiated work woke on an efficiency
        // core after the other engine ran. Every parallel rate uses this class.
        let qos = 0x21;
        let kind = match from.hz() {
            22_050 => JobKind::Rate22 {
                coeffs: std::ptr::from_ref(coeffs),
                engine,
            },
            24_000 => JobKind::Rate24 {
                coeffs: std::ptr::from_ref(coeffs),
                engine,
            },
            44_100 => JobKind::Rate44 {
                coeffs: std::ptr::from_ref(coeffs),
                engine,
            },
            32_000 => match row32 {
                Some((row, row_len)) => JobKind::Rate32 {
                    row,
                    row_len,
                    engine,
                },
                None => {
                    post_error = Some(Error::Overflow);
                    break;
                }
            },
            48_000 => match row48 {
                Some((row, row_len)) => JobKind::Rate48 { row, row_len },
                None => {
                    post_error = Some(Error::Overflow);
                    break;
                }
            },
            96_000 => match row96 {
                Some((row, row_len)) => JobKind::Wide {
                    row,
                    row_len,
                    engine,
                    step: 6,
                },
                None => {
                    post_error = Some(Error::Overflow);
                    break;
                }
            },
            192_000 => match row192 {
                Some((row, row_len)) => JobKind::Wide {
                    row,
                    row_len,
                    engine,
                    step: 12,
                },
                None => {
                    post_error = Some(Error::Overflow);
                    break;
                }
            },
            _ => {
                post_error = Some(Error::Overflow);
                break;
            }
        };
        // SAFETY: `ranges_cover` made these slices disjoint and inside `samples`.
        // `samples` is not read until every posted job has been waited.
        // The shared cursor is on this stack and outlives `wait`. Each claimed
        // chunk is a disjoint subslice of that same buffer.
        let (output, out_len, job_start, job_end, steal, chunk) = if steal_chunk {
            (
                base,
                samples.len(),
                first,
                raw_len,
                &cursor as *const std::sync::atomic::AtomicUsize,
                STEAL_CHUNK,
            )
        } else {
            (
                unsafe { base.add(offset) },
                len,
                start,
                end,
                std::ptr::null(),
                0,
            )
        };
        match slots[index].post(Job {
            kind,
            qos,
            input: input.as_ptr(),
            input_len: input.len(),
            output,
            out_len,
            start: job_start,
            end: job_end,
            steal,
            chunk,
        }) {
            Ok(()) => posted += 1,
            Err(err) => {
                post_error = Some(err);
                break;
            }
        }
    }
    for slot in &slots[..posted] {
        slot.wake();
    }
    let mut failure: Option<Error> = None;
    let mut panic_payload = None;
    for slot in &slots[..posted] {
        match slot.wait() {
            Ok(()) => {}
            Err(SlotFail::Work(err)) => {
                if failure.is_none() {
                    failure = Some(err);
                }
            }
            Err(SlotFail::Panic(payload)) => {
                if panic_payload.is_none() {
                    panic_payload = Some(payload);
                }
            }
        }
    }
    if let Some(payload) = panic_payload {
        std::panic::resume_unwind(payload);
    }
    if let Some(err) = failure {
        return Err(err);
    }
    if let Some(err) = post_error {
        return Err(err);
    }
    Ok(())
}

enum JobKind {
    Rate22 {
        coeffs: *const Table,
        engine: Engine,
    },
    Rate24 {
        coeffs: *const Table,
        engine: Engine,
    },
    Rate44 {
        coeffs: *const Table,
        engine: Engine,
    },
    Rate32 {
        row: *const f32,
        row_len: usize,
        engine: Engine,
    },
    Rate48 {
        row: *const f32,
        row_len: usize,
    },
    /// 96 kHz (`step` 6) or 192 kHz (`step` 12). The row is the static Kaiser.
    Wide {
        row: *const f32,
        row_len: usize,
        engine: Engine,
        step: usize,
    },
}

struct Job {
    kind: JobKind,
    qos: u32,
    input: *const f32,
    input_len: usize,
    output: *mut f32,
    out_len: usize,
    start: usize,
    end: usize,
    /// Null when this job owns `start..end`. Otherwise workers pull `chunk`
    /// outputs from the cursor, which `dispatch_ranges` keeps alive.
    steal: *const std::sync::atomic::AtomicUsize,
    chunk: usize,
}

// SAFETY: `dispatch_ranges` keeps the input, the coefficient table, the 32, 48,
// 96, and 192 kHz rows, the output slice, and the chunk cursor alive until
// `wait` returns. A slot runs one job at a time. Fixed output slices posted
// together do not alias. Stolen chunks are claimed from the cursor before a
// worker borrows one. Workers only read the input and the rows.
unsafe impl Send for Job {}

struct SlotState {
    job: Option<Job>,
    result: Option<std::thread::Result<Result<(), Error>>>,
    stop: bool,
}

/// macOS `pthread_mutex_t`. The bytes are `PTHREAD_MUTEX_INITIALIZER`.
/// `std::sync::Mutex` allocates this object on first use.
#[cfg(target_os = "macos")]
#[repr(C, align(8))]
struct RawMutex {
    bytes: UnsafeCell<[u8; 64]>,
}

// SAFETY: the bytes are a macOS pthread mutex. Lock, unlock, and cond_wait
// are the only accesses, and those calls synchronize the object. It is not
// moved after the workers start.
#[cfg(target_os = "macos")]
unsafe impl Sync for RawMutex {}

/// macOS `pthread_cond_t`. The bytes are `PTHREAD_COND_INITIALIZER`.
/// `std::sync::Condvar` allocates this object on first use.
#[cfg(target_os = "macos")]
#[repr(C, align(8))]
struct RawCond {
    bytes: UnsafeCell<[u8; 48]>,
}

// SAFETY: the bytes are a macOS pthread condvar. Signal and wait are the only
// accesses, and they run with the slot mutex. The object is not moved after
// the workers start.
#[cfg(target_os = "macos")]
unsafe impl Sync for RawCond {}

#[cfg(target_os = "macos")]
const fn raw_mutex() -> RawMutex {
    let mut bytes = [0_u8; 64];
    bytes[0] = 0xa7;
    bytes[1] = 0xab;
    bytes[2] = 0xaa;
    bytes[3] = 0x32;
    RawMutex {
        bytes: UnsafeCell::new(bytes),
    }
}

#[cfg(target_os = "macos")]
const fn raw_cond() -> RawCond {
    let mut bytes = [0_u8; 48];
    bytes[0] = 0xbb;
    bytes[1] = 0xb1;
    bytes[2] = 0xb0;
    bytes[3] = 0x3c;
    RawCond {
        bytes: UnsafeCell::new(bytes),
    }
}

#[cfg(target_os = "macos")]
struct Slot {
    lock: RawMutex,
    work: RawCond,
    done: RawCond,
    state: UnsafeCell<SlotState>,
}

// SAFETY: `lock` is a pthread mutex. `state` is read and written only while
// that mutex is held. The condvars are the static initializers. The slot is
// not moved after the workers start.
#[cfg(target_os = "macos")]
unsafe impl Sync for Slot {}

#[cfg(not(target_os = "macos"))]
struct Slot {
    lock: std::sync::Mutex<SlotState>,
    work: std::sync::Condvar,
    done: std::sync::Condvar,
}

enum SlotFail {
    Work(Error),
    Panic(Box<dyn std::any::Any + Send>),
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn pthread_mutex_lock(lock: *mut RawMutex) -> i32;
    fn pthread_mutex_unlock(lock: *mut RawMutex) -> i32;
    fn pthread_cond_wait(cond: *mut RawCond, lock: *mut RawMutex) -> i32;
    fn pthread_cond_signal(cond: *mut RawCond) -> i32;
}

#[cfg(target_os = "macos")]
fn mutex_ptr(lock: &RawMutex) -> *mut RawMutex {
    std::ptr::from_ref(lock).cast_mut()
}

#[cfg(target_os = "macos")]
fn cond_ptr(cond: &RawCond) -> *mut RawCond {
    std::ptr::from_ref(cond).cast_mut()
}

/// Owns one successful `pthread_mutex_lock`. Drop unlocks it.
#[cfg(target_os = "macos")]
struct MacLock<'a> {
    lock: &'a RawMutex,
}

#[cfg(target_os = "macos")]
fn lock_raw(lock: &RawMutex) -> Result<MacLock<'_>, ()> {
    // SAFETY: `lock` is a live macOS pthread mutex whose bytes are the static
    // initializer and live in an `UnsafeCell`. A nonzero code means the call
    // did not acquire it.
    let rc = unsafe { pthread_mutex_lock(mutex_ptr(lock)) };
    if rc != 0 {
        return Err(());
    }
    Ok(MacLock { lock })
}

#[cfg(target_os = "macos")]
impl Drop for MacLock<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after a successful lock.
        // `pthread_cond_wait` reacquires that same mutex before it returns,
        // so the guard still owns the lock here. The static is not destroyed.
        unsafe { pthread_mutex_unlock(mutex_ptr(self.lock)) };
    }
}

#[cfg(target_os = "macos")]
impl Slot {
    fn post(&self, job: Job) -> Result<(), Error> {
        let _guard = lock_raw(&self.lock).map_err(|()| Error::Overflow)?;
        // SAFETY: the slot mutex is held. `state` is written only while it is held.
        // This borrow ends before the guard drops and does not cross a wait.
        let state = unsafe { &mut *self.state.get() };
        if state.job.is_some() {
            return Err(Error::Overflow);
        }
        state.result = None;
        state.job = Some(job);
        Ok(())
    }

    fn wake(&self) {
        // SAFETY: `work` is this slot's static condvar. The job is already
        // stored under the mutex. A worker that has not reached `wait` sees
        // that job on its next lock instead of missing the signal.
        unsafe { pthread_cond_signal(cond_ptr(&self.work)) };
    }

    fn wait(&self) -> Result<(), SlotFail> {
        let _guard = match lock_raw(&self.lock) {
            Ok(guard) => guard,
            Err(()) => return Err(SlotFail::Work(Error::Overflow)),
        };
        while {
            // SAFETY: the mutex is held. The borrow ends before `wait`, so a
            // worker can store the result while this thread is asleep.
            let state = unsafe { &mut *self.state.get() };
            state.result.is_none()
        } {
            // SAFETY: the mutex is held. The wait releases it and reacquires
            // it before returning, which is the ownership `MacLock` still has.
            // A nonzero code is ignored; the predicate is checked again.
            let _ = unsafe { pthread_cond_wait(cond_ptr(&self.done), mutex_ptr(&self.lock)) };
        }
        // SAFETY: the mutex is held again. The borrow ends before the guard drops.
        let state = unsafe { &mut *self.state.get() };
        match state.result.take() {
            Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(err))) => Err(SlotFail::Work(err)),
            Some(Err(payload)) => Err(SlotFail::Panic(payload)),
            None => Err(SlotFail::Work(Error::Overflow)),
        }
    }

    /// Block until a job is posted. `None` means the slot was stopped.
    /// A failed lock is retried. Leaving the thread would stall the pool.
    fn take_job(&self) -> Option<Job> {
        let _guard = loop {
            match lock_raw(&self.lock) {
                Ok(guard) => break guard,
                Err(()) => std::thread::yield_now(),
            }
        };
        while {
            // SAFETY: the mutex is held. The borrow ends before `wait`.
            let state = unsafe { &mut *self.state.get() };
            state.job.is_none() && !state.stop
        } {
            // SAFETY: same ownership as `Slot::wait`. The predicate is rechecked.
            let _ = unsafe { pthread_cond_wait(cond_ptr(&self.work), mutex_ptr(&self.lock)) };
        }
        // SAFETY: the mutex is held. The job is taken before the guard unlocks,
        // and `run_job` runs only after this function returns.
        let state = unsafe { &mut *self.state.get() };
        if state.job.is_none() {
            return None;
        }
        state.job.take()
    }

    fn finish(&self, result: std::thread::Result<Result<(), Error>>) {
        let _guard = loop {
            match lock_raw(&self.lock) {
                Ok(guard) => break guard,
                Err(()) => std::thread::yield_now(),
            }
        };
        // SAFETY: the mutex is held. The signal runs before the guard unlocks,
        // so a waiter cannot miss the stored result.
        let state = unsafe { &mut *self.state.get() };
        state.result = Some(result);
        unsafe { pthread_cond_signal(cond_ptr(&self.done)) };
    }
}

#[cfg(not(target_os = "macos"))]
impl Slot {
    fn post(&self, job: Job) -> Result<(), Error> {
        let mut guard = match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.job.is_some() {
            return Err(Error::Overflow);
        }
        guard.result = None;
        guard.job = Some(job);
        Ok(())
    }

    fn wake(&self) {
        self.work.notify_one();
    }

    fn wait(&self) -> Result<(), SlotFail> {
        let mut guard = match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        while guard.result.is_none() {
            guard = match self.done.wait(guard) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        match guard.result.take() {
            Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(err))) => Err(SlotFail::Work(err)),
            Some(Err(payload)) => Err(SlotFail::Panic(payload)),
            None => Err(SlotFail::Work(Error::Overflow)),
        }
    }

    fn take_job(&self) -> Option<Job> {
        let mut guard = match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        while guard.job.is_none() && !guard.stop {
            guard = match self.work.wait(guard) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        if guard.job.is_none() {
            return None;
        }
        guard.job.take()
    }

    fn finish(&self, result: std::thread::Result<Result<(), Error>>) {
        let mut guard = match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.result = Some(result);
        self.done.notify_one();
    }
}

#[cfg(target_os = "macos")]
impl Slot {
    const fn new() -> Self {
        Self {
            lock: raw_mutex(),
            work: raw_cond(),
            done: raw_cond(),
            state: UnsafeCell::new(SlotState {
                job: None,
                result: None,
                stop: false,
            }),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Slot {
    const fn new() -> Self {
        Self {
            lock: std::sync::Mutex::new(SlotState {
                job: None,
                result: None,
                stop: false,
            }),
            work: std::sync::Condvar::new(),
            done: std::sync::Condvar::new(),
        }
    }
}

/// Process-wide workers. The slots and the gate are BSS. macOS creates the
/// threads with `pthread_create` and a 2 MiB mapped stack, so the pool does
/// not call `malloc`. The first parallel rate chooses the park class.
static SLOTS: [Slot; MAX_WORKERS] = [
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
];
#[cfg(target_os = "macos")]
static GATE: RawMutex = raw_mutex();
#[cfg(not(target_os = "macos"))]
static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static PARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STARTED: std::sync::Once = std::sync::Once::new();
static START_ERR: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn start_pool(park_interactive: bool) -> Result<&'static [Slot], Error> {
    STARTED.call_once(|| {
        let count = worker_count(usize::MAX).clamp(2, MAX_WORKERS);
        PARK.store(park_interactive, std::sync::atomic::Ordering::Release);
        match spawn_workers(count) {
            Ok(()) => WORKERS.store(count, std::sync::atomic::Ordering::Release),
            Err(code) => START_ERR.store(code, std::sync::atomic::Ordering::Release),
        }
    });
    if START_ERR.load(std::sync::atomic::Ordering::Acquire) != 0 {
        return Err(Error::Overflow);
    }
    let count = WORKERS.load(std::sync::atomic::Ordering::Acquire);
    Ok(&SLOTS[..count])
}

fn spawn_workers(count: usize) -> Result<(), i32> {
    #[cfg(target_os = "macos")]
    {
        spawn_workers_macos(count)
    }
    #[cfg(not(target_os = "macos"))]
    {
        for index in 0..count {
            let slot = &SLOTS[index];
            std::thread::spawn(move || {
                worker_loop(slot, PARK.load(std::sync::atomic::Ordering::Acquire));
            });
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn spawn_workers_macos(count: usize) -> Result<(), i32> {
    /// libSystem `pthread_attr_t` on this OS: 64 bytes, 8-byte alignment.
    #[repr(C, align(8))]
    struct RawAttr {
        _bytes: [u8; 64],
    }
    unsafe extern "C" {
        fn pthread_attr_init(attr: *mut RawAttr) -> i32;
        fn pthread_attr_destroy(attr: *mut RawAttr) -> i32;
        fn pthread_attr_setstacksize(attr: *mut RawAttr, stacksize: usize) -> i32;
        fn pthread_create(
            thread: *mut *mut core::ffi::c_void,
            attr: *const RawAttr,
            start: extern "C" fn(*mut core::ffi::c_void) -> *mut core::ffi::c_void,
            arg: *mut core::ffi::c_void,
        ) -> i32;
    }
    extern "C" fn entry(arg: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
        // SAFETY: the argument is the address of one static `SLOTS` element.
        // The slot outlives the thread. The thread only uses that one slot.
        let slot = unsafe { &*(arg.cast::<Slot>()) };
        worker_loop(slot, PARK.load(std::sync::atomic::Ordering::Acquire));
        core::ptr::null_mut()
    }
    let mut attr = RawAttr { _bytes: [0; 64] };
    // SAFETY: `attr` is a live object of the macOS `pthread_attr_t` size and
    // alignment. These three calls initialize it, request a 2 MiB stack, and
    // destroy it. libSystem maps that stack; it does not malloc the bytes.
    let rc = unsafe { pthread_attr_init(&raw mut attr) };
    if rc != 0 {
        return Err(rc);
    }
    let rc = unsafe { pthread_attr_setstacksize(&raw mut attr, 2 * 1024 * 1024) };
    if rc != 0 {
        unsafe { pthread_attr_destroy(&raw mut attr) };
        return Err(rc);
    }
    for index in 0..count {
        let mut thread = core::ptr::null_mut();
        let arg: *mut core::ffi::c_void = core::ptr::from_ref(&SLOTS[index]).cast_mut().cast();
        let rc = unsafe { pthread_create(&raw mut thread, &raw const attr, entry, arg) };
        if rc != 0 {
            unsafe { pthread_attr_destroy(&raw mut attr) };
            return Err(rc);
        }
    }
    unsafe { pthread_attr_destroy(&raw mut attr) };
    Ok(())
}

fn worker_loop(slot: &'static Slot, park_interactive: bool) {
    // 24 kHz, 32 kHz, and 48 kHz must take user-interactive before the first
    // wait, or a later pass wakes on an efficiency core. The same park pins
    // every 44.1 kHz worker to a slow cluster, so the 22.05 kHz and 44.1 kHz
    // pools do not.
    #[cfg(target_arch = "aarch64")]
    if park_interactive {
        prefer_interactive();
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = park_interactive;
    loop {
        // The lock drops before `run_job`. `finish` signals while it holds the
        // lock again, matching the previous condvar protocol.
        let Some(job) = slot.take_job() else {
            return;
        };
        #[cfg(target_arch = "aarch64")]
        if job.qos == 0x21 {
            prefer_interactive();
        } else {
            prefer_p_cores();
        }
        #[cfg(not(target_arch = "aarch64"))]
        set_qos(job.qos);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job)));
        slot.finish(result);
    }
}

fn run_job(job: &Job) -> Result<(), Error> {
    // SAFETY: `dispatch_ranges` keeps `input`, the coefficient table, the 32, 48,
    // 96, and 192 kHz rows, and `output` alive until `wait` returns. Each output
    // slice is disjoint. Workers only read the input and the rows. A stolen
    // chunk borrows its own subslice only after the cursor claims that range.
    let input = unsafe { std::slice::from_raw_parts(job.input, job.input_len) };
    if !job.steal.is_null() {
        return steal_chunks(job, input);
    }
    let output = unsafe { std::slice::from_raw_parts_mut(job.output, job.out_len) };
    match job.kind {
        JobKind::Rate22 { coeffs, engine } => {
            let coeffs = unsafe { &*coeffs };
            rate22::fill_into(input, coeffs, job.end, engine, job.start, output)
        }
        JobKind::Rate24 { coeffs, engine } => {
            let coeffs = unsafe { &*coeffs };
            rate24::fill_into(input, coeffs, job.end, engine, job.start, output)
        }
        JobKind::Rate44 { coeffs, engine } => {
            let coeffs = unsafe { &*coeffs };
            rate44::fill_into(input, coeffs, job.end, engine, job.start, output)
        }
        JobKind::Rate32 {
            row,
            row_len,
            engine,
        } => {
            let row = unsafe { std::slice::from_raw_parts(row, row_len) };
            int2::fill_into_32(input, row, job.end, output, job.start, engine)
        }
        JobKind::Rate48 { row, row_len } => {
            let row = unsafe { std::slice::from_raw_parts(row, row_len) };
            int2::fill_into_48(input, row, job.end, output, job.start)
        }
        JobKind::Wide {
            row,
            row_len,
            engine,
            step,
        } => {
            let row = unsafe { std::slice::from_raw_parts(row, row_len) };
            match step {
                6 => int2::fill_into_96(input, row, job.end, output, job.start, engine),
                12 => int2::fill_into_192(input, row, job.end, output, job.start, engine),
                _ => Err(Error::Overflow),
            }
        }
    }
}

fn steal_chunks(job: &Job, input: &[f32]) -> Result<(), Error> {
    if job.chunk == 0 {
        return Err(Error::Overflow);
    }
    // SAFETY: the cursor belongs to the dispatch stack and stays alive until
    // every posted worker has returned. `fetch_add` hands each chunk to one
    // worker. The resulting subslices of `output` do not overlap.
    let cursor = unsafe { &*job.steal };
    loop {
        let start = cursor.fetch_add(job.chunk, std::sync::atomic::Ordering::Relaxed);
        if start >= job.end {
            return Ok(());
        }
        let end = match start.checked_add(job.chunk) {
            Some(end) if end < job.end => end,
            Some(_) | None => job.end,
        };
        let Some(offset) = start.checked_sub(job.start) else {
            return Err(Error::Overflow);
        };
        let Some(len) = end.checked_sub(start) else {
            return Err(Error::Overflow);
        };
        let Some(tail) = offset.checked_add(len) else {
            return Err(Error::Overflow);
        };
        if tail > job.out_len {
            return Err(Error::Overflow);
        }
        let output = unsafe { std::slice::from_raw_parts_mut(job.output.add(offset), len) };
        match job.kind {
            JobKind::Rate24 { coeffs, engine } => {
                let coeffs = unsafe { &*coeffs };
                rate24::fill_into(input, coeffs, end, engine, start, output)?;
            }
            JobKind::Rate44 { coeffs, engine } => {
                let coeffs = unsafe { &*coeffs };
                rate44::fill_into(input, coeffs, end, engine, start, output)?;
            }
            JobKind::Rate32 {
                row,
                row_len,
                engine,
            } => {
                let row = unsafe { std::slice::from_raw_parts(row, row_len) };
                int2::fill_into_32(input, row, end, output, start, engine)?;
            }
            JobKind::Rate48 { row, row_len } => {
                let row = unsafe { std::slice::from_raw_parts(row, row_len) };
                int2::fill_into_48(input, row, end, output, start)?;
            }
            JobKind::Wide {
                row,
                row_len,
                engine,
                step,
            } => {
                let row = unsafe { std::slice::from_raw_parts(row, row_len) };
                match step {
                    6 => int2::fill_into_96(input, row, end, output, start, engine)?,
                    12 => int2::fill_into_192(input, row, end, output, start, engine)?,
                    _ => return Err(Error::Overflow),
                }
            }
            JobKind::Rate22 { .. } => return Err(Error::Overflow),
        }
    }
}

/// Ask macOS for a P-core. A refusal leaves the thread on the default class.
/// The pool posts interactive work; this class is the other branch.
fn prefer_p_cores() {
    // QOS_CLASS_USER_INITIATED.
    set_qos(0x19);
}

/// Persistent workers on every parallel rate. User-initiated was fast for one
/// pass and then woke on an efficiency core after the other engine ran.
#[cfg(target_arch = "aarch64")]
fn prefer_interactive() {
    // QOS_CLASS_USER_INTERACTIVE.
    set_qos(0x21);
}

fn set_qos(class: u32) {
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
        }
        // SAFETY: libSystem exports this symbol. `class` is a documented QoS
        // constant and the relative priority is zero. The call only changes
        // this thread.
        let _ = pthread_set_qos_class_self_np(class, 0);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = class;
}

fn fill_range(
    input: &[f32],
    from: Rate,
    to: Rate,
    engine: Engine,
    fast: bool,
    first: usize,
    raw_len: usize,
    coeffs: &Table,
    ratio: Ratio,
) -> Result<Vec<f32>, Error> {
    let in_rate = ratio.numerator();
    let out_rate = ratio.denominator();
    // `scalar` and `decline` keep this general loop, so a test can compare
    // it with the specialized integer kernels.
    if fast && let Some(kind) = int2::kind(in_rate, out_rate) {
        return int2::fill(input, coeffs, raw_len, kind, engine, first);
    }
    if fast && from.hz() == 44_100 && to.hz() == 16_000 {
        return rate44::fill(input, coeffs, raw_len, engine, first);
    }
    if fast && from.hz() == 24_000 && to.hz() == 16_000 {
        return rate24::fill(input, coeffs, raw_len, engine, first);
    }
    if fast && from.hz() == 22_050 && to.hz() == 16_000 {
        return rate22::fill(input, coeffs, raw_len, engine, first);
    }
    if let Some(kind) = int2::cascade_classify(in_rate, out_rate, coeffs.taps) {
        return fill_cascade_scalar(input, coeffs, raw_len, first, kind);
    }
    let mut samples = Vec::new();
    if samples.try_reserve_exact(raw_len - first).is_err() {
        return Err(Error::Overflow);
    }
    // The placement formula scales with both rates. Lowest terms stay inside
    // `i128` whenever [`output_len`] accepted the length, and the phase is
    // unchanged.
    for n in first..raw_len {
        let n = match u64::try_from(n) {
            Ok(n) => n,
            Err(_) => return Err(Error::Overflow),
        };
        samples.push(sample(input, coeffs, n, in_rate, out_rate, engine)?);
    }
    Ok(samples)
}

/// Scalar outputs `[skip, skip + count)` after this table's own group delay.
/// `fast` stays off, so the loop cannot enter a kernel that hardcodes 235 or 467 taps.
#[cfg(test)]
pub(crate) fn scalar_after_delay(
    input: &[f32],
    coeffs: &Table,
    from: Rate,
    to: Rate,
    skip: usize,
    count: usize,
) -> Result<Vec<f32>, Error> {
    let ratio = Ratio::reduce(from, to);
    let delay =
        usize::try_from(group_delay(coeffs.taps, PHASES, ratio)?).map_err(|_| Error::Overflow)?;
    let first = match delay.checked_add(skip) {
        Some(first) => first,
        None => return Err(Error::Overflow),
    };
    let end = match first.checked_add(count) {
        Some(end) => end,
        None => return Err(Error::Overflow),
    };
    fill_range(
        input,
        from,
        to,
        Engine::Scalar,
        false,
        first,
        end,
        coeffs,
        ratio,
    )
}

/// One shared `f64` copy of a live phase table. The first caller builds it.
/// Later calls, including worker threads, borrow that copy.
/// The exact 44.1 path reads f32 rows and does not build this copy.
#[allow(dead_code)]
pub(super) fn cached_wide(from_hz: u64, coeffs: &Table) -> Result<&'static [f64], Error> {
    // `get_or_try_init` is still unstable. A failed widen is not stored.
    // Two first callers may each build a copy; `get_or_init` keeps one.
    let slot = match from_hz {
        22_050 => {
            static SLOT: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
            &SLOT
        }
        44_100 => {
            static SLOT: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
            &SLOT
        }
        _ => return Err(Error::Overflow),
    };
    if let Some(wide) = slot.get() {
        return Ok(wide.as_slice());
    }
    let wide = widen_phases(coeffs)?;
    Ok(slot.get_or_init(|| wide).as_slice())
}

/// One `f64` copy of every phase row, in phase order. Each value is
/// `f64::from` of the stored `f32`, so a later dot matches the scalar product.
#[allow(dead_code)]
pub(super) fn widen_phases(coeffs: &Table) -> Result<Vec<f64>, Error> {
    let taps = coeffs.taps as usize;
    let phases = coeffs.phases();
    let count = match phases.checked_mul(taps) {
        Some(count) => count,
        None => return Err(Error::Overflow),
    };
    let mut wide = Vec::new();
    if wide.try_reserve_exact(count).is_err() {
        return Err(Error::Overflow);
    }
    for phase in 0..phases {
        let row = coeffs.row(phase)?;
        if row.len() != taps {
            return Err(Error::Overflow);
        }
        for coeff in row {
            wide.push(f64::from(*coeff));
        }
    }
    Ok(wide)
}

pub(super) fn phase_f64(wide: &[f64], phase: usize, taps: usize) -> Option<&[f64]> {
    let start = phase.checked_mul(taps)?;
    let end = start.checked_add(taps)?;
    wide.get(start..end)
}

/// Eight left-to-right dots. Lanes 0..4 are the first phase and 4..8 the next.
/// Tap `i` joins that lane before tap `i + 1`. Products are multiply then add.
#[cfg(target_arch = "aarch64")]
pub(super) fn f64_dots4(w: [&[f32]; 4], a: [&[f64]; 4], b: [&[f64]; 4]) -> Option<[f64; 8]> {
    let n = w[0].len();
    let fit = |row: &[f64]| row.len() == n;
    if w[1].len() != n || w[2].len() != n || w[3].len() != n {
        return None;
    }
    if !fit(a[0]) || !fit(a[1]) || !fit(a[2]) || !fit(a[3]) {
        return None;
    }
    if !fit(b[0]) || !fit(b[1]) || !fit(b[2]) || !fit(b[3]) {
        return None;
    }
    // SAFETY: each slice length is `n`. NEON is baseline on aarch64.
    // `vld1q_f64` is used at an even index, so it stays inside the row.
    Some(unsafe { f64_dots4_lanes(w, a, b, n) })
}

/// # Safety
///
/// Every slice length is `n`. NEON is available.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn f64_dots4_lanes(w: [&[f32]; 4], a: [&[f64]; 4], b: [&[f64]; 4], n: usize) -> [f64; 8] {
    use std::arch::aarch64::{vcvt_f64_f32, vgetq_lane_f64, vld1_f32, vld1q_f64, vmulq_f64};
    unsafe {
        let w0 = w[0].as_ptr();
        let w1 = w[1].as_ptr();
        let w2 = w[2].as_ptr();
        let w3 = w[3].as_ptr();
        let a0 = a[0].as_ptr();
        let a1 = a[1].as_ptr();
        let a2 = a[2].as_ptr();
        let a3 = a[3].as_ptr();
        let b0 = b[0].as_ptr();
        let b1 = b[1].as_ptr();
        let b2 = b[2].as_ptr();
        let b3 = b[3].as_ptr();
        let mut s0 = 0.0_f64;
        let mut s1 = 0.0_f64;
        let mut s2 = 0.0_f64;
        let mut s3 = 0.0_f64;
        let mut t0 = 0.0_f64;
        let mut t1 = 0.0_f64;
        let mut t2 = 0.0_f64;
        let mut t3 = 0.0_f64;
        let mut i = 0_usize;
        let pairs = n / 2 * 2;
        while i < pairs {
            let x0 = vcvt_f64_f32(vld1_f32(w0.add(i)));
            let x1 = vcvt_f64_f32(vld1_f32(w1.add(i)));
            let x2 = vcvt_f64_f32(vld1_f32(w2.add(i)));
            let x3 = vcvt_f64_f32(vld1_f32(w3.add(i)));
            let p0 = vmulq_f64(x0, vld1q_f64(a0.add(i)));
            let p1 = vmulq_f64(x1, vld1q_f64(a1.add(i)));
            let p2 = vmulq_f64(x2, vld1q_f64(a2.add(i)));
            let p3 = vmulq_f64(x3, vld1q_f64(a3.add(i)));
            let q0 = vmulq_f64(x0, vld1q_f64(b0.add(i)));
            let q1 = vmulq_f64(x1, vld1q_f64(b1.add(i)));
            let q2 = vmulq_f64(x2, vld1q_f64(b2.add(i)));
            let q3 = vmulq_f64(x3, vld1q_f64(b3.add(i)));
            // Low lane is tap `i` and joins the accumulator before tap `i + 1`.
            s0 += vgetq_lane_f64(p0, 0);
            s0 += vgetq_lane_f64(p0, 1);
            s1 += vgetq_lane_f64(p1, 0);
            s1 += vgetq_lane_f64(p1, 1);
            s2 += vgetq_lane_f64(p2, 0);
            s2 += vgetq_lane_f64(p2, 1);
            s3 += vgetq_lane_f64(p3, 0);
            s3 += vgetq_lane_f64(p3, 1);
            t0 += vgetq_lane_f64(q0, 0);
            t0 += vgetq_lane_f64(q0, 1);
            t1 += vgetq_lane_f64(q1, 0);
            t1 += vgetq_lane_f64(q1, 1);
            t2 += vgetq_lane_f64(q2, 0);
            t2 += vgetq_lane_f64(q2, 1);
            t3 += vgetq_lane_f64(q3, 0);
            t3 += vgetq_lane_f64(q3, 1);
            i += 2;
        }
        if i < n {
            let x0 = f64::from(w[0][i]);
            let x1 = f64::from(w[1][i]);
            let x2 = f64::from(w[2][i]);
            let x3 = f64::from(w[3][i]);
            s0 += x0 * a[0][i];
            t0 += x0 * b[0][i];
            s1 += x1 * a[1][i];
            t1 += x1 * b[1][i];
            s2 += x2 * a[2][i];
            t2 += x2 * b[2][i];
            s3 += x3 * a[3][i];
            t3 += x3 * b[3][i];
        }
        [s0, s1, s2, s3, t0, t1, t2, t3]
    }
}

/// Four left-to-right dots of one phase. Tap `i` joins before tap `i + 1`.
#[cfg(target_arch = "aarch64")]
pub(super) fn f64_dots4_one(w: [&[f32]; 4], a: [&[f64]; 4]) -> Option<[f64; 4]> {
    let n = w[0].len();
    if w[1].len() != n || w[2].len() != n || w[3].len() != n {
        return None;
    }
    if a[0].len() != n || a[1].len() != n || a[2].len() != n || a[3].len() != n {
        return None;
    }
    // SAFETY: each slice length is `n`. NEON is baseline on aarch64.
    // `vld1q_f64` is used at an even index, so it stays inside the row.
    // Lane 0 of each accumulator is the left-to-right sum: tap `i`, then tap `i + 1`.
    Some(unsafe { f64_dots4_one_lanes(w, a, n) })
}

/// # Safety
///
/// Every slice length is `n`. NEON is available.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn f64_dots4_one_lanes(w: [&[f32]; 4], a: [&[f64]; 4], n: usize) -> [f64; 4] {
    use std::arch::aarch64::{
        vaddq_f64, vcvt_f64_f32, vdupq_n_f64, vextq_f64, vgetq_lane_f64, vld1_f32, vld1q_f64,
        vmulq_f64,
    };
    unsafe {
        let w0 = w[0].as_ptr();
        let w1 = w[1].as_ptr();
        let w2 = w[2].as_ptr();
        let w3 = w[3].as_ptr();
        let a0 = a[0].as_ptr();
        let a1 = a[1].as_ptr();
        let a2 = a[2].as_ptr();
        let a3 = a[3].as_ptr();
        // Lane 0 holds the running sum. Lane 1 is only a place to park tap `i + 1`
        // until the next add moves it into lane 0. That is `sum + tap i`, then
        // `+ tap i + 1`, the same order as the scalar dot.
        let mut s0 = vdupq_n_f64(0.0);
        let mut s1 = vdupq_n_f64(0.0);
        let mut s2 = vdupq_n_f64(0.0);
        let mut s3 = vdupq_n_f64(0.0);
        let zero = vdupq_n_f64(0.0);
        let mut i = 0_usize;
        let pairs = n / 2 * 2;
        while i < pairs {
            let x0 = vcvt_f64_f32(vld1_f32(w0.add(i)));
            let x1 = vcvt_f64_f32(vld1_f32(w1.add(i)));
            let x2 = vcvt_f64_f32(vld1_f32(w2.add(i)));
            let x3 = vcvt_f64_f32(vld1_f32(w3.add(i)));
            let p0 = vmulq_f64(x0, vld1q_f64(a0.add(i)));
            let p1 = vmulq_f64(x1, vld1q_f64(a1.add(i)));
            let p2 = vmulq_f64(x2, vld1q_f64(a2.add(i)));
            let p3 = vmulq_f64(x3, vld1q_f64(a3.add(i)));
            s0 = vaddq_f64(s0, p0);
            s0 = vaddq_f64(s0, vextq_f64(p0, zero, 1));
            s1 = vaddq_f64(s1, p1);
            s1 = vaddq_f64(s1, vextq_f64(p1, zero, 1));
            s2 = vaddq_f64(s2, p2);
            s2 = vaddq_f64(s2, vextq_f64(p2, zero, 1));
            s3 = vaddq_f64(s3, p3);
            s3 = vaddq_f64(s3, vextq_f64(p3, zero, 1));
            i += 2;
        }
        let mut out0 = vgetq_lane_f64(s0, 0);
        let mut out1 = vgetq_lane_f64(s1, 0);
        let mut out2 = vgetq_lane_f64(s2, 0);
        let mut out3 = vgetq_lane_f64(s3, 0);
        if i < n {
            out0 += f64::from(w[0][i]) * a[0][i];
            out1 += f64::from(w[1][i]) * a[1][i];
            out2 += f64::from(w[2][i]) * a[2][i];
            out3 += f64::from(w[3][i]) * a[3][i];
        }
        [out0, out1, out2, out3]
    }
}

fn fill_cascade_scalar(
    input: &[f32],
    coeffs: &Table,
    raw_len: usize,
    first: usize,
    kind: int2::CascadeKind,
) -> Result<Vec<f32>, Error> {
    let Some(span) = raw_len.checked_sub(first) else {
        return Err(Error::Overflow);
    };
    let mut samples = Vec::new();
    if samples.try_reserve_exact(span).is_err() {
        return Err(Error::Overflow);
    }
    samples.resize(span, 0.0);
    let stage1 = coeffs.row(0)?;
    let stage2_owned = int2::cascade_stage2_table()?;
    let stage2 = stage2_owned.row(0)?;
    int2::cascade_write(kind, stage1, stage2, first, span, &mut samples, |index| {
        sample_at(input, index)
    })?;
    Ok(samples)
}

fn sample(
    input: &[f32],
    coeffs: &Table,
    n: u64,
    in_rate: u64,
    out_rate: u64,
    engine: Engine,
) -> Result<f32, Error> {
    if let Some(kind) = int2::cascade_classify(in_rate, out_rate, coeffs.taps) {
        let stage1 = coeffs.row(0)?;
        let stage2_owned = int2::cascade_stage2_table()?;
        let stage2 = stage2_owned.row(0)?;
        let n = usize::try_from(n).map_err(|_| Error::Overflow)?;
        return int2::cascade_reference_one(kind, stage1, stage2, n, |index| {
            sample_at(input, index)
        });
    }
    let place = placement(n, in_rate, out_rate, coeffs.taps)?;
    let row = coeffs.row(place.phase)?;
    // 3/1 with the 443-tap row is 48 kHz to 16 kHz. Residue-class order is
    // the definition for that pair, including when the fast kernel is forced off.
    if in_rate == 3 && out_rate == 1 && place.frac == 0.0 && row.len() == 443 {
        return finite_sample(dot_48(place.origin, row, |index| sample_at(input, index)));
    }
    let mut acc = (1.0 - place.frac) * dot(input, place.origin, row, engine);
    if place.frac != 0.0 {
        let next = match place.phase.checked_add(1) {
            Some(next) => next,
            None => return Err(Error::Overflow),
        };
        let phases = phase_count(in_rate, out_rate) as usize;
        if next == phases {
            let origin = match place.origin.checked_add(1) {
                Some(origin) => origin,
                None => return Err(Error::Overflow),
            };
            let row = coeffs.row(0)?;
            acc += place.frac * dot(input, origin, row, engine);
        } else {
            let row = coeffs.row(next)?;
            acc += place.frac * dot(input, place.origin, row, engine);
        }
    }
    finite_sample(acc)
}

/// A convolution that does not fit in a finite `f32` is [`Error::Range`].
pub(crate) fn finite_sample(acc: f64) -> Result<f32, Error> {
    let sample = acc as f32;
    if sample.is_finite() {
        Ok(sample)
    } else {
        Err(Error::Range)
    }
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

/// `BANDLIMIT_KERNEL=scalar` and `decline` force the scalar dot. Any other
/// set value, and an unset variable, leave this false and use runtime
/// detection. Tests set `BANDLIMIT_KERNEL_REPORT` so the engine is recorded.
fn scalar_forced() -> bool {
    match std::env::var_os("BANDLIMIT_KERNEL") {
        Some(value) if value == "scalar" || value == "decline" => true,
        Some(_) | None => false,
    }
}

fn detected_engine() -> Engine {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_x86::avx_available() {
            return Engine::Avx;
        }
    }
    Engine::Scalar
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Scalar => "scalar",
        #[cfg(target_arch = "x86_64")]
        Engine::Avx => "avx",
    }
}

fn note_engine(engine: Engine) {
    if std::env::var_os("BANDLIMIT_KERNEL").is_none()
        && std::env::var_os("BANDLIMIT_KERNEL_REPORT").is_none()
    {
        return;
    }
    let path = std::env::temp_dir().join(format!("bandlimit-kernel-{}", std::process::id()));
    // The SIMD test reads this file. A write error leaves that read to fail.
    if let Ok(()) = std::fs::write(path, engine_name(engine)) {}
}

fn dot(input: &[f32], origin: i64, coeffs: &[f32], engine: Engine) -> f64 {
    match engine {
        Engine::Scalar => dot_scalar(input, origin, coeffs),
        // SAFETY: `Engine::Avx` is built only after `avx_available` is true.
        #[cfg(target_arch = "x86_64")]
        Engine::Avx => unsafe { simd_x86::dot(input, origin, coeffs) },
    }
}

fn dot_scalar(input: &[f32], origin: i64, coeffs: &[f32]) -> f64 {
    let mut acc = 0.0_f64;
    let mut index = origin;
    for coeff in coeffs {
        if let Some(sample) = sample_at(input, index) {
            acc += f64::from(sample) * f64::from(*coeff);
        }
        index = match index.checked_add(1) {
            Some(next) => next,
            None => break,
        };
    }
    acc
}

/// 48 kHz to 16 kHz association. Phase 0, then 1, then 2. Inside a phase,
/// tap `q` adds into partial `q % 4`. The partials fold `p0 + p1 + p2 + p3`.
/// Samples outside `at` are skipped. Products are not fused.
pub(crate) fn dot_48(origin: i64, row: &[f32], mut at: impl FnMut(i64) -> Option<f32>) -> f64 {
    let mut partial = [0.0_f64; 4];
    let mut q = 0_usize;
    for phase in 0..3_usize {
        let mut k = phase;
        while k < row.len() {
            let Some(index) = origin.checked_add(k as i64) else {
                break;
            };
            if let Some(sample) = at(index) {
                partial[q & 3] += f64::from(sample) * f64::from(row[k]);
            }
            q = match q.checked_add(1) {
                Some(next) => next,
                None => break,
            };
            k = match k.checked_add(3) {
                Some(next) => next,
                None => break,
            };
        }
    }
    let mut acc = partial[0];
    acc += partial[1];
    acc += partial[2];
    acc += partial[3];
    acc
}

fn sample_at(input: &[f32], index: i64) -> Option<f32> {
    if index < 0 {
        return None;
    }
    let Ok(index) = usize::try_from(index) else {
        return None;
    };
    input.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, PHASES, TAPS, TAPS_8_16, TAPS_32_16, TAPS_48_16, finite_sample, i128_or_overflow,
        raw_samples,
    };
    use crate::coefficients::Table;
    use crate::{Error, Rate, Ratio, group_delay, output_len};

    fn rate(hz: i64) -> Rate {
        match Rate::new(hz) {
            Ok(rate) => rate,
            Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
        }
    }

    fn unique_max_index(samples: &[f32]) -> usize {
        let mut max_at = 0;
        let mut max = f32::NEG_INFINITY;
        let mut hits = 0_usize;
        for (index, sample) in samples.iter().enumerate() {
            if *sample > max {
                max = *sample;
                max_at = index;
                hits = 1;
            } else if *sample == max {
                hits += 1;
            }
        }
        assert_eq!(hits, 1, "maximum {max} occurred {hits} times");
        assert!(max.is_finite() && max > 0.0, "peak {max}");
        max_at
    }

    #[test]
    fn impulse_raw_peak_is_the_group_delay() {
        for (from_hz, to_hz, taps, delay_samples) in [
            (8_000, 16_000, TAPS_8_16, 16),
            (32_000, 16_000, TAPS_32_16, 169),
            (48_000, 16_000, TAPS_48_16, 221),
            (16_000, 48_000, TAPS, 186),
        ] {
            let from = rate(from_hz);
            let to = rate(to_hz);
            let ratio = Ratio::reduce(from, to);
            let mut input = vec![0.0_f32; 384];
            input[0] = 1.0;
            let raw = match raw_samples(&input, from, to, Engine::Scalar, false, 0) {
                Ok(raw) => raw,
                Err(err) => panic!("{from_hz} Hz to {to_hz} Hz raw impulse failed: {err:?}"),
            };
            let delay = match group_delay(taps, PHASES, ratio) {
                Ok(delay) => delay,
                Err(err) => panic!("{from_hz} Hz to {to_hz} Hz group delay failed: {err:?}"),
            };
            let expected = match output_len(taps, PHASES, ratio, 384) {
                Ok(len) => len,
                Err(err) => panic!("{from_hz} Hz to {to_hz} Hz output_len failed: {err:?}"),
            };
            assert_eq!(delay, delay_samples, "{from_hz} Hz to {to_hz} Hz");
            assert_eq!(
                u64::try_from(raw.len()).ok(),
                Some(expected),
                "{from_hz} Hz to {to_hz} Hz raw length"
            );
            let delay_index = match usize::try_from(delay) {
                Ok(index) => index,
                Err(_) => panic!("delay {delay} does not fit"),
            };
            assert_eq!(
                unique_max_index(&raw),
                delay_index,
                "{from_hz} Hz to {to_hz} Hz"
            );
        }
    }

    #[test]
    fn finite_sample_rejects_a_sum_past_f32() {
        let too_wide = f64::from(f32::MAX) * 2.0;
        assert_eq!(finite_sample(too_wide), Err(Error::Range));
        assert_eq!(finite_sample(-too_wide), Err(Error::Range));
        assert_eq!(finite_sample(f64::INFINITY), Err(Error::Range));
        assert_eq!(finite_sample(f64::NAN), Err(Error::Range));
        assert_eq!(finite_sample(0.0), Ok(0.0));
        assert_eq!(finite_sample(f64::from(f32::MAX)), Ok(f32::MAX));
    }

    #[test]
    fn index_and_arithmetic_overflow_are_rejected() {
        let empty = Table::empty();
        assert_eq!(empty.row(usize::MAX), Err(Error::Overflow));
        assert_eq!(empty.row(1), Err(Error::Overflow));
        assert_eq!(i128_or_overflow(None), Err(Error::Overflow));
    }
}
