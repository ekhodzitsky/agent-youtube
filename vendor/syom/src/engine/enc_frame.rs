//! `LcEncoder` — per-frame LC pipeline: attack detector → window sequence
//! state machine → window + forward MDCT → TNS analysis (`enc_tns`) →
//! per-band M/S (`enc_ms`) → psy model (per-band precision targets) →
//! quantize → section plan → rate loop (one global sf offset, `rate`
//! submodule) → `raw_data_block()` bytes.
//!
//! Block switching: OnlyLong → LongStart → EightShort → LongStop; CPE
//! `common_window = 1` ORs both detectors. TNS is long-only unless
//! `with_short_tns`. Grouping/refine/PNS/IS off unless opted in.
//! Causal block switching codes an attack in the first ~448 new samples
//! of a frame on the LongStart's flat region (reduced, not eliminated,
//! pre-echo); the opt-in one-frame lookahead (`lookahead` child module,
//! `crate::EncodeOptions::with_lookahead`) decides frame N from
//! attack(N) || attack(N+1) at one frame of latency.

use super::adts::ADTS_SAMPLE_RATES_HZ;
use super::enc_ms::MsBands;
use super::enc_psy::{AttackDetector, Psy};
use super::enc_quant::{MAX_BANDS, MAX_FLAT_SHORT, QuantChannel, QuantShort};
use super::enc_short::{self, ShortWindows};
use super::enc_tns::EncTns;
use super::error::{Error, Result};
use super::filterbank::window_left;
use super::ics::{WindowSequence, WindowShape};
use super::mdct::mdct_into_f32;
use super::swb::{LONG_WINDOW_LEN, long_offsets, short_offsets};

/// ADTS `raw_data_block` payload ceiling (8191-byte frame − 7-byte header).
pub const MAX_PAYLOAD_BYTES: usize = 8184;
/// AAC-LC max bits per channel per 1024-sample frame (FAAD2 ER / common
/// literature). Independent of the ADTS 13-bit length field.
pub const MAX_BITS_PER_CHANNEL: usize = 6144;
/// Flat quality target: band peaks quantize to about this magnitude before
/// the rate loop trades it against the bit budget.
pub(crate) const TARGET_Q: f32 = 2048.0;

/// AAC-LC encoder state (push-shaped: one `raw_data_block` per 1024
/// samples, so a streaming wrapper is additive later).
pub struct LcEncoder {
    fs_index: u8,
    sample_rate: u32,
    channels: usize,
    bitrate_bps: u32,
    offsets: &'static [u16],
    short_offsets: &'static [u16],
    /// Full 2048-tap KBD window pre-scaled by 32768 (s16 domain).
    window: Box<[f32; 2 * LONG_WINDOW_LEN]>,
    /// Start/stop/short analysis windows (same scaling).
    windows: ShortWindows,
    prev: Box<[[f32; LONG_WINDOW_LEN]; 2]>,
    /// Frame scratch kept off the stack (TASK-118): windowed input and the
    /// pre-TNS spectra the psy model reads.
    win_buf: Box<[f32; 2 * LONG_WINDOW_LEN]>,
    psy_buf: Option<Box<[[f32; LONG_WINDOW_LEN]; 2]>>,
    chans: Box<[QuantChannel; 2]>,
    books: [[u8; MAX_BANDS]; 2],
    gains: [u8; 2],
    psy: Psy,
    target_q: [[f32; MAX_BANDS]; 2],
    /// Unspent bits carried forward (bounded at one frame's budget).
    credit: i64,
    /// Previous frame's noise offset. The next search tries it first.
    rate_guess: i32,
    /// Stuffing debt so 10 s payload/valid stays within ±3%.
    pad_debt: i64,
    /// This frame's per-band M/S decision (`ms_mask_present` 0/1/2).
    ms: MsBands,
    /// This frame's per-channel TNS decision (`enc_tns`; off on short).
    tns: [EncTns; 2],
    /// Previously emitted sequence (legal-transition state).
    prev_seq: WindowSequence,
    /// This frame's sequence (set by `encode_frame`).
    seq: WindowSequence,
    /// Per-channel attack detectors (OR'd for the shared CPE decision).
    detectors: [AttackDetector; 2],
    /// One-frame attack lookahead (`push_frame` / `flush`).
    lookahead: bool,
    short_tns: bool,
    short_group: bool,
    band_refine: bool,
    pns: bool,
    intensity: bool,
    /// Pre-M/S IS candidate mask (long stereo).
    is_band: [bool; MAX_BANDS],
    grouping: super::enc_group::Grouping,
    /// The frame held for the lookahead decision (a private copy — the
    /// caller's buffers are reused between pushes).
    held: Option<Box<lookahead::HeldFrame>>,
    /// Reused `raw_data_block` bytes (TASK-78).
    payload: Vec<u8>,
    /// HE: SBR FIL `extension_payload` bytes (empty = LC); set per frame.
    fill: Vec<u8>,
    /// ABR stuffing: EXT_FILL payload bytes as FILs before `END` (TASK-121); 0 outside `fit_budget`.
    pad_fill: usize,
    /// Long-frame allocation inputs, once per frame (TASK-113).
    alloc: Box<[super::enc_alloc::AllocCache; 2]>,
    /// Quality VBR (TASK-67): a fixed allowed-noise offset instead of the
    /// ABR rate loop; `None` = ABR on `bitrate_bps`.
    quality: Option<i32>,
    /// Opt-in CBR reservoir (`enc_cbr`). `None` keeps ABR.
    cbr: Option<cbr::Reservoir>,
    /// Short-path state (touched only on EightShort frames).
    chans_s: Box<[QuantShort; 2]>,
    books_s: [[u8; MAX_FLAT_SHORT]; 2],
    target_q_s: [[f32; MAX_FLAT_SHORT]; 2],
    psy_short: Psy,
    /// Off = OnlyLong every frame (LFE elements; pre-echo A/B in tests).
    block_switching: bool,
    /// Test hook: whole-pair M/S A/B (per-band when true).
    #[cfg(test)]
    ms_per_band: bool,
    /// Off = no TNS (LFE elements; A/B in tests).
    tns_enabled: bool,
}

