//! Independent memory budgets (F07 / TASK-23).
//!
//! Duration is not a memory cap: a 2 h 5.1 split collection is ~7.7 GiB of
//! planar f32 while `speech()` still mixes to one plane. A lifetime
//! compressed-byte counter on `Decoder::feed` also blocks indefinite
//! bounded streaming. Decode, `Decoder::feed` and the M4A/fMP4 index call
//! the fences listed below.
//!
//! Call sites:
//! - finite compressed bytes: `decode` / `decode_with` / `decode_streaming` /
//!   `read` / `read_file_capped` (`InputScope::FiniteComplete`)
//! - streaming compressed bytes: `Decoder::feed` resident buffer and
//!   declared AU length (`InputScope::StreamingFeed`) — no lifetime total
//! - decoded PCM: one-shot plane `reserve`/`resize` in `decode_with`
//! - channels: first-frame lock in `stream/pump.rs` / `stream/m4a.rs`
//! - metadata/index: `isomp4` `stsz`/`stts`/`stsc`/`stco` before `Vec` alloc;
//!   fMP4 push (`stream/fmp4.rs`) buffers the init `moov` and each `moof`
//!   whole under this budget — the resident `feed` cap there is
//!   `max(metadata, buffered)`, one box payload / one AU plus headers
//! - workspace: `Decoder` filterbank + SBR/PS + input buffer (TASK-25)

use crate::options::DEFAULT_MAX_INPUT_BYTES;

/// Collected planar f32 cap (4 GiB). Covers 2 h stereo @ 48 kHz; 2 h 5.1
/// split does not fit (F07). Same default as ryf `max_output_bytes`.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1 << 32;

/// Output / PCE plane ceiling: product layouts are 1–8 (7.1 since TASK-62);
/// a PCE beyond eight planes is a Format error, not a memory bomb.
pub const DEFAULT_MAX_CHANNELS: u32 = 8;

/// M4A table / box-payload bytes (`stsz`/`stts`/`stsc`/`stco` plus names).
pub const DEFAULT_MAX_METADATA_BYTES: u64 = 16 << 20;

/// Sample-table entry count (`stsz` samples, `stts`/`stsc` rows, chunks).
pub const DEFAULT_MAX_INDEX_ENTRIES: u32 = 1 << 20;

/// ISOBMFF box nesting depth.
pub const DEFAULT_MAX_BOX_DEPTH: u32 = 32;

/// Resident codec + compressed-buffer workspace, independent of duration.
pub const DEFAULT_MAX_WORKSPACE_BYTES: u64 = 8 << 20;

/// Streaming `Decoder` compressed bytes after compact (partial AU + unconsumed).
pub const DEFAULT_MAX_BUFFERED_INPUT_BYTES: u64 = 1 << 20;

/// Declared access-unit length (ADTS 13-bit max is 8191; LATM/M4A `stsz`).
pub const DEFAULT_MAX_DECLARED_AU_BYTES: u32 = 64 << 10;

/// LC filterbank overlap (1024) + IMDCT scratch (2048) f32 per channel.
pub const LC_FILTERBANK_BYTES_PER_CH: u64 = (1024 + 2048) * 4;

/// Persistent SBR QMF + history per channel (analysis `x`/`m`, synthesis
/// `v`/`n_mat`, `w_hist` 8×32, `y_prev` 40×64). Not XLow scratch.
pub const SBR_STATE_BYTES_PER_CH: u64 =
    320 * 8 + 32 * 64 * 16 + 1280 * 8 + 128 * 64 * 16 + 8 * 32 * 16 + 40 * 64 * 16;

/// Parametric-stereo lower bound: one stereo QMF pair, 32 slots × 64 bins.
pub const PS_STATE_BYTES: u64 = 2 * 32 * 64 * 16;

/// Which compressed-byte policy applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputScope {
    /// Complete buffer or file: `decode` / `decode_streaming` / `read`.
    FiniteComplete,
    /// Push `Decoder::feed`: resident buffer only, no lifetime total.
    StreamingFeed,
}

/// Which budget a [`BudgetExceeded`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    Input,
    Output,
    Channel,
    Metadata,
    Workspace,
    AccessUnit,
}

impl core::fmt::Display for BudgetKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Channel => "channel",
            Self::Metadata => "metadata",
            Self::Workspace => "workspace",
            Self::AccessUnit => "access-unit",
        })
    }
}

