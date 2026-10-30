//! Polyphase coefficients. 8 kHz to 16 kHz stores a 33-tap halfband. 32 kHz
//! and 48 kHz to 16 kHz each store one Kaiser row. 96 kHz and 192 kHz to 16 kHz
//! each store the first stage of a cascade into that 32 kHz row. 22.05 kHz to
//! 16 kHz and 44.1 kHz to 16 kHz each store a Kaiser phase table. Every other
//! ratio uses the Blackman-Harris 2 seed.

use super::{Error, Rate, Ratio, TABLE_BUDGET_BYTES};

pub(crate) const TAPS: u32 = 373;
/// 8 kHz to 16 kHz. Halfband interpolator, Kaiser beta 6. Phase 0 is the unit
/// impulse. Phase 128 is the half-sample sinc, normalized on its own.
pub(crate) const TAPS_8_16: u32 = 33;
/// 32 kHz to 16 kHz. Kaiser, attenuation design 160 dB, pass edge 7000 Hz,
/// stop edge 8000 Hz. The reduced ratio 2/1 keeps one phase.
pub(crate) const TAPS_32_16: u32 = 339;
/// 48 kHz to 16 kHz. Kaiser, attenuation design 140 dB, pass edge 7000 Hz,
/// stop edge 8000 Hz.
pub(crate) const TAPS_48_16: u32 = 443;
/// 96 kHz to 16 kHz group delay, in taps. The cascade peaks at output 182, so
/// this is `2 * 182 + 1`. [`STAGE_96_16`] is the stored 96 kHz to 32 kHz row.
pub(crate) const TAPS_96_16: u32 = 365;
/// 96 kHz to 32 kHz. Kaiser, attenuation design 140 dB, pass edge 7000 Hz,
/// stop edge 24000 Hz. The length is the next odd integer at or above
/// `(140 - 8) / (2.285 * 2π * 17000 / 96000)`.
pub(crate) const STAGE_96_16: u32 = 53;
/// 192 kHz to 16 kHz group delay, in taps. The cascade peaks at output 195,
/// so this is `2 * 195 + 1`. [`STAGE_192_16`] is the stored 192 kHz to 32 kHz row.
pub(crate) const TAPS_192_16: u32 = 391;
/// 192 kHz to 32 kHz. Kaiser, attenuation design 140 dB, pass edge 7000 Hz,
/// stop edge 24000 Hz. The length is the next odd integer at or above
/// `(140 - 8) / (2.285 * 2π * 17000 / 192000)`.
pub(crate) const STAGE_192_16: u32 = 105;
/// 44.1 kHz to 16 kHz. Kaiser, attenuation design 160 dB, pass edge 7000 Hz,
/// stop edge 8000 Hz. The reduced ratio 441/160 stores 320 phases, one per
/// denominator step, so the fractional delay lands on a row.
pub(crate) const TAPS_44_16: u32 = 467;
/// 22.05 kHz to 16 kHz. Kaiser, attenuation design 160 dB, pass edge 7000 Hz,
/// stop edge 8000 Hz. The reduced ratio 441/320 stores 640 phases, one per
/// denominator step. The length is the next odd integer at or above
/// `(160 - 8) / (2.285 * 2π * 1000 / 22050)`.
pub(crate) const TAPS_22_16: u32 = 235;
pub(crate) const PHASES: u32 = 256;

/// Stored phase rows for a reduced ratio. 441/320 and 441/160 use the
/// denominator of the placement formula, `out_rate * 2`, so every output
/// fraction is zero. Every other ratio keeps [`PHASES`].
pub(crate) const fn phase_count(in_rate: u64, out_rate: u64) -> u32 {
    if in_rate == 441 && out_rate == 320 {
        640
    } else if in_rate == 441 && out_rate == 160 {
        320
    } else {
        PHASES
    }
}

/// Cutoff at the middle of the 7000–8000 Hz transition, as a fraction of 32 kHz.
const KAISER_32_16_FC: f64 = 7_500.0 / 32_000.0;
const KAISER_32_16_ATTEN_DB: f64 = 160.0;
/// Cutoff at the middle of the 7000–8000 Hz transition, as a fraction of 48 kHz.
const KAISER_48_16_FC: f64 = 7_500.0 / 48_000.0;
const KAISER_48_16_ATTEN_DB: f64 = 140.0;
/// Cutoff at the middle of the 7000–24000 Hz transition, as a fraction of 96 kHz.
const KAISER_96_16_FC: f64 = 15_500.0 / 96_000.0;
const KAISER_96_16_ATTEN_DB: f64 = 140.0;
/// Cutoff at the middle of the 7000–24000 Hz transition, as a fraction of 192 kHz.
const KAISER_192_16_FC: f64 = 15_500.0 / 192_000.0;
const KAISER_192_16_ATTEN_DB: f64 = 140.0;
/// Cutoff at the middle of the 7000–8000 Hz transition, as a fraction of 44.1 kHz.
const KAISER_44_16_FC: f64 = 7_500.0 / 44_100.0;
const KAISER_44_16_ATTEN_DB: f64 = 160.0;
/// Cutoff at the middle of the 7000–8000 Hz transition, as a fraction of 22.05 kHz.
const KAISER_22_16_FC: f64 = 7_500.0 / 22_050.0;
const KAISER_22_16_ATTEN_DB: f64 = 160.0;
/// Kaiser window on the half-sample sinc. The passband gate is 100–3500 Hz.
const HALFBAND_8_16_BETA: f64 = 6.0;

/// Owned rows are private to one call. Shared rows are the process-wide 24 kHz
/// pair. Static rows are the 22.05, 32, 44.1, 48, 96, and 192 kHz tables in
/// BSS. Cloning any of them does not copy the coefficients.
#[derive(Clone)]
enum Coeffs {
    Owned(Vec<f32>),
    Shared(std::sync::Arc<Vec<f32>>),
    Static(&'static [f32]),
}

impl std::ops::Deref for Coeffs {
    type Target = [f32];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(values) => values.as_slice(),
            Self::Shared(values) => values.as_slice(),
            Self::Static(values) => values,
        }
    }
}