impl LcEncoder {
    /// `channels` must be 1 or 2; `sample_rate` must be in the ADTS table.
    pub fn new(sample_rate: u32, channels: usize, bitrate_bps: u32) -> Result<Self> {
        let fs_index = ADTS_SAMPLE_RATES_HZ
            .iter()
            .position(|&r| r == sample_rate)
            .ok_or(Error::UnsupportedSampleRateIndex(0xFF))? as u8;
        if !(1..=2).contains(&channels) {
            return Err(Error::Format("LC encoder: channels must be 1 or 2"));
        }
        if bitrate_bps == 0 {
            return Err(Error::Format("LC encoder: bitrate must be > 0"));
        }
        if bitrate_bps > rate::max_bitrate_bps(sample_rate, channels) {
            return Err(Error::Format(
                "LC encoder: bitrate exceeds 6144 bits/channel",
            ));
        }
        let offsets = long_offsets(fs_index)?;
        let short_offsets = short_offsets(fs_index)?;
        // KBD both halves (alpha 4, matching the decoder table): far better
        // sidelobe rejection than sine, so masked-band decisions aren't
        // fighting analysis leakage. The ics_info shape bit matches.
        let half = window_left(2 * LONG_WINDOW_LEN, WindowShape::Kbd);
        let mut window = super::heap::heap_array::<f32, { 2 * LONG_WINDOW_LEN }>(0.0);
        for i in 0..LONG_WINDOW_LEN {
            window[i] = half[i] * 32768.0;
            window[2 * LONG_WINDOW_LEN - 1 - i] = half[i] * 32768.0;
        }
        let n_bands = offsets.len() - 1;
        Ok(Self {
            fs_index,
            sample_rate,
            channels,
            bitrate_bps,
            offsets,
            short_offsets,
            window,
            windows: ShortWindows::new(),
            prev: super::heap::heap_array([0.0; LONG_WINDOW_LEN]),
            win_buf: super::heap::heap_array(0.0),
            psy_buf: None,
            chans: super::heap::heap_array(QuantChannel::new(n_bands)),
            books: [[0; MAX_BANDS]; 2],
            gains: [100; 2],
            psy: Psy::new(offsets, sample_rate),
            target_q: [[0.0; MAX_BANDS]; 2],
            credit: 0,
            rate_guess: rate::OFFSET_LO,
            pad_debt: 0,
            ms: MsBands::off(),
            tns: [EncTns::off(), EncTns::off()],
            prev_seq: WindowSequence::OnlyLong,
            seq: WindowSequence::OnlyLong,
            detectors: [AttackDetector::new(), AttackDetector::new()],
            lookahead: false,
            short_tns: false,
            short_group: false,
            band_refine: false,
            pns: false,
            intensity: false,
            is_band: [false; MAX_BANDS],
            grouping: super::enc_group::Grouping::ungrouped(),
            held: None,
            payload: Vec::with_capacity(MAX_PAYLOAD_BYTES),
            fill: Vec::new(),
            pad_fill: 0,
            alloc: super::heap::heap_array(super::enc_alloc::AllocCache::new()),
            quality: None,
            cbr: None,
            chans_s: super::heap::heap_array(QuantShort::new(short_offsets.len() - 1)),
            books_s: [[0; MAX_FLAT_SHORT]; 2],
            target_q_s: [[0.0; MAX_FLAT_SHORT]; 2],
            psy_short: Psy::new_short(short_offsets, sample_rate),
            block_switching: true,
            #[cfg(test)]
            ms_per_band: true,
            tns_enabled: true,
        })
    }
    /// Wire `sampling_frequency_index` (for the ADTS header / ASC).
    #[must_use]
    pub fn fs_index(&self) -> u8 {
        self.fs_index
    }