/// Exact-boundary / overflow failure. TASK-24 maps this to a public error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub kind: BudgetKind,
    pub observed: u64,
    pub max: u64,
}

/// Named memory limits. `Default` is the speech-ingest set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemoryBudgets {
    pub max_input_bytes: u64,
    pub max_buffered_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_channels: u32,
    pub max_metadata_bytes: u64,
    pub max_index_entries: u32,
    pub max_box_depth: u32,
    pub max_workspace_bytes: u64,
    pub max_declared_au_bytes: u32,
}

impl Default for MemoryBudgets {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_buffered_input_bytes: DEFAULT_MAX_BUFFERED_INPUT_BYTES,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_channels: DEFAULT_MAX_CHANNELS,
            max_metadata_bytes: DEFAULT_MAX_METADATA_BYTES,
            max_index_entries: DEFAULT_MAX_INDEX_ENTRIES,
            max_box_depth: DEFAULT_MAX_BOX_DEPTH,
            max_workspace_bytes: DEFAULT_MAX_WORKSPACE_BYTES,
            max_declared_au_bytes: DEFAULT_MAX_DECLARED_AU_BYTES,
        }
    }
}

impl MemoryBudgets {
    /// Same fences as [`Default`], but collected PCM uses `u64::MAX / 2`
    /// so an unbounded-duration one-shot still cannot wrap the byte count.
    pub fn unbounded_collection() -> Self {
        Self {
            max_output_bytes: u64::MAX / 2,
            ..Self::default()
        }
    }

    /// Zero in any field disables the fence; reject that like TASK-21.
    pub fn validate(&self) -> Result<(), BudgetExceeded> {
        fn nz(kind: BudgetKind, v: u64) -> Result<(), BudgetExceeded> {
            if v == 0 {
                Err(BudgetExceeded {
                    kind,
                    observed: 0,
                    max: 1,
                })
            } else {
                Ok(())
            }
        }
        nz(BudgetKind::Input, self.max_input_bytes)?;
        nz(BudgetKind::Input, self.max_buffered_input_bytes)?;
        nz(BudgetKind::Output, self.max_output_bytes)?;
        nz(BudgetKind::Channel, u64::from(self.max_channels))?;
        nz(BudgetKind::Metadata, self.max_metadata_bytes)?;
        nz(BudgetKind::Metadata, u64::from(self.max_index_entries))?;
        nz(BudgetKind::Metadata, u64::from(self.max_box_depth))?;
        nz(BudgetKind::Workspace, self.max_workspace_bytes)?;
        nz(
            BudgetKind::AccessUnit,
            u64::from(self.max_declared_au_bytes),
        )?;
        Ok(())
    }

    pub fn check_input_finite(&self, len: u64) -> Result<usize, BudgetExceeded> {
        check_planned(BudgetKind::Input, len, self.max_input_bytes)
    }

    pub fn check_buffered(&self, buffered: u64) -> Result<usize, BudgetExceeded> {
        check_planned(BudgetKind::Input, buffered, self.max_buffered_input_bytes)
    }

    pub fn check_declared_au(&self, len: u64) -> Result<usize, BudgetExceeded> {
        check_planned(
            BudgetKind::AccessUnit,
            len,
            u64::from(self.max_declared_au_bytes),
        )
    }

    pub fn check_channels(&self, n: u32) -> Result<(), BudgetExceeded> {
        if n == 0 || n > self.max_channels {
            return Err(BudgetExceeded {
                kind: BudgetKind::Channel,
                observed: u64::from(n),
                max: u64::from(self.max_channels),
            });
        }
        Ok(())
    }

    pub fn check_output(&self, channels: u32, samples: u64) -> Result<usize, BudgetExceeded> {
        self.check_channels(channels)?;
        let Some(bytes) = pcm_bytes(channels, samples) else {
            return Err(BudgetExceeded {
                kind: BudgetKind::Output,
                observed: u64::MAX,
                max: self.effective_output_bytes(),
            });
        };
        check_planned(BudgetKind::Output, bytes, self.effective_output_bytes())
    }