/// One process-wide coefficient image. The bytes live in BSS, so filling them
/// does not call the allocator. `Once` publishes the slice only after every
/// element has been written.
struct StaticF32<const N: usize> {
    once: std::sync::Once,
    failed: std::sync::atomic::AtomicBool,
    values: std::cell::UnsafeCell<std::mem::MaybeUninit<[f32; N]>>,
}

// SAFETY: `get` hands out the slice only after `Once` completes a successful
// fill. That fill is the only write. A failed fill sets `failed` and returns
// no pointer. Readers share the immutable slice.
unsafe impl<const N: usize> Sync for StaticF32<N> {}

impl<const N: usize> StaticF32<N> {
    const fn new() -> Self {
        Self {
            once: std::sync::Once::new(),
            failed: std::sync::atomic::AtomicBool::new(false),
            values: std::cell::UnsafeCell::new(std::mem::MaybeUninit::uninit()),
        }
    }

    fn get(
        &self,
        init: impl FnOnce(&mut [std::mem::MaybeUninit<f32>]) -> Result<(), Error>,
    ) -> Result<&[f32], Error> {
        let mut init_error = None;
        self.once.call_once(|| {
            // SAFETY: `call_once` is the only writer. The slice covers the
            // uninitialized array and no element is read before `write`.
            let slots = unsafe {
                std::slice::from_raw_parts_mut(
                    (*self.values.get())
                        .as_mut_ptr()
                        .cast::<std::mem::MaybeUninit<f32>>(),
                    N,
                )
            };
            if let Err(err) = init(slots) {
                self.failed
                    .store(true, std::sync::atomic::Ordering::Release);
                init_error = Some(err);
            }
        });
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(init_error.unwrap_or(Error::Seed));
        }
        // SAFETY: a successful fill wrote every element before `call_once`
        // returned. The array is never written again.
        Ok(unsafe { std::slice::from_raw_parts((*self.values.get()).as_ptr().cast::<f32>(), N) })
    }
}

/// Retain visited phases and interpolation neighbors on compact grids.
#[derive(Clone)]
pub(crate) struct Table {
    values: Coeffs,
    stride: usize,
    pub(crate) taps: u32,
    phases: u32,
    phase_rows: Vec<u16>,
}

/// Scale the input-domain support to preserve the 3:1 transition width.
pub(crate) fn taps_for(from: Rate, to: Rate) -> Result<u32, Error> {
    if from.hz() == 8_000 && to.hz() == 16_000 {
        return Ok(TAPS_8_16);
    }
    if from.hz() == 32_000 && to.hz() == 16_000 {
        return Ok(TAPS_32_16);
    }
    if from.hz() == 48_000 && to.hz() == 16_000 {
        return Ok(TAPS_48_16);
    }
    if from.hz() == 96_000 && to.hz() == 16_000 {
        return Ok(TAPS_96_16);
    }
    if from.hz() == 192_000 && to.hz() == 16_000 {
        return Ok(TAPS_192_16);
    }
    if from.hz() == 44_100 && to.hz() == 16_000 {
        return Ok(TAPS_44_16);
    }
    if from.hz() == 22_050 && to.hz() == 16_000 {
        return Ok(TAPS_22_16);
    }
    let half = (u128::from(from.hz()) * 62)
        .div_ceil(u128::from(to.hz()))
        .max(u128::from((TAPS - 1) / 2));
    u32::try_from(2 * half + 1).map_err(|_| Error::Overflow)
}

impl Table {
    pub(crate) fn new(from: Rate, to: Rate) -> Result<Self, Error> {
        // These tables are filled in BSS. `assemble` borrows that image and
        // does not allocate a row.
        if to.hz() == 16_000
            && matches!(
                from.hz(),
                22_050 | 32_000 | 44_100 | 48_000 | 96_000 | 192_000
            )
        {
            return assemble(from, to);
        }
        if let Some(hit) = recall_table(from.hz(), to.hz()) {
            return Ok(hit);
        }
        let built = assemble(from, to)?;
        // One shared copy. `Arc<[f32]>::from` would allocate a second buffer.
        // 24 kHz shares so a later call does not allocate the rows again.
        let built = if to.hz() == 16_000 && from.hz() == 24_000 {
            built.share()
        } else {
            built
        };
        remember_table(from.hz(), to.hz(), &built);
        Ok(built)
    }

    fn share(self) -> Self {
        let values = match self.values {
            Coeffs::Owned(values) => Coeffs::Shared(std::sync::Arc::new(values)),
            shared => shared,
        };
        Self {
            values,
            stride: self.stride,
            taps: self.taps,
            phases: self.phases,
            phase_rows: self.phase_rows,
        }
    }

    pub(crate) fn phases(&self) -> usize {
        self.phases as usize
    }

    #[cfg(test)]
    fn values_ptr(&self) -> *const f32 {
        self.values.as_ptr()
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            values: Coeffs::Owned(Vec::new()),
            stride: 1,
            taps: TAPS,
            phases: PHASES,
            phase_rows: Vec::new(),
        }
    }
}

/// 24 kHz rebuilds its two rows once and keeps that shared copy. 22.05, 32,
/// 44.1, and 48 kHz use [`StaticF32`] and do not enter this cache. Other rates
/// stay uncached so a one-shot still allocates only its own row.
fn cacheable_rate(from_hz: u64, to_hz: u64) -> bool {
    to_hz == 16_000 && from_hz == 24_000
}

struct CachedTable {
    from_hz: u64,
    to_hz: u64,
    table: Table,
}

fn table_cache() -> &'static std::sync::Mutex<Vec<CachedTable>> {
    static CACHE: std::sync::Mutex<Vec<CachedTable>> = std::sync::Mutex::new(Vec::new());
    &CACHE
}

fn recall_table(from_hz: u64, to_hz: u64) -> Option<Table> {
    if !cacheable_rate(from_hz, to_hz) {
        return None;
    }
    let guard = match table_cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .iter()
        .find(|entry| entry.from_hz == from_hz && entry.to_hz == to_hz)
        .map(|entry| entry.table.clone())
}