    /// Opt into one-frame attack lookahead (see the module docs).
    #[must_use]
    pub fn with_lookahead(mut self, on: bool) -> Self {
        self.lookahead = on;
        self
    }

    /// Opt-in ATH / tonality on long and short psy (default off).
    #[must_use]
    pub(crate) fn with_psy(mut self, ath: bool, tonality: bool) -> Self {
        self.psy.enable_ath(ath);
        self.psy.enable_tonality(tonality);
        self.psy_short.enable_ath(ath);
        self.psy_short.enable_tonality(tonality);
        self
    }

    /// Is one-frame attack lookahead enabled?
    pub(crate) fn lookahead_enabled(&self) -> bool {
        self.lookahead
    }

    /// Test-only A/B switch for pre-echo measurements.
    #[cfg(test)]
    pub fn set_block_switching(&mut self, on: bool) {
        self.block_switching = on;
    }

    /// Test-only A/B switch: whole-pair M/S (off) vs per-band (on).
    #[cfg(test)]
    pub fn set_ms_per_band(&mut self, on: bool) {
        self.ms_per_band = on;
    }

    /// Test-only A/B switch: TNS on long frames.
    #[cfg(test)]
    pub fn set_tns(&mut self, on: bool) {
        self.tns_enabled = on;
    }

    /// Test hook: did the last frame emit TNS on any channel?
    #[cfg(test)]
    pub(crate) fn tns_on(&self) -> bool {
        self.tns[..self.channels].iter().any(EncTns::is_on)
    }

    /// Window sequence state machine (see the module docs). Causal: the
    /// decision uses only frames already pushed.
    fn next_seq(&self, attack: bool) -> WindowSequence {
        match (self.prev_seq, attack) {
            (WindowSequence::OnlyLong | WindowSequence::LongStop, true) => {
                WindowSequence::LongStart
            }
            (WindowSequence::OnlyLong | WindowSequence::LongStop, false) => {
                WindowSequence::OnlyLong
            }
            (WindowSequence::LongStart, _) => WindowSequence::EightShort,
            (WindowSequence::EightShort, true) => WindowSequence::EightShort,
            (WindowSequence::EightShort, false) => WindowSequence::LongStop,
        }
    }

    /// Frame-shape check shared by the causal and lookahead entry points.
    fn check_shape(&self, pcm: &[&[f32]]) -> Result<()> {
        if pcm.len() != self.channels || pcm.iter().any(|c| c.len() != LONG_WINDOW_LEN) {
            return Err(Error::Format("LC encoder: bad frame shape"));
        }
        Ok(())
    }

    /// Run the per-channel attack detectors on `pcm` (OR'd for the shared
    /// CPE window decision).
    fn detect(&mut self, pcm: &[&[f32]]) -> bool {
        let attack = (0..self.channels).any(|ch| self.detectors[ch].push(pcm[ch]));
        attack && self.block_switching
    }

    /// Window through emit. `attack` is this frame, or one frame ahead.
    fn encode_with_attack(&mut self, pcm: &[&[f32]], attack: bool) -> Result<()> {
        let specs = self.analyze(pcm, attack);
        self.rate_control(&specs)?;
        self.prev_seq = self.seq;
        Ok(())
    }