    pub fn check_index(&self, entries: u64, bytes_per_entry: u64) -> Result<usize, BudgetExceeded> {
        if entries > u64::from(self.max_index_entries) {
            return Err(BudgetExceeded {
                kind: BudgetKind::Metadata,
                observed: entries,
                max: u64::from(self.max_index_entries),
            });
        }
        let Some(bytes) = entries.checked_mul(bytes_per_entry) else {
            return Err(BudgetExceeded {
                kind: BudgetKind::Metadata,
                observed: u64::MAX,
                max: self.max_metadata_bytes,
            });
        };
        check_planned(BudgetKind::Metadata, bytes, self.max_metadata_bytes)
    }

    pub fn check_box_depth(&self, depth: u32) -> Result<(), BudgetExceeded> {
        if depth > self.max_box_depth {
            return Err(BudgetExceeded {
                kind: BudgetKind::Metadata,
                observed: u64::from(depth),
                max: u64::from(self.max_box_depth),
            });
        }
        Ok(())
    }

    pub fn check_workspace(
        &self,
        channels: u32,
        he: bool,
        ps: bool,
    ) -> Result<usize, BudgetExceeded> {
        self.check_channels(channels)?;
        let Some(bytes) = codec_workspace_bytes(channels, he, ps) else {
            return Err(BudgetExceeded {
                kind: BudgetKind::Workspace,
                observed: u64::MAX,
                max: self.max_workspace_bytes,
            });
        };
        check_planned(BudgetKind::Workspace, bytes, self.max_workspace_bytes)
    }

    /// 32-bit hosts cannot `Vec` more than `isize::MAX` bytes.
    pub fn effective_output_bytes(&self) -> u64 {
        self.max_output_bytes.min(isize::MAX as u64)
    }
}

/// `n_ch * n_samples * size_of::<f32>()`; `None` on overflow.
#[inline]
pub fn pcm_bytes(channels: u32, samples: u64) -> Option<u64> {
    u64::from(channels).checked_mul(samples)?.checked_mul(4)
}

/// Codec-only resident workspace (filterbank + optional SBR/PS). The
/// streaming input buffer is a separate [`DEFAULT_MAX_BUFFERED_INPUT_BYTES`]
/// budget.
pub fn codec_workspace_bytes(channels: u32, he: bool, ps: bool) -> Option<u64> {
    let ch = u64::from(channels);
    let mut n = ch.checked_mul(LC_FILTERBANK_BYTES_PER_CH)?;
    if he {
        n = n.checked_add(ch.checked_mul(SBR_STATE_BYTES_PER_CH)?)?;
    }
    if ps {
        n = n.checked_add(PS_STATE_BYTES)?;
    }
    Some(n)
}

/// Lower-bound resident workspace (codec state + streaming input buffer).
/// Collected PCM is the output budget, not this.
pub fn workspace_lower_bound_bytes(channels: u32, he: bool, ps: bool) -> Option<u64> {
    codec_workspace_bytes(channels, he, ps)?.checked_add(DEFAULT_MAX_BUFFERED_INPUT_BYTES)
}

/// `Vec` capacity is `isize::MAX` bytes. `None` if `planned` cannot be a `usize`
/// length on this target.
#[inline]
pub fn allocable_bytes(planned: u64) -> Option<usize> {
    if planned > isize::MAX as u64 {
        return None;
    }
    usize::try_from(planned).ok()
}

/// Finite complete buffers use [`DEFAULT_MAX_INPUT_BYTES`]. Streaming
/// `feed` must not: a lifetime sum is F07 and is a no-go.
#[inline]
pub fn input_cap_applies(scope: InputScope) -> bool {
    matches!(scope, InputScope::FiniteComplete)
}

/// Exact-boundary: `planned == max` is allowed; `planned == max + 1` is not.
/// Overflow and 32-bit `isize::MAX` clip are the same class as over-budget.
/// TASK-24/25 map a failed `try_reserve` to [`BudgetExceeded`] — never panic.
pub fn check_planned(kind: BudgetKind, planned: u64, max: u64) -> Result<usize, BudgetExceeded> {
    if planned > max {
        return Err(BudgetExceeded {
            kind,
            observed: planned,
            max,
        });
    }
    allocable_bytes(planned).ok_or(BudgetExceeded {
        kind,
        observed: planned,
        max: max.min(isize::MAX as u64),
    })
}

#[cfg(test)]
#[path = "budgets_tests.rs"]
mod budgets_tests;