fn remember_table(from_hz: u64, to_hz: u64, table: &Table) {
    if !cacheable_rate(from_hz, to_hz) {
        return;
    }
    let mut guard = match table_cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard
        .iter()
        .any(|entry| entry.from_hz == from_hz && entry.to_hz == to_hz)
    {
        return;
    }
    if guard.len() == 4 {
        guard.remove(0);
    }
    guard.push(CachedTable {
        from_hz,
        to_hz,
        table: table.clone(),
    });
}

fn assemble(from: Rate, to: Rate) -> Result<Table, Error> {
    if from.hz() == 8_000 && to.hz() == 16_000 {
        return halfband_8_16();
    }
    if from.hz() == 32_000 && to.hz() == 16_000 {
        return kaiser_32_16();
    }
    if from.hz() == 48_000 && to.hz() == 16_000 {
        return kaiser_48_16();
    }
    if from.hz() == 96_000 && to.hz() == 16_000 {
        return kaiser_96_16();
    }
    if from.hz() == 192_000 && to.hz() == 16_000 {
        return kaiser_192_16();
    }
    if from.hz() == 44_100 && to.hz() == 16_000 {
        return kaiser_44_16();
    }
    if from.hz() == 22_050 && to.hz() == 16_000 {
        return kaiser_22_16();
    }
    let denominator = Ratio::reduce(from, to).denominator();
    let stride = if u64::from(PHASES).is_multiple_of(denominator) {
        (u64::from(PHASES) / denominator) as usize
    } else {
        1
    };
    let indexed = stride == 1 && denominator < u64::from(PHASES / 2);
    let mut active = [false; PHASES as usize];
    if indexed {
        for remainder in 0..denominator {
            let scaled = remainder * u64::from(PHASES);
            let phase = (scaled / denominator) as usize;
            active[phase] = true;
            if scaled % denominator != 0 {
                active[(phase + 1) % PHASES as usize] = true;
            }
        }
    } else {
        for phase in (0..PHASES as usize).step_by(stride) {
            active[phase] = true;
        }
    }
    let taps = taps_for(from, to)?;
    let rows = active.iter().filter(|used| **used).count() as u32;
    let count = blob_len(taps, rows)? / 4;
    let alpha = alpha(0.95, from, to)?;
    let norm = phase_zero_norm(alpha, taps, PHASES)?;
    let mut phase_rows = Vec::new();
    if indexed {
        phase_rows
            .try_reserve_exact(PHASES as usize)
            .map_err(|_| Error::Overflow)?;
        phase_rows.resize(PHASES as usize, u16::MAX);
    }
    let mut values = Vec::new();
    if values.try_reserve_exact(count).is_err() {
        return Err(Error::Overflow);
    }
    for (phase, used) in active.iter().enumerate() {
        if !used {
            continue;
        }
        if indexed {
            phase_rows[phase] = (values.len() / taps as usize) as u16;
        }
        for tap in 0..taps {
            let sample = (raw(alpha, taps, PHASES, phase as u32, tap) / norm) as f32;
            if !sample.is_finite() {
                return Err(Error::Seed);
            }
            values.push(sample);
        }
    }
    Ok(Table {
        values: Coeffs::Owned(values),
        stride,
        taps,
        phases: PHASES,
        phase_rows,
    })
}

impl Table {
    pub(crate) fn row(&self, phase: usize) -> Result<&[f32], Error> {
        if phase >= self.phases as usize {
            return Err(Error::Overflow);
        }
        let row = if self.phase_rows.is_empty() {
            if !phase.is_multiple_of(self.stride) {
                return Err(Error::Overflow);
            }
            phase / self.stride
        } else {
            let row = usize::from(self.phase_rows[phase]);
            if row == usize::from(u16::MAX) {
                return Err(Error::Overflow);
            }
            row
        };
        let start = row * self.taps as usize;
        self.values
            .get(start..start + self.taps as usize)
            .ok_or(Error::Overflow)
    }
}

struct Parsed {
    cutoff: f64,
    phases: u32,
    taps: u32,
}

/// Rebuild the polyphase coefficient blob for one Blackman-Harris 2 seed.
///
/// `seed` is four space-separated fields: window name, cutoff, phase count,
/// and tap count. The window must be `blackman-harris-2`. `from` is the input
/// rate and `to` is the output rate. `alpha` is `min(cutoff * to / from, 1)`.
/// Each coefficient is an `f32` in little-endian, phase-major, with taps
/// inside the phase. Phase 0's raw sum normalizes every phase.
///
/// [`Error::Overflow`] means `taps * phases * 4` does not fit in `u64` or
/// exceeds [`TABLE_BUDGET_BYTES`]. Nothing is allocated in that case.
/// [`Error::Seed`] means the seed is not a design this generator builds,
/// `alpha` is not positive and finite, or a coefficient was not finite.
///
/// [`Error::Overflow`]: crate::Error::Overflow
/// [`Error::Seed`]: crate::Error::Seed
/// [`TABLE_BUDGET_BYTES`]: crate::TABLE_BUDGET_BYTES
#[must_use = "a rejected coefficient table is an error"]
pub fn coefficient_blob(seed: &str, from: Rate, to: Rate) -> Result<Vec<u8>, Error> {
    let parsed = parse_seed(seed)?;
    let len = blob_len(parsed.taps, parsed.phases)?;
    let alpha = alpha(parsed.cutoff, from, to)?;
    let norm = phase_zero_norm(alpha, parsed.taps, parsed.phases)?;
    encode(alpha, parsed.taps, parsed.phases, norm, len)
}

fn parse_seed(seed: &str) -> Result<Parsed, Error> {
    let Some((window, cutoff_text, phases_text, taps_text)) = four_fields(seed) else {
        return Err(Error::Seed);
    };
    if window != "blackman-harris-2" {
        return Err(Error::Seed);
    }
    let cutoff = match cutoff_text.parse::<f64>() {
        Ok(cutoff) if cutoff.is_finite() => cutoff,
        _ => return Err(Error::Seed),
    };
    let phases = match phases_text.parse::<u32>() {
        Ok(phases) if phases > 0 => phases,
        _ => return Err(Error::Seed),
    };
    let taps = match taps_text.parse::<u32>() {
        Ok(taps) if taps >= 2 => taps,
        _ => return Err(Error::Seed),
    };
    Ok(Parsed {
        cutoff,
        phases,
        taps,
    })
}