    /// Everything before rate control: the coded spectra of this frame.
    fn analyze(&mut self, pcm: &[&[f32]], attack: bool) -> [[f32; LONG_WINDOW_LEN]; 2] {
        self.seq = self.next_seq(attack);
        let mut specs = [[0.0f32; LONG_WINDOW_LEN]; 2];
        let buf = &mut *self.win_buf;
        for ((spec, prev), plane) in specs.iter_mut().zip(self.prev.iter()).zip(pcm.iter()) {
            buf[..LONG_WINDOW_LEN].copy_from_slice(prev);
            buf[LONG_WINDOW_LEN..].copy_from_slice(plane);
            match self.seq {
                WindowSequence::EightShort => enc_short::spectra(buf, &self.windows, spec),
                seq => {
                    let window: &[f32] = match seq {
                        WindowSequence::LongStart => &self.windows.start[..],
                        WindowSequence::LongStop => &self.windows.stop[..],
                        _ => &self.window[..],
                    };
                    for (b, &w) in buf.iter_mut().zip(window.iter()) {
                        *b *= w;
                    }
                    mdct_into_f32(buf, spec);
                }
            }
        }
        for (prev, plane) in self.prev.iter_mut().zip(pcm.iter()) {
            prev.copy_from_slice(plane);
        }
        // TNS runs on the raw L/R spectra before M/S (the decoder inverts
        // in that order). Psy reads the pre-TNS spectrum: whitening would
        // sink the −60 dB coded-band floor; the TNS span is the coded span.
        let long = !self.seq.is_eight_short();
        let mut psy_specs = self
            .psy_buf
            .take()
            .unwrap_or_else(|| super::heap::heap_array([0.0; LONG_WINDOW_LEN]));
        let mut coded = [[false; MAX_BANDS]; 2];
        if long {
            *psy_specs = specs;
            self.prepare_alloc(&specs);
            for (c, a) in coded.iter_mut().zip(self.alloc.iter()) {
                *c = a.coded;
            }
        }
        self.apply_tns(&mut specs, &coded, self.tns_enabled);
        self.grouping = if self.seq.is_eight_short() && self.short_group {
            super::enc_group::Grouping::decide(&specs, self.channels, self.short_offsets)
        } else {
            super::enc_group::Grouping::ungrouped()
        };
        self.decide_stereo(&mut specs, &mut psy_specs, long);
        if long {
            self.finish_alloc(&specs, &psy_specs);
        }
        self.psy_buf = Some(psy_specs);
        specs
    }
}

/// Rate loop + emission (`enc_frame_rate.rs`): split out for the line cap.
/// A child module, so the impl keeps using `LcEncoder`'s private fields.
#[path = "enc_frame_rate.rs"]
mod rate;
pub use control::QUALITY_MAX;
pub use rate::max_bitrate_bps;
pub(crate) use rate::search_monotonic;

#[path = "enc_refine.rs"]
mod refine;

/// One-frame attack lookahead (`enc_frame_lookahead.rs`), a child like `rate`.
#[path = "enc_frame_lookahead.rs"]
mod lookahead;

#[path = "enc_frame_reset.rs"]
mod reset;

#[path = "enc_frame_opts.rs"]
mod opts;

/// ABR rate loop vs quality VBR (`enc_frame_control.rs`), a child like `rate`.
#[path = "enc_frame_control.rs"]
mod control;

/// Opt-in CBR bit reservoir (`enc_cbr.rs`). Default ABR does not enter it.
#[path = "enc_cbr.rs"]
mod cbr;

#[path = "enc_frame_alloc.rs"]
mod alloc;

/// HE hooks (`enc_frame_he.rs`): SBR fill bytes + core cutoff (TASK-89).
#[path = "enc_frame_he.rs"]
mod he;

/// Split-phase hooks for the surround encoder (`enc_frame_mc.rs`, TASK-115).
#[path = "enc_frame_mc.rs"]
mod mc;
pub(crate) use mc::Specs;

#[cfg(test)]
#[path = "enc_frame_tests.rs"]
mod enc_frame_tests;

#[cfg(test)]
#[path = "enc_block_tests.rs"]
mod enc_block_tests;

#[cfg(test)]
#[path = "enc_lookahead_tests.rs"]
mod enc_lookahead_tests;