fn four_fields(seed: &str) -> Option<(&str, &str, &str, &str)> {
    let (window, rest) = seed.split_once(' ')?;
    let (cutoff, rest) = rest.split_once(' ')?;
    let (phases, taps) = rest.split_once(' ')?;
    if window.is_empty()
        || cutoff.is_empty()
        || phases.is_empty()
        || taps.is_empty()
        || taps.contains(' ')
    {
        return None;
    }
    Some((window, cutoff, phases, taps))
}

/// Byte length of the phase-major `f32` table. Overflow allocates nothing.
fn blob_len(taps: u32, phases: u32) -> Result<usize, Error> {
    let bytes = match u64::from(taps)
        .checked_mul(u64::from(phases))
        .and_then(|count| count.checked_mul(4))
    {
        Some(bytes) if bytes <= TABLE_BUDGET_BYTES => bytes,
        Some(_) | None => return Err(Error::Overflow),
    };
    match usize::try_from(bytes) {
        Ok(len) => Ok(len),
        Err(_) => Err(Error::Overflow),
    }
}

fn alpha(cutoff: f64, from: Rate, to: Rate) -> Result<f64, Error> {
    let in_rate = from.hz() as f64;
    let out_rate = to.hz() as f64;
    let alpha = (cutoff * out_rate / in_rate).min(1.0);
    if alpha.is_finite() && alpha > 0.0 {
        Ok(alpha)
    } else {
        Err(Error::Seed)
    }
}

// One norm, from phase 0. Later phases are not scaled by their own sum.
fn phase_zero_norm(alpha: f64, taps: u32, phases: u32) -> Result<f64, Error> {
    let mut norm = 0.0_f64;
    for tap in 0..taps {
        let coeff = raw(alpha, taps, phases, 0, tap);
        if !coeff.is_finite() {
            return Err(Error::Seed);
        }
        norm += coeff;
    }
    if norm.is_finite() && norm != 0.0 {
        Ok(norm)
    } else {
        Err(Error::Seed)
    }
}

fn encode(alpha: f64, taps: u32, phases: u32, norm: f64, len: usize) -> Result<Vec<u8>, Error> {
    let mut blob = Vec::new();
    match blob.try_reserve_exact(len) {
        Ok(()) => {}
        Err(_) => return Err(Error::Overflow),
    }
    for phase in 0..phases {
        for tap in 0..taps {
            let value = raw(alpha, taps, phases, phase, tap) / norm;
            if !value.is_finite() {
                return Err(Error::Seed);
            }
            let sample = value as f32;
            if !sample.is_finite() {
                return Err(Error::Seed);
            }
            blob.extend_from_slice(&sample.to_le_bytes());
        }
    }
    Ok(blob)
}

fn raw(alpha: f64, taps: u32, phases: u32, phase: u32, tap: u32) -> f64 {
    let offset =
        f64::from(tap) - (f64::from(taps) - 1.0) / 2.0 - f64::from(phase) / f64::from(phases);
    sinc(alpha * offset) * blackman_harris_2(offset, taps)
}

fn sinc(u: f64) -> f64 {
    if u == 0.0 {
        1.0
    } else {
        let x = std::f64::consts::PI * u;
        x.sin() / x
    }
}

/// Square of the symmetric 4-term Blackman-Harris window. Denominator is `taps - 1`.
fn blackman_harris_2(offset: f64, taps: u32) -> f64 {
    let denom = f64::from(taps) - 1.0;
    let n = offset + denom / 2.0;
    let theta = 2.0 * std::f64::consts::PI * n / denom;
    let bh = 0.35875 - 0.48829 * theta.cos() + 0.14128 * (2.0 * theta).cos()
        - 0.01168 * (3.0 * theta).cos();
    bh * bh
}

fn kaiser_48_16() -> Result<Table, Error> {
    const N: usize = TAPS_48_16 as usize;
    static ROWS: StaticF32<N> = StaticF32::new();
    let values = ROWS.get(|slots| {
        let taps = TAPS_48_16;
        let half = f64::from((taps - 1) / 2);
        // Beta for a Kaiser window when the attenuation design is above 50 dB.
        let beta = 0.1102 * (KAISER_48_16_ATTEN_DB - 8.7);
        let scale = bessel_i0(beta);
        if !scale.is_finite() || scale == 0.0 {
            return Err(Error::Seed);
        }
        let mut norm = 0.0_f64;
        for tap in 0..taps {
            let coeff = kaiser_coeff(tap, half, beta, scale);
            if !coeff.is_finite() {
                return Err(Error::Seed);
            }
            norm += coeff;
        }
        if !norm.is_finite() || norm == 0.0 {
            return Err(Error::Seed);
        }
        for tap in 0..taps {
            let sample = (kaiser_coeff(tap, half, beta, scale) / norm) as f32;
            if !sample.is_finite() {
                return Err(Error::Seed);
            }
            slots[tap as usize].write(sample);
        }
        Ok(())
    })?;
    Ok(Table {
        values: Coeffs::Static(values),
        stride: PHASES as usize,
        taps: TAPS_48_16,
        phases: PHASES,
        phase_rows: Vec::new(),
    })
}

fn kaiser_coeff(tap: u32, half: f64, beta: f64, scale: f64) -> f64 {
    let offset = f64::from(tap) - half;
    let ideal = if offset == 0.0 {
        2.0 * KAISER_48_16_FC
    } else {
        let angle = 2.0 * std::f64::consts::PI * KAISER_48_16_FC * offset;
        angle.sin() / (std::f64::consts::PI * offset)
    };
    let ratio = offset / half;
    let inside = (1.0 - ratio * ratio).max(0.0);
    ideal * bessel_i0(beta * inside.sqrt()) / scale
}

fn kaiser_96_16() -> Result<Table, Error> {
    const N: usize = STAGE_96_16 as usize;
    static ROWS: StaticF32<N> = StaticF32::new();
    kaiser_one_row(&ROWS, STAGE_96_16, KAISER_96_16_FC, KAISER_96_16_ATTEN_DB)
}

fn kaiser_192_16() -> Result<Table, Error> {
    const N: usize = STAGE_192_16 as usize;
    static ROWS: StaticF32<N> = StaticF32::new();
    kaiser_one_row(
        &ROWS,
        STAGE_192_16,
        KAISER_192_16_FC,
        KAISER_192_16_ATTEN_DB,
    )
}

fn kaiser_one_row<const N: usize>(
    rows: &'static StaticF32<N>,
    taps: u32,
    fc: f64,
    atten_db: f64,
) -> Result<Table, Error> {
    let values = rows.get(|slots| fill_kaiser_row(slots, taps, fc, atten_db))?;
    Ok(Table {
        values: Coeffs::Static(values),
        stride: PHASES as usize,
        taps,
        phases: PHASES,
        phase_rows: Vec::new(),
    })
}

fn fill_kaiser_row(
    slots: &mut [std::mem::MaybeUninit<f32>],
    taps: u32,
    fc: f64,
    atten_db: f64,
) -> Result<(), Error> {
    if slots.len() != taps as usize {
        return Err(Error::Seed);
    }
    let half = f64::from((taps - 1) / 2);
    let beta = 0.1102 * (atten_db - 8.7);
    let scale = bessel_i0(beta);
    if !scale.is_finite() || scale == 0.0 {
        return Err(Error::Seed);
    }
    let mut norm = 0.0_f64;
    for tap in 0..taps {
        let coeff = kaiser_lowpass(tap, half, beta, scale, fc);
        if !coeff.is_finite() {
            return Err(Error::Seed);
        }
        norm += coeff;
    }
    if !norm.is_finite() || norm == 0.0 {
        return Err(Error::Seed);
    }
    for tap in 0..taps {
        let sample = (kaiser_lowpass(tap, half, beta, scale, fc) / norm) as f32;
        if !sample.is_finite() {
            return Err(Error::Seed);
        }
        slots[tap as usize].write(sample);
    }
    Ok(())
}

fn kaiser_lowpass(tap: u32, half: f64, beta: f64, scale: f64, fc: f64) -> f64 {
    let offset = f64::from(tap) - half;
    let ideal = if offset == 0.0 {
        2.0 * fc
    } else {
        let angle = 2.0 * std::f64::consts::PI * fc * offset;
        angle.sin() / (std::f64::consts::PI * offset)
    };
    let ratio = offset / half;
    let inside = (1.0 - ratio * ratio).max(0.0);
    ideal * bessel_i0(beta * inside.sqrt()) / scale
}

fn kaiser_32_16() -> Result<Table, Error> {
    const N: usize = TAPS_32_16 as usize;
    static ROWS: StaticF32<N> = StaticF32::new();
    let values = ROWS.get(|slots| {
        let taps = TAPS_32_16;
        let half = f64::from((taps - 1) / 2);
        let beta = 0.1102 * (KAISER_32_16_ATTEN_DB - 8.7);
        let scale = bessel_i0(beta);
        if !scale.is_finite() || scale == 0.0 {
            return Err(Error::Seed);
        }
        let mut norm = 0.0_f64;
        for tap in 0..taps {
            let coeff = kaiser_32_coeff(tap, half, beta, scale);
            if !coeff.is_finite() {
                return Err(Error::Seed);
            }
            norm += coeff;
        }
        if !norm.is_finite() || norm == 0.0 {
            return Err(Error::Seed);
        }
        for tap in 0..taps {
            let sample = (kaiser_32_coeff(tap, half, beta, scale) / norm) as f32;
            if !sample.is_finite() {
                return Err(Error::Seed);
            }
            slots[tap as usize].write(sample);
        }
        Ok(())
    })?;
    Ok(Table {
        values: Coeffs::Static(values),
        stride: PHASES as usize,
        taps: TAPS_32_16,
        phases: PHASES,
        phase_rows: Vec::new(),
    })
}

fn kaiser_32_coeff(tap: u32, half: f64, beta: f64, scale: f64) -> f64 {
    let offset = f64::from(tap) - half;
    let ideal = if offset == 0.0 {
        2.0 * KAISER_32_16_FC
    } else {
        let angle = 2.0 * std::f64::consts::PI * KAISER_32_16_FC * offset;
        angle.sin() / (std::f64::consts::PI * offset)
    };
    let ratio = offset / half;
    let inside = (1.0 - ratio * ratio).max(0.0);
    ideal * bessel_i0(beta * inside.sqrt()) / scale
}

fn kaiser_44_16() -> Result<Table, Error> {
    // 441/160 stores one row per placement denominator step.
    const PHASE_COUNT: u32 = phase_count(441, 160);
    const N: usize = (TAPS_44_16 as usize) * (PHASE_COUNT as usize);
    static ROWS: StaticF32<N> = StaticF32::new();
    let values = ROWS.get(|slots| {
        let taps = TAPS_44_16;
        let phases = PHASE_COUNT;
        let beta = 0.1102 * (KAISER_44_16_ATTEN_DB - 8.7);
        let scale = bessel_i0(beta);
        if !scale.is_finite() || scale == 0.0 {
            return Err(Error::Seed);
        }
        let mut norm = 0.0_f64;
        for tap in 0..taps {
            let coeff = kaiser_poly(0, tap, taps, phases, KAISER_44_16_FC, beta, scale);
            if !coeff.is_finite() {
                return Err(Error::Seed);
            }
            norm += coeff;
        }
        if !norm.is_finite() || norm == 0.0 {
            return Err(Error::Seed);
        }
        for phase in 0..phases {
            for tap in 0..taps {
                let sample = (kaiser_poly(phase, tap, taps, phases, KAISER_44_16_FC, beta, scale)
                    / norm) as f32;
                if !sample.is_finite() {
                    return Err(Error::Seed);
                }
                let index = (phase as usize) * (taps as usize) + tap as usize;
                slots[index].write(sample);
            }
        }
        Ok(())
    })?;
    Ok(Table {
        values: Coeffs::Static(values),
        stride: 1,
        taps: TAPS_44_16,
        phases: PHASE_COUNT,
        phase_rows: Vec::new(),
    })
}

fn kaiser_22_16() -> Result<Table, Error> {
    // 441/320 stores one row per placement denominator step.
    const PHASE_COUNT: u32 = phase_count(441, 320);
    const N: usize = (TAPS_22_16 as usize) * (PHASE_COUNT as usize);
    static ROWS: StaticF32<N> = StaticF32::new();
    let values = ROWS.get(|slots| {
        let taps = TAPS_22_16;
        let phases = PHASE_COUNT;
        let beta = 0.1102 * (KAISER_22_16_ATTEN_DB - 8.7);
        let scale = bessel_i0(beta);
        if !scale.is_finite() || scale == 0.0 {
            return Err(Error::Seed);
        }
        let mut norm = 0.0_f64;
        for tap in 0..taps {
            let coeff = kaiser_poly(0, tap, taps, phases, KAISER_22_16_FC, beta, scale);
            if !coeff.is_finite() {
                return Err(Error::Seed);
            }
            norm += coeff;
        }
        if !norm.is_finite() || norm == 0.0 {
            return Err(Error::Seed);
        }
        for phase in 0..phases {
            for tap in 0..taps {
                let sample = (kaiser_poly(phase, tap, taps, phases, KAISER_22_16_FC, beta, scale)
                    / norm) as f32;
                if !sample.is_finite() {
                    return Err(Error::Seed);
                }
                let index = (phase as usize) * (taps as usize) + tap as usize;
                slots[index].write(sample);
            }
        }
        Ok(())
    })?;
    Ok(Table {
        values: Coeffs::Static(values),
        stride: 1,
        taps: TAPS_22_16,
        phases: PHASE_COUNT,
        phase_rows: Vec::new(),
    })
}

/// Kaiser phase table for a speech-grid probe. Same `kaiser_poly`, phase-0
/// norm, and `f32` cast as the shipped 22.05 and 44.1 builders. `phases` is
/// the production count for that ratio. `atten_db` must be above 50 so beta
/// stays on that builder's branch, `0.1102 * (atten_db - 8.7)`. The shipped
/// tap counts are not read here.
#[cfg(test)]
pub(crate) fn speech_kaiser(
    fc: f64,
    atten_db: f64,
    taps: u32,
    phases: u32,
) -> Result<Table, Error> {
    if taps < 3 || taps.is_multiple_of(2) || phases == 0 {
        return Err(Error::Overflow);
    }
    if !fc.is_finite() || fc <= 0.0 || !atten_db.is_finite() || atten_db <= 50.0 {
        return Err(Error::Seed);
    }
    let beta = 0.1102 * (atten_db - 8.7);
    let scale = bessel_i0(beta);
    if !scale.is_finite() || scale == 0.0 {
        return Err(Error::Seed);
    }
    let mut norm = 0.0_f64;
    for tap in 0..taps {
        let coeff = kaiser_poly(0, tap, taps, phases, fc, beta, scale);
        if !coeff.is_finite() {
            return Err(Error::Seed);
        }
        norm += coeff;
    }
    if !norm.is_finite() || norm == 0.0 {
        return Err(Error::Seed);
    }
    let count = blob_len(taps, phases)? / 4;
    let mut values = Vec::new();
    if values.try_reserve_exact(count).is_err() {
        return Err(Error::Overflow);
    }
    for phase in 0..phases {
        for tap in 0..taps {
            let sample = (kaiser_poly(phase, tap, taps, phases, fc, beta, scale) / norm) as f32;
            if !sample.is_finite() {
                return Err(Error::Seed);
            }
            values.push(sample);
        }
    }
    Ok(Table {
        values: Coeffs::Owned(values),
        stride: 1,
        taps,
        phases,
        phase_rows: Vec::new(),
    })
}

fn kaiser_poly(
    phase: u32,
    tap: u32,
    taps: u32,
    phases: u32,
    fc: f64,
    beta: f64,
    scale: f64,
) -> f64 {
    let half = (f64::from(taps) - 1.0) / 2.0;
    let offset = f64::from(tap) - half - f64::from(phase) / f64::from(phases);
    if offset.abs() > half {
        return 0.0;
    }
    let ideal = if offset == 0.0 {
        2.0 * fc
    } else {
        let angle = 2.0 * std::f64::consts::PI * fc * offset;
        angle.sin() / (std::f64::consts::PI * offset)
    };
    let ratio = offset / half;
    let inside = (1.0 - ratio * ratio).max(0.0);
    ideal * bessel_i0(beta * inside.sqrt()) / scale
}

fn halfband_8_16() -> Result<Table, Error> {
    let taps = TAPS_8_16;
    let scale = bessel_i0(HALFBAND_8_16_BETA);
    if !scale.is_finite() || scale == 0.0 {
        return Err(Error::Seed);
    }
    let mut norm = 0.0_f64;
    for tap in 0..taps {
        let coeff = halfband_odd(tap, taps, scale);
        if !coeff.is_finite() {
            return Err(Error::Seed);
        }
        norm += coeff;
    }
    if !norm.is_finite() || norm == 0.0 {
        return Err(Error::Seed);
    }
    let mut values = Vec::new();
    if values.try_reserve_exact(taps as usize * 2).is_err() {
        return Err(Error::Overflow);
    }
    let half = (taps - 1) / 2;
    for tap in 0..taps {
        values.push(if tap == half { 1.0 } else { 0.0 });
    }
    for tap in 0..taps {
        let sample = (halfband_odd(tap, taps, scale) / norm) as f32;
        if !sample.is_finite() {
            return Err(Error::Seed);
        }
        values.push(sample);
    }
    Ok(Table {
        values: Coeffs::Owned(values),
        stride: (PHASES / 2) as usize,
        taps,
        phases: PHASES,
        phase_rows: Vec::new(),
    })
}

/// Ideal halfband interpolator at half-sample offsets, Kaiser-windowed.
/// `|offset| > half` is outside the phase-0 support and stays zero.
fn halfband_odd(tap: u32, taps: u32, scale: f64) -> f64 {
    let half = (f64::from(taps) - 1.0) / 2.0;
    let offset = f64::from(tap) - half - 0.5;
    if offset.abs() > half {
        return 0.0;
    }
    let angle = std::f64::consts::PI * offset;
    let ideal = angle.sin() / angle;
    let ratio = offset / half;
    let inside = (1.0 - ratio * ratio).max(0.0);
    ideal * bessel_i0(HALFBAND_8_16_BETA * inside.sqrt()) / scale
}

/// Modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0_f64;
    let mut term = 1.0_f64;
    let half = x * 0.5;
    let mut k = 1.0_f64;
    for _ in 0..64 {
        term *= half / k;
        term *= half / k;
        sum += term;
        if term < sum * 1.0e-16 {
            break;
        }
        k += 1.0;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::{
        PHASES, STAGE_96_16, STAGE_192_16, TAPS, TAPS_8_16, TAPS_22_16, TAPS_32_16, TAPS_44_16,
        TAPS_48_16, TAPS_96_16, TAPS_192_16, Table, coefficient_blob, taps_for,
    };
    use crate::{Error, Rate};

    fn rate(hz: i64) -> Rate {
        match Rate::new(hz) {
            Ok(rate) => rate,
            Err(err) => panic!("positive rate: {err:?}"),
        }
    }

    #[test]
    fn compact_tables_keep_the_recorded_coefficients() {
        for (from_hz, to_hz, rows) in [(24_000, 16_000, 2), (16_000, 48_000, 5)] {
            let from = rate(from_hz);
            let to = rate(to_hz);
            let table = match Table::new(from, to) {
                Ok(table) => table,
                Err(err) => panic!("table: {err:?}"),
            };
            let blob = match coefficient_blob("blackman-harris-2 0.95 256 373", from, to) {
                Ok(blob) => blob,
                Err(err) => panic!("blob: {err:?}"),
            };
            assert_eq!(table.values.len(), rows * TAPS as usize);
            let mut visited = 0;
            for phase in 0..PHASES as usize {
                let Ok(row) = table.row(phase) else {
                    continue;
                };
                visited += 1;
                let start = phase * TAPS as usize * 4;
                for (sample, bytes) in row.iter().zip(blob[start..].chunks_exact(4)) {
                    assert_eq!(sample.to_le_bytes().as_slice(), bytes);
                }
            }
            assert_eq!(visited, rows);
            assert_eq!(table.row(PHASES as usize), Err(Error::Overflow));
            if table.stride > 1 {
                assert_eq!(table.row(1), Err(Error::Overflow));
            }
        }
    }

    #[test]
    fn rate_44k_shares_the_cached_f32_rows() {
        let from = rate(44_100);
        let to = rate(16_000);
        let _warm = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("44.1 kHz: {err:?}"),
        };
        let second = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("44.1 kHz again: {err:?}"),
        };
        let third = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("44.1 kHz third: {err:?}"),
        };
        assert_eq!(second.values_ptr(), third.values_ptr());
        let left = match second.row(0) {
            Ok(row) => row,
            Err(err) => panic!("phase 0: {err:?}"),
        };
        let right = match third.row(0) {
            Ok(row) => row,
            Err(err) => panic!("phase 0 again: {err:?}"),
        };
        assert_eq!(left.len(), right.len());
        for (sample, again) in left.iter().zip(right.iter()) {
            assert_eq!(sample.to_bits(), again.to_bits());
        }
    }

    #[test]
    fn rate_22k_shares_the_cached_f32_rows() {
        let from = rate(22_050);
        let to = rate(16_000);
        let _warm = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("22.05 kHz: {err:?}"),
        };
        let second = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("22.05 kHz again: {err:?}"),
        };
        let third = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("22.05 kHz third: {err:?}"),
        };
        assert_eq!(second.values_ptr(), third.values_ptr());
    }

    #[test]
    fn rate_22k_uses_the_kaiser_phases() {
        let from = rate(22_050);
        let to = rate(16_000);
        assert_eq!(taps_for(from, to), Ok(TAPS_22_16));
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        assert_eq!(table.taps, TAPS_22_16);
        assert_eq!(table.phases(), 640);
        let mut visited = 0_usize;
        for phase in 0..640 {
            let row = match table.row(phase) {
                Ok(row) => row,
                Err(err) => panic!("phase {phase}: {err:?}"),
            };
            assert_eq!(row.len(), TAPS_22_16 as usize);
            assert!(row.iter().all(|sample| sample.is_finite()), "phase {phase}");
            visited += 1;
        }
        assert_eq!(visited, 640);
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        let mut sum = 0.0_f64;
        for (index, sample) in row.iter().enumerate() {
            let mirror = row[row.len() - 1 - index];
            assert_eq!(sample.to_bits(), mirror.to_bits(), "tap {index}");
            sum += f64::from(*sample);
        }
        assert!(
            (sum - 1.0).abs() <= 1.0e-5,
            "phase 0 sum {sum} is not within 1e-5 of 1"
        );
        assert!(table.row(256).is_ok());
        assert_eq!(table.row(640), Err(Error::Overflow));
    }

    #[test]
    fn rate_8k_uses_the_halfband() {
        let from = rate(8_000);
        let to = rate(16_000);
        assert_eq!(taps_for(from, to), Ok(TAPS_8_16));
        assert_eq!(taps_for(rate(24_000), rate(48_000)), Ok(TAPS));
        assert_eq!(taps_for(rate(22_050), rate(44_100)), Ok(TAPS));
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        assert_eq!(table.taps, TAPS_8_16);
        let even = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("phase 0: {err:?}"),
        };
        let odd = match table.row((PHASES / 2) as usize) {
            Ok(row) => row,
            Err(err) => panic!("phase 128: {err:?}"),
        };
        assert_eq!(even.len(), TAPS_8_16 as usize);
        assert_eq!(odd.len(), TAPS_8_16 as usize);
        let center = (TAPS_8_16 as usize - 1) / 2;
        for (index, sample) in even.iter().enumerate() {
            let expected = if index == center { 1.0_f32 } else { 0.0 };
            assert_eq!(sample.to_bits(), expected.to_bits(), "even tap {index}");
        }
        let mut sum = 0.0_f64;
        for (index, sample) in odd.iter().enumerate() {
            assert!(sample.is_finite(), "odd tap {index}");
            sum += f64::from(*sample);
        }
        assert!(
            (sum - 1.0).abs() <= 1.0e-5,
            "phase 128 sum {sum} is not within 1e-5 of 1"
        );
        assert_eq!(table.row(1), Err(Error::Overflow));
        let blob = match coefficient_blob("blackman-harris-2 0.95 256 373", from, to) {
            Ok(blob) => blob,
            Err(err) => panic!("blob: {err:?}"),
        };
        assert_ne!(blob.len(), even.len() * 4);
    }

    #[test]
    fn rate_48k_uses_the_kaiser_row() {
        let from = rate(48_000);
        let to = rate(16_000);
        assert_eq!(taps_for(from, to), Ok(TAPS_48_16));
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        assert_eq!(table.taps, TAPS_48_16);
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        assert_eq!(row.len(), TAPS_48_16 as usize);
        let mut sum = 0.0_f64;
        for (index, sample) in row.iter().enumerate() {
            assert!(sample.is_finite(), "tap {index}");
            let mirror = row[row.len() - 1 - index];
            assert_eq!(sample.to_bits(), mirror.to_bits(), "tap {index}");
            sum += f64::from(*sample);
        }
        assert!(
            (sum - 1.0).abs() <= 1.0e-5,
            "phase 0 sum {sum} is not within 1e-5 of 1"
        );
        assert_eq!(table.row(1), Err(Error::Overflow));
        let blob = match coefficient_blob("blackman-harris-2 0.95 256 373", from, to) {
            Ok(blob) => blob,
            Err(err) => panic!("blob: {err:?}"),
        };
        assert_ne!(blob.len(), row.len() * 4);
    }

    #[test]
    fn rate_32k_uses_the_kaiser_row() {
        let from = rate(32_000);
        let to = rate(16_000);
        assert_eq!(taps_for(from, to), Ok(TAPS_32_16));
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        assert_eq!(table.taps, TAPS_32_16);
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        assert_eq!(row.len(), TAPS_32_16 as usize);
        let mut sum = 0.0_f64;
        for (index, sample) in row.iter().enumerate() {
            assert!(sample.is_finite(), "tap {index}");
            let mirror = row[row.len() - 1 - index];
            assert_eq!(sample.to_bits(), mirror.to_bits(), "tap {index}");
            sum += f64::from(*sample);
        }
        assert!(
            (sum - 1.0).abs() <= 1.0e-5,
            "phase 0 sum {sum} is not within 1e-5 of 1"
        );
        assert_eq!(table.row(1), Err(Error::Overflow));
        let blob = match coefficient_blob("blackman-harris-2 0.95 256 373", from, to) {
            Ok(blob) => blob,
            Err(err) => panic!("blob: {err:?}"),
        };
        assert_ne!(blob.len(), row.len() * 4);
    }

    #[test]
    fn rate_44k_uses_the_kaiser_phases() {
        let from = rate(44_100);
        let to = rate(16_000);
        assert_eq!(taps_for(from, to), Ok(TAPS_44_16));
        let table = match Table::new(from, to) {
            Ok(table) => table,
            Err(err) => panic!("table: {err:?}"),
        };
        assert_eq!(table.taps, TAPS_44_16);
        assert_eq!(table.phases(), 320);
        let mut visited = 0_usize;
        for phase in 0..320 {
            let row = match table.row(phase) {
                Ok(row) => row,
                Err(err) => panic!("phase {phase}: {err:?}"),
            };
            assert_eq!(row.len(), TAPS_44_16 as usize);
            assert!(row.iter().all(|sample| sample.is_finite()), "phase {phase}");
            visited += 1;
        }
        assert_eq!(visited, 320);
        let row = match table.row(0) {
            Ok(row) => row,
            Err(err) => panic!("row: {err:?}"),
        };
        let mut sum = 0.0_f64;
        for (index, sample) in row.iter().enumerate() {
            let mirror = row[row.len() - 1 - index];
            assert_eq!(sample.to_bits(), mirror.to_bits(), "tap {index}");
            sum += f64::from(*sample);
        }
        assert!(
            (sum - 1.0).abs() <= 1.0e-5,
            "phase 0 sum {sum} is not within 1e-5 of 1"
        );
        assert!(table.row(256).is_ok());
        assert_eq!(table.row(320), Err(Error::Overflow));
    }

    #[test]
    fn high_rates_use_the_kaiser_row() {
        for (from_hz, delay_taps, stage_taps) in [
            (96_000, TAPS_96_16, STAGE_96_16),
            (192_000, TAPS_192_16, STAGE_192_16),
        ] {
            let from = rate(from_hz);
            let to = rate(16_000);
            assert_eq!(taps_for(from, to), Ok(delay_taps));
            assert_eq!(
                (delay_taps - 1) / 2,
                if from_hz == 96_000 { 182 } else { 195 }
            );
            let table = match Table::new(from, to) {
                Ok(table) => table,
                Err(err) => panic!("table: {err:?}"),
            };
            assert_eq!(table.taps, stage_taps);
            let row = match table.row(0) {
                Ok(row) => row,
                Err(err) => panic!("row: {err:?}"),
            };
            assert_eq!(row.len(), stage_taps as usize);
            let mut sum = 0.0_f64;
            for (index, sample) in row.iter().enumerate() {
                assert!(sample.is_finite(), "{from_hz} tap {index}");
                let mirror = row[row.len() - 1 - index];
                assert_eq!(sample.to_bits(), mirror.to_bits(), "{from_hz} tap {index}");
                sum += f64::from(*sample);
            }
            assert!(
                (sum - 1.0).abs() <= 1.0e-5,
                "{from_hz} phase 0 sum {sum} is not within 1e-5 of 1"
            );
            assert_eq!(table.row(1), Err(Error::Overflow));
        }
        assert_eq!(taps_for(rate(i64::MAX), rate(1)), Err(Error::Overflow));
        assert!(matches!(
            Table::new(rate(1_000_001), rate(3)),
            Err(Error::Overflow)
        ));
        let mapped = Table::new(rate(16_000), rate(48_000)).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(mapped.row(1), Err(Error::Overflow));
    }
}
