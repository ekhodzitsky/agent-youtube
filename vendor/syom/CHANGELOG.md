# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - Unreleased

### Fixed

- Prevent stack overflow on the first cold decode on a 128 KiB thread
  stack. Spectral Huffman lookup tables now initialize directly on the
  heap; decoded PCM and steady-state allocation behavior are unchanged.
  Fresh-process optimized regressions cover LC, HE v1/v2, implicit PS,
  multichannel and LD. This fixes the cold small-stack limit recorded
  during the earlier encoder measurements; those results are preserved.

### Added

- Opt-in exact M4A interval planning with borrowed access-unit byte ranges,
  native presentation coordinates and explicit initial-state prefix replay.
  `seek_exact` rejects an insufficient replay-AU budget before I/O; missing
  prefix bytes remain errors. No independent bounded-preroll window is claimed.
- Opaque source-bound logical seek positions with budgeted replay on restore.
  They are not codec-state checkpoints. Foreign positions, unsupported exact
  profiles and failed exact decode state produce explicit errors. The new
  contract covers LC/LD mono/stereo; HE/PS and prediction histories are excluded.
  Existing `seek` and the 0.8.1 PNS history fix remain available.

### Changed

- Separate current support and qualification from archived corpus,
  conformance, footprint and frame-grid decisions. Add contributor setup,
  complete lab navigation, portable local-state ignores and an executable
  published-README check. Record fresh local library-line coverage with
  explicit exclusions, preserving the earlier measurements.
- Reorganize the root README around a verified published-crate quick start,
  compact comparisons and scoped historical measurements. Move detailed
  format, integration and benchmark evidence into linked documentation;
  distinguish registry badges from local candidate validation. Record a
  prioritized documentation and root-layout audit with remaining gaps.
- Further reduce LC Huffman cost/emission and section-planning work without
  changing encoded bytes. Store psychoacoustic spreading matrices at their
  active dimensions, saving 10404 logical bytes per 48 kHz LC encoder on
  x86_64. Retain balanced local measurements and rejected experiments;
  the strict FDK speed gate is still unmet.
- Reduce LC encoder work by caching frame-invariant analysis and unchanged
  quantized bands, preserving greedy section choices with integer prefix
  costs, and avoiding duplicate quantization and frame serialization.
  Write fields by bytes and append padding at the original bit position.
  Existing goldens and 60 encoder configurations retain their exact bytes.
- Correct the isolated FDK adapter to drain delayed output completely.
  Retain a balanced one-core comparison and raw observations: LC improves
  locally, but the strict FDK encode-speed gate remains unmet. Record the
  additional fixed encoder workspace and historical cold small-stack
  finding, fixed in this candidate.
- Qualify the existing opt-in LC CBR reservoir against independently decoded
  CORE_TUNE speech/tremolo scores and separate 10-second payload-rate/cap
  checks. Retain a reproducible isolated runner and full-precision results;
  production encoder bytes and historical measurements are unchanged.
- Record verified 0.8.1 registry publication and complete the downstream
  consumer dependency patch with its locked registry checksum.
- Add an opt-in upload-admission mode to the local HTTP load lab. Honor
  `upload_busy` backoff, check client latency including retries, and use a
  held job to test pool timeouts independently of upload rejection. Keep
  the historical queueing protocol as the default.
- Link the maintained opt-in gigastt integration and its new qualification,
  distinguishing the refactored adapter from the earlier consumer snapshot.

## [0.8.1] - 2026-10-28

### Fixed

- Preserve PNS random-generator history during M4A seek. Cold seeks and
  seeks past noise-coded material replay the required prefix; verified
  noise-free LC/LD prefixes retain short overlap preroll. Legacy profiles
  also replay prediction history. This restores exact agreement with
  linear decoding on a real 408-second recording and the PNS regression.

- Skip opaque AAC fill padding without copying it into temporary vectors.
  Padded LC ABR/CBR push decode again performs zero allocations after
  warmup in the packaged consumer checks, with or without ID3 metadata.
  Truncated fill payloads still fail; SBR and DRC validation is unchanged.
- Rebuild the packaged consumer's release dependency after replacing
  extracted sources, so a same-version package cannot reuse stale code.

### Changed

- Refresh the Rust competitor comparison against rusty_aac 2.0.1 and
  record the 0.8.0 production audit, including the failing padded-LC
  allocation gate. Extend the isolated fuzz lab to the 0.8.0 profiles
  and raw ASC/AU mutations.
- Qualify the local 0.8.1 candidate on real consumer recordings and a
  paired HTTP RNN-T canary against syom 0.7.1; preserve source hashes,
  response parity, latency/RSS gates and readiness/rollback results.
- Extend local HTTP qualification to concurrent 1/2/4/8-client load,
  repeated memory checks, queue/watchdog timeouts and recovery after
  disconnects and client read timeouts, against the same 0.7.1 baseline.

## [0.8.0] - 2026-10-19

### Added

- Opt-in LC CBR (`EncodeOptions::with_cbr`): a bit reservoir inside the
  6144 bits/channel frame cap. Default encode stays ABR, and ADTS
  `adts_buffer_fullness` stays `0x7FF` unless CBR is on. Not combined
  with HE, AAC-LD, quality VBR, or surround.
- Decode 960-sample AAC-LC and the 480-sample AAC-LD grid. HE-AAC on a
  960-sample core is `Unsupported`, not a 960-line LC frame. 1024-sample
  and 512-sample streams are unchanged.
- Decode AAC Main and LTP to planar f32. SSR long windows decode; eight-short
  SSR stays a typed error. LC, HE, and AAC-LD goldens are unchanged.

## [0.7.1] - 2026-10-02

### Changed

- Minimum supported Rust is **1.89** (was 1.88). The x86_64 paths use
  stable AVX-512 intrinsics from 1.89. CI runs the library and integration
  tests on that compiler.

## [0.7.0] - 2026-10-01

### Added

- Opt-in AAC-LD encode (`EncodeOptions::with_ld`): 512-sample frames,
  mono or stereo, LOAS or M4A, priming of one frame. `encode` stays
  AAC-LC. ADTS is rejected (it cannot signal AOT 23), as are HE,
  lookahead, quality VBR and the LC-only tools. Qualified against
  FDK v2.0.3 and libxaac 0.1.13 (syom's decode of the committed
  streams is within 2 LSB and 55 dB of both).

- Bounded fragmented-MP4 input (TASK-124, decision-25; contract and lavf
  parity in lab/fmp4/REPORT.md): an init `moov` (empty sample table,
  `mvex`/`trex` defaults) plus `moof`/`mdat` fragments decode on the slice
  (`decode` / `decode_with` / `decode_streaming`) and seekable
  (`decode_seek` / `M4aSeek`) paths, including concatenated DASH
  init + media segments (a zero `elst.segment_duration` there is an open
  end: priming trim, no duration cap). tfhd base resolution covers
  explicit `base_data_offset`, `default_base_is_moof` and the implicit
  previous-fragment base; trun v0/v1 with the trun → tfhd → trex defaults
  chain; resolved sample ranges are fenced inside the following `mdat`
  payload. `probe` reports the init segment (profile/rates/layout; the
  priming edit when present). Outside the envelope, errors are typed and
  matchable — encrypted (`enca` / traf-level `saiz`/`saio`/`senc`),
  multi-track or foreign-track fragments, nonzero composition offsets and
  sample-description switches are
  `AacError::Unsupported(UnsupportedFeature::FragmentedMp4(..))`; mfhd
  sequence gaps, unknown trun versions, zero resolved durations and ranges
  outside the `mdat` are `AacError::Malformed`; a truncated
  init/`moof`/`mdat` once the fMP4 init is visible, and dangling box
  headers, are `AacError::Truncated` (TASK-126).

- Incremental fragmented-MP4 delivery through the push `Decoder`
  (TASK-125): `feed` accepts arbitrary chunkings of an fMP4 init segment
  plus `moof`/`mdat` fragments (a fragmented init is recognized by its
  `mvex`; a flat moov keeps `UnsupportedFeature::M4aPush`) and delivers
  each AU as soon as it completes inside the `mdat`. Resident buffering
  stays at one box payload / one AU plus headers: the init `moov` and
  each `moof` are fenced by the metadata budget, and the per-fragment
  sample table is dropped with its fragment. `mfhd`/`tfdt` continuity,
  the edit-window trim (priming/remainder in `StreamInfo`) and the
  failed/finished/`reset` lifecycle mirror the one-shot fMP4 and
  ADTS/LATM push rules; a finish over a mid-box or mid-AU cut is a typed
  `AacError::Truncated`. One `feed` of a whole init widens to the
  metadata budget after the bytes sniff as fMP4 (it is not stuck on the
  ADTS partial-buffer cap). A box size that wraps the absolute end is
  `AacError::Malformed`, not a panic.

- fMP4 qualification corpus (TASK-126): every lab/fmp4/REPORT.md §2
  contract cell is pinned by committed goldens with sha256 provenance
  (`lab/fmp4/GOLDENS.md`, checked by `verify_pins.py`), including the
  third-party GPAC-muxed Big Buck Bunny DASH HE vector
  (`fmp4_bbb_{init,seg1}.m4a`, pins in `lab/fmp4/PIN.md`). Fragment
  timelines (per-sample dts/cts, tfdt-less accumulation, DASH `elst`
  priming versus the untrimmed mov-muxer presentation, continuous `mfhd`
  sequence numbers) are asserted against a committed ffprobe-derived
  oracle (`src/goldens/fmp4_timeline.txt`,
  `lab/fmp4/gen_timeline_oracle.py`);
  the tfdt-less fixture `fmp4_lc_notfdt.mp4` (`lab/fmp4/strip_tfdt.py`)
  proves accumulation decodes bit-identically, and truncation /
  mid-fragment cuts are typed errors with a fuzz seed
  (`corpus/fuzz/fmp4-truncated.bin`). Tests never spawn ffmpeg/ffprobe.

- AAC-LD (AOT 23) configuration and transport framing (TASK-128, first
  step of the LD epic, decision-28): `AudioSpecificConfig` and LATM/LOAS
  `StreamMuxConfig` carrying ER AAC LD now parse (GA syntax with the ER
  resilience flags and `epConfig`), and `probe` / `probe_with` report the
  new `ProbeProfile::Ld` with correct rate and channel metadata.
  ADTS cannot signal AOT 23 and keeps its typed error. ELD (39) and USAC (42)
  configs remain rejected.
- AAC-LD decode (TASK-129): 512-sample LOAS and raw access units reconstruct
  through the LD window pair (sine and low-overlap), the 1024-point IMDCT
  and LD TNS limits. FDK v2.0.3 PCM matches within 2 LSB / 55 dB on frames
  that do not use perceptual noise (`ld64m`, `ld64mus`, and the noise-free
  frames of `ld48`). The PNS generator stays the non-normative lavc LCG, so
  it is not bit-identical to FDK or libxaac fixed-point noise. A 480-sample
  `frameLengthFlag` stays `Unsupported`.
- AAC-LD in M4A (TASK-130): an `mp4a` sample entry with an AOT 23 config
  decodes on the slice and seekable paths. `elst` priming and tail are
  sample counts on the 512-sample grid (same rules as LC), and a seek
  replays one overlap frame rather than the two LC frames. `probe` reports
  `ProbeProfile::Ld` with that trim. Push LOAS follows the same
  failed/finished/`reset` lifecycle as ADTS.
- AAC-LD decode qualification (TASK-131): the committed oracles are FDK
  v2.0.3 and, for `ld64m`, libxaac 0.1.13 (`lab/profiles/LD_QUALIFY.md`).
  Decoder delay matches FDK `outputDelay` 0. libavcodec 7.0.2 still cannot
  decode these streams, and no ISO 14496-26 ER-LD conformance vectors are
  in the tree. That is the claim boundary, not a certificate.
- `syom::guide`: a task-oriented guide with compiling, self-contained
  examples (one-call use, borrowed PCM and `Read`/`Write`, timing, streaming,
  raw access units, probing and seeking, encoder modes, limits and errors,
  support matrix, behaviour changes); the README opens with a task table
  (TASK-107).
- HE-AAC v2 encode (TASK-91, TASK-92, TASK-93): `EncodeOptions::with_he_v2`
  codes stereo as a mono HE core plus parametric stereo (20-band level
  difference and coherence per access unit) for about 16–40 kbps; ADTS
  implicit, M4A / LATM / `Encoder::asc` explicit AOT 29, HE v1 timeline.
  libavcodec decodes the goldens `he2_48e*` as HE-AACv2 stereo with the
  written image (`lab/quality/HE_V2.md`). New public field
  `EncodeOptions::ps`.
- Surround AAC-LC encode (TASK-114, TASK-116): `encode*`, `Encoder`,
  `encode_write` and `encode_write_m4a` accept 3, 4, 5, 6 or 8 planes in the
  decode order (5.1 = FL FR FC LFE BL BR, 7.1 adds SL SR) and signal
  `channel_configuration` 3–7 in ADTS, M4A and LATM. One LC encoder per
  element; ABR searches one allowed-noise offset for the whole frame so
  bits follow demand across elements (TASK-115, `lab/quality/MC_ALLOC.md`),
  LFE long-only, band-limited and coded 12 dB finer; quality VBR works; HE, lookahead and PCE
  output are errors. libavcodec-decoded goldens `enc_mc51*` / `enc_mc71*`.
- AAC-LC 7.1 decode (TASK-62): `channel_configuration` 7 maps to the
  libavcodec order FL FR FC LFE BL BR SL SR with `Layout::Mpeg(7)` labels
  (`mpeg_channels(7)`), PCE streams may declare up to eight planes (a second
  front CPE is the side pair), speech mono keeps the non-LFE mean; lavc
  goldens `mc71` (config 7) and `mc71p` (ffmpeg `7.1(wide)` PCE).
  `channel_configuration` 8–15 now fails with
  `UnsupportedFeature::ChannelConfiguration` instead of a label-less
  element-order fallback.

### Changed

- One-shot ADTS encode (`encode`, `encode_with`, `write`, `encode_write`)
  prepends an ID3v2.4 `iTunSMPB` tag with the encoder delay, the unplayed
  tail and the source length. HE v1 and HE v2 priming is 3018
  output-rate samples. Decode trims to the source length when the decoded
  sample count equals priming + source + remainder, and reports those
  edges. A stale tag leaves the padded PCM in place. A push decode applies
  a tag whose priming and remainder are each at most 4096 samples as
  frames arrive, and ignores a larger edge so the resident buffer stays
  one access unit. `probe` reports the tag when the buffered ADTS frames
  fill that sum (1024 or 2048 samples per frame). Push `Encoder` callbacks
  stay raw access units. Untagged ADTS is unchanged. LATM output carries
  no delay tag.
  M4A and one-shot fragmented MP4 without an edit list honour the same
  tag inside `moov`. An edit list wins. A push fragmented MP4 decode does
  not read the tag.
- Parametric-stereo IID gains are a const table of `10^(dB/20)` filled by
  `exp10_f64`. The decode path no longer calls `powf` for those gains.
  The committed `ps48` planar f32 on this host is unchanged.
- The crate summary and the README no longer open with a slogan. The
  README is a short codec page: badges, one example, the support matrix,
  and the 2026-10-01 matched-shape timings.
- LC encode keeps the same bitstream. On x86_64 the long-window quantizer
  uses SSE2 multiply-then-add and trunc-toward-zero, which matches `floor`
  on the non-negative quantizer sum; other targets use the same scalar
  trunc. The TNS autocorrelation converts the span to f64 once, and the
  analysis filter is a fixed-order FIR with the same left-to-right f64
  products. Huffman length lookups use the index the book LAV already
  bounds, the section scan accumulates each live book in one pass, and
  the long-window mask leaves `Σ√|x|` to the post-TNS allocation. Encoder
  goldens (`enc48*`, `he48e*`, scalar FFT and SSE2-only FFT) are unchanged.
  On one 2.0 s 48 kHz stereo clip at a 24 kbps ADTS request, in-process
  `encode_with` (`taskset -c 0`, release, load average 0.96) moved from a
  seven-rep median of 6.877 ms to 5.479 ms. The output stayed 6774 bytes.
  The recorded FDK-AAC 2.0.3 driver-only median on this cell is 5.902 ms.
- Decoder IMDCT pre/twiddle/post tables, TNS reflection sines, QMF DCT
  plans, and parametric-stereo mix angles are built with the
  exactly-rounded helpers in `det_math`. `sine48`, `tns48`, `he48` and
  `ps48` decode to the same planar f32 on x86_64 and Darwin arm64
  (rustc 1.97.1). Sine and KBD windows were already host-identical and
  stay shared with analysis, so the committed encoder goldens are
  unchanged.
- LC IMDCT on x86_64 inlines the AVX radix-2 stages and runs the
  pre/post rotate on SSE2 with separate mul/add/sub (no FMA). PCM bits
  of the committed LC fixtures are unchanged. On one paired run of
  `sine48.adts` (48000 Hz, 1 channel, 13312 samples, `taskset -c 0`,
  load average about 16) syom's median was 94 µs (bootstrap 86–138)
  and symphonia 0.6.1 was 143 µs (143.3–143.7).
- Frames without pulses write each spectral coefficient during the
  Huffman read (signed magnitude times the scale-factor gain) and skip
  the integer quant buffer. Pulse frames still parse, apply the pulse,
  then rescale. Grouped short windows stay one coefficient stream.
  `invquant` uses the unsigned magnitude, so a debug build does not
  overflow on `i32::MIN.abs()`; no AAC coefficient reaches that value.
  Only-long frames window the IMDCT output with a multiply and then an
  add. Encoder bytes are unchanged.
- Low-rate HE v1 (at most 24 kbps per channel) sets the LC core cutoff
  per frame to the top of the highest analysis band below the SBR
  crossover that is within 20 dB of that frame's peak. A flat spectrum
  and silence stay at the crossover. On the lecture cells, LF SNR moved
  from 31.5 dB to 36.6 dB (stereo) and from 32.2 dB to 37.8 dB (mono);
  the LC, tremolo-LC and noise guards did not move
  (`lab/quality/CORE_TUNE.md`). Rates above 24 kbps per channel are
  unchanged.
- LC encode reuses work inside the rate loop. Scalefactor gains are a
  256-entry table of the same deterministic `exp2`. Long-window band
  peaks come from the per-frame allocation cache. An all-zero band skips
  the Huffman walk. The noise-offset search starts at the previous
  frame's offset and walks a short neighborhood before bisecting. Bit
  counts are not strictly monotone, so an arbitrary start can stop at a
  different crossing; the running guess matches a cold bisection on the
  locked signals, including the encoder goldens. A steady tone quantizes
  at most four times per frame after the first; busy noise stays under
  eight. Release median over 15 cells is +20.6% (busy ABR +43% to
  +57%); one click cell sits inside the noise of a 7-rep run. Those
  streams are byte-identical (`lab/quality/SPEED.md`).
- Stack use (TASK-118): decoding needs 39 KiB of stack (HE v2 was 391 KiB,
  LC 115 KiB) and encoding 83–91 KiB (was 163 KiB) in optimized builds, so
  the codec now runs on 128 KiB thread stacks such as musl's default.
  Frame-sized matrices moved into reusable heap state; output is unchanged.
- Minimum supported Rust is now **1.88** (was 1.97): the code needs `let`
  chains and nothing newer; the library tests run on 1.88 in CI (TASK-99).
- `NOTICE` (patent disclaimer) ships in the crate and now names encoding
  as well as decoding; provenance audit in `corpus/PROVENANCE.md` (TASK-104).
- The published crate ships library sources only — 141 files / 355 KB
  instead of 568 files / 5.1 MB with goldens, lab and tooling
  (`scripts/check-package.py` audits contents, the empty dependency table
  and a 512 KiB budget).
- LC encode speed (TASK-83): the rate loop caches `|x|^0.75` once per
  frame and costs all Huffman books in one pass per band — 21–42% faster
  on busy material, byte-identical output (`lab/quality/SPEED.md`).
- LC quantization and rate control (TASK-113): scalefactors now come
  from the masked threshold (noise-to-mask targets per band, computed
  once per frame) and the rate loop moves an allowed-noise offset —
  uniform refinement with spare bits, a per-coefficient water level
  under shortage (quiet noise-like bands go first; band limiting falls
  out of it) — instead of peak-normalized precision with one global
  shift. M/S bands use the smaller of the L/R thresholds; TNS scales the
  residual's allowed noise; tonality (opt-in) raises noise-like bands'
  allowed noise instead of lowering their precision cap. All-zero bands
  are ZERO_HCB; the quantizer rounds with the ISO 0.4054 offset. Tone +
  white noise at 64 kbps stereo: SNR 2.0 → 15.5 dB (tone region 18 dB);
  ≤ 48 kbps stereo no longer decodes to silence; noise-like level
  inflation +3 → +0.5 dB. LC encode CPU −3 % at 128 kbps. Encoder
  goldens re-minted (`enc48{,m,t,l}`, `he48e{,m}`) with ffmpeg 7.0.2 and
  registered in the corpus/oracle manifests; decoded output changes.
- LC encoder core tuning (TASK-133, closing the TASK-95 deficits on
  tonal-modulated stereo and low-rate speech): the attack detector now
  compares each high-passed sub-block against a leaky accumulator fed the
  previous sub-block's energy (the fdk-aac `block_switch` scheme) instead
  of a flat two-frame mean, so smooth periodic swells (8 Hz AM tremolo)
  stay on long windows while clicks still switch; TNS span bands are
  coded or dropped by the rate loop's water level like any other band
  (force-coding the span at a coarse target blew the frame budget exactly
  where TNS fired hardest — stream-start collapses, tremolo and click
  clusters); the psychoacoustic signal-to-mask ratio moves from 18 to
  27 dB. Tremolo 48 kbps stereo (dev synth, neutral ffmpeg decode):
  SNR 21.0 → 44.8 dB, LF SNR 25.9 → 54.7 dB vs fdk-lc 37.5 / 40.5 at
  matched rate; lecture 24 kbps: LC LF SNR 28.5 → 42.6 dB, HE v1
  15.1 → 31.5 dB vs fdk 31.5 / 34.4; no loss on noise/mix guards
  (`lab/quality/CORE_TUNE.md`). All encoder goldens re-minted
  (`enc48*`, `he48e*`, `he2_48e*`, `enc_mc*`, the `fmp4_he*` remuxes)
  with ffmpeg 7.0.2 and registered in the corpus/oracle manifests;
  decoded output changes; the lavc tolerance gates (≤ 2 LSB / ≥ 55 dB)
  and the full-length FDK decode gate pass on every re-minted stream.

### Fixed

- Push `Encoder` with lookahead matches one-shot bytes when a steady
  level steps into the silent drain. Finish flushed the held frame
  before that drain, so the last content frame missed the drain's
  attack and stayed on a long window where one-shot used a long-start
  window. The drain is now the lookahead successor, as in one-shot.
  Causal encode and the committed lookahead goldens (minted one-shot)
  are unchanged.

- HE encode covers the source tail. Priming is 3018 output samples, and
  one silent drain frame was not always enough: a 3127-sample input
  coded three access units (6144 decoded samples) against 6145 required,
  so ADTS dropped the last source sample and M4A refused the file
  (`valid samples exceed coded duration after priming`). Finish now
  emits at most one extra silent access unit when the coded length
  would not cover priming plus the source. HE v1 and HE v2 share that
  finish. Lengths that already fit, including the committed HE goldens,
  are unchanged.

- `encode_write` rejects unequal plane lengths with
  `InvalidPcm::PlaneLength` before any byte is written. A shorter later
  plane used to panic while slicing the chunk.

- An in-band PCE with more channel elements than the mapper's old
  16-slot table is a format error (`PCE layout exceeds 7.1` or
  `PCE extra channel element`) instead of a panic. Layouts up to 7.1
  are unchanged.

- A fragmented-MP4 sample of size 0 is fenced inside its `mdat` the
  same way a non-empty sample is. An offset outside that payload is
  `Malformed`. On the push path it used to move the cursor past the
  fragment and underflow the following skip. An empty sample whose
  offset lies in the payload is unchanged.

- ISOBMFF box ends are computed in 64-bit arithmetic before they become
  a buffer offset. A largesize that does not fit the address width
  (wasm32) is a format error; it no longer collapses to a short box
  whose child walker would not advance. Sample-table indexes use the
  same checked add. Valid files are unchanged.

- ABR stuffing interoperability (TASK-121): undersized frames were padded
  with zero bytes after `ID_END`, which fdk-aac 2.0.3 (the AOSP platform
  software AAC decoder codebase) rejects with `AAC_DEC_UNKNOWN`, killing
  the stream; FAAD2 also lost ADTS sync on stuffed multichannel streams.
  Leftover budget is now EXT_FILL `fill_element()` padding *before*
  `ID_END` (`engine/enc_pad`), chunked at the 269-byte count syntax. The
  ABR payload/valid ±3 % contract and per-frame budgets are unchanged;
  decoded PCM is unchanged. Encoder goldens re-minted (`enc48*`,
  `he48e*`, `he2_48e*`, `enc_mc*`) with ffmpeg 7.0.2 and registered in
  the corpus/oracle manifests; `lab/fdk/smoke.py` gates every syom
  golden on a full-length FDK decode.

### Added

- LATM/LOAS output (TASK-94; new output format → minor bump):
  `EncodeContainer::Latm` for `encode_with`, the push `Encoder` and
  `encode_write` (LC and HE v1), plus `wrap_loas_au(payload, asc)` for
  raw access units. One `AudioSyncStream` frame per access unit with
  `StreamMuxConfig` in every frame (any frame is a sync point / config
  refresh), `audioMuxVersion 0`, `frameLengthType 0`, no CRC; ≈ 11 bytes
  of framing per frame (ADTS: 7). Goldens `enc48lt.latm` / `he48elt.latm` decode in
  libavcodec within the LC tolerance; LATM, ADTS and raw carry identical
  access units.
- `encode_write_m4a(sink, planes, rate, opts)` (TASK-60): incremental M4A
  on any `Write + Seek` sink — `ftyp` + `mdat` header first, access units
  as produced, `mdat` size patched and `moov` appended at finish (exact
  `elst` priming / presentation duration, LC and HE). Retained memory is
  one frame plus 4 bytes per access unit (2^20 cap); a version-0 offset
  ceiling is checked before every write; write/seek failures are
  `AacError::Io` leaving an unplayable prefix (valid only after `Ok`).
  Bytes identical to one-shot M4A.
- `encode_write(sink, planes, rate, opts)` (TASK-59): encode borrowed
  planes incrementally into any `std::io::Write` (ADTS, or raw AUs);
  bytes identical to `encode_with`, one-frame workspace, no collected
  output. A sink error ends the encode with `AacError::Io` after the
  frames already written (exact prefix, nothing repeated); M4A stays
  `EncodeM4aStreaming`.
- One-shot `encode` / `encode_with` / `write` / `write_with` take any
  `&[P] where P: AsRef<[f32]>` (TASK-55): borrowed `&[&[f32]]`, arrays or
  boxed slices encode without a copy; `&[Vec<f32>]` calls are unchanged.
  Migration note: a plane built by an unannotated `.collect()` now needs
  its type (`Vec<f32>`), since the parameter no longer fixes it.
- Quality VBR (TASK-67): `EncodeOptions::with_quality(0..=10)` codes every
  LC frame at one fixed allowed-noise offset (level 5 = the psy target,
  6 dB per level; no target rate, no padding; the 6144 bits/channel cap is
  met by uniform coarsening). Bytes follow the content; monotonic in the
  level on aggregate; one-shot / push byte-exact; LC only. Curves in
  `lab/quality/CURVES.md`.
- Encoder presets (TASK-108, decision-20): `EncodeOptions::high_quality()`
  (LC ADTS 192 kbps, causal) and `EncodeOptions::low_rate()` (HE v1 ADTS
  48 kbps) chosen from development curves (`lab/quality/CURVES.md`,
  `syom_curves`); the default stays LC 128 kbps causal. Lookahead, ATH and
  tonality remain opt-in. Candidate frozen for qualification
  (`lab/quality/FREEZE.md`). Names are settings, not quality claims.
- Opt-in HE-AAC v1 encode (TASK-90; new output format → minor bump):
  `EncodeOptions::with_he(true)` for `encode_with` / `write_with` and the
  push `Encoder` (ADTS or raw AUs). Input 16 / 22.05 / 24 / 32 / 44.1 /
  48 kHz (else `UnsupportedFeature::EncodeHeRate`), mono/stereo,
  `bitrate_bps` = whole-stream budget (core + SBR). ADTS: LC header at
  the core rate, SBR in FIL (implicit); M4A and `Encoder::asc`: explicit
  two-rate AOT 5 config (`2b098800` for 24/48 kHz mono), output-rate
  timeline with `elst` priming 3018; `mux_raw_he_m4a` for external
  muxers; `EncodeInfo::priming/remainder/coded_samples` at the output
  rate. Goldens `he48e.adts` / `he48em.m4a` decode in libavcodec as
  HE-AAC 48 kHz stereo within the LC tolerance; `encode` default and LC
  goldens unchanged.
- HE v1 access units (TASK-89): crate-internal `HeEncoder` — output-rate
  PCM → halfband core + 64-band analysis → LC core at half rate,
  band-limited at the SBR crossover (which follows the core kbps per
  channel), with the SBR `EXT_SBR_DATA` FIL of the same AU before `END`
  and counted in the whole-stream `bitrate_bps` budget. Chunk/reset/
  lookahead byte-identical; exact finish accounting (priming 3018 output
  samples at 48 kHz, content tail drained); libavcodec decodes the
  committed `he48e.adts` golden as HE-AAC 48 kHz stereo within the LC
  golden tolerance; encode CPU 0.69× LC at the same rate. `encode` and
  the push `Encoder` still emit LC only; the opt-in surface is TASK-90.
- HE v1 SBR bitstream writer (TASK-88): crate-internal
  `enc_sbr_bits` — `sbr_header`, all four `sbr_grid` classes, dtdf,
  invf, Huffman envelope/noise rows (inverse ISO tables), SCE and
  uncoupled CPE `sbr_data`, `sbr_extension_data` with header transmit
  or reuse, the `EXT_SBR_DATA` `extension_payload` bytes with zero fill,
  and the `fill_element` count/escape wrapper. Hand-authored bit
  vectors, exact bit accounting (writer bits = estimator `est_bits`),
  unrepresentable parameters are errors; no CRC variant. Not wired
  into `encode` (still LC). Evidence in `lab/quality/SBR_BITS.md`.
- HE v1 SBR parameter estimation (TASK-87): crate-internal
  `SbrEstimator` — fixed 3.0 dB header per rate (pinned band tables),
  FIXFIX 1/2/4-envelope grid from an HF energy surge, envelope and
  noise-floor scalefactors on the decoder's dequantisation scale,
  inverse-filter modes from a tonality gap, cheapest valid delta
  direction with a Huffman bit estimate (~61 bits/frame mono). Decoding
  the parameters with the in-tree SBR decoder over the ideal core
  matches interior HF envelope bands within 0.3 dB mean; not wired
  into `encode` (still LC). Evidence in `lab/quality/SBR_EST.md`.
- HE v1 core-rate preparation (TASK-86): crate-internal `SbrPrep`
  (17-tap halfband 2:1 downsample, delay 4 core samples) and a `det_math`
  64-band analysis QMF on full-rate PCM (the decoder's `X` grid), incremental/resettable with
  exact counts, chunk-invariant. Not wired into `encode` (still LC);
  evidence in `lab/quality/SBR_PREP.md`.
- HE-AAC v1 encode architecture (TASK-85): **go** for a staged original
  2× dual-rate SBR path (mono/stereo, ADTS/M4A). **Not** a 0.x default
  (`encode` stays LC). HE v2/PS encode stays gated. Tables: reuse
  in-tree ISO QMF/Huffman/bands; do not copy FDK/oxideav encoders.
  Targets and task split in `lab/quality/HE_ENC.md`.
- Bounded metadata probe (TASK-54): `probe` / `probe_with` report
  container, LC/HE/PS profile, core/output rate, layout, and whether
  duration/trim are exact, estimated, or unknown. Partial prefixes
  return `NeedMore`; VBR ADTS/LATM duration stays unknown; M4A `elst`
  is exact. No PCM decode and no allocation from `stsz` counts.
- Raw LC access units for external muxers (TASK-53):
  `EncodeOptions::raw()`, `EncodedFrame::payload`, `Encoder::asc`,
  `wrap_adts_au`, `mux_raw_lc_m4a`. Rewrap matches one-shot ADTS/M4A
  (causal and lookahead). One-shot raw is rejected.
- Recorded conformance/hostile-input campaign (TASK-103): 24 CPU-h
  parser+stateful fuzz (400 M iters, 0 panics); advertised-row matrix;
  **93.6%** library line coverage (tests excluded). ISO 14496-26 still
  not obtained (`lab/baseline/CAMPAIGN.md`, `COVERAGE.md`).
- Native CI matrix and encoder byte-determinism (TASK-98): Linux/macOS/
  Windows × x86_64/aarch64, toolchain 1.97.1. Encoder goldens (causal,
  lookahead, M4A, transient) stay byte-exact vs scalar and vs SSE2-only
  FFT. i686/wasm/mobile unqualified (`lab/baseline/PLATFORM.md`).
- x86 SIMD FFT butterflies for IMDCT/MDCT (TASK-82): SSE2 4-wide and
  AVX 8-wide, runtime-probed, separate mul/add (no FMA). Bit-identical
  to scalar; encoder goldens unchanged. Isolated 512-point FFT **−24%**
  vs scalar; isolated LC decode **−7%**. Declared x86_64 minimum is
  SSE2 (`lab/baseline/SIMD.md`).
- DCT-IV factorization of SBR QMF analysis/synthesis modulation
  (TASK-81): ISO GEMV kept as the test reference. Isolated QMF kernel
  **−43%** vs GEMV; whole HE **−30%** median (940 µs vs 1.35 ms,
  isolated process, thin LTO). LC isolated median unchanged. Goldens
  unchanged (`lab/baseline/QMF.md`).
- Reused PS hybrid/decorr/output workspace (TASK-80): fit hybrid rows,
  stack H-slot and QMF planes, mix-to-QMF without hybrid L/R matrices.
  PS borrowed decode after 2-frame warmup **24.75 allocs/frame** (−90%
  vs 259) and **−24% peak live heap** (430 KiB vs 568 KiB). Remaining
  ~25/frame are `ps_data` parse/resolve. Goldens unchanged
  (`lab/baseline/PS.md`).
- Reused HE SBR conversion/reconstruction workspace (TASK-79): in-place
  f32 planes, recycled SBR parse box, stack XLow/XHigh/env-adjust maps,
  interned QMF modulation tables. HE borrowed decode after 2-frame
  warmup **14.4 allocs/frame** (−91% vs 165) and **−30% peak live heap**
  (313 KiB vs 451 KiB). Goldens unchanged (`lab/baseline/SBR.md`).
- Zero-alloc LC speech/stereo borrowed callbacks after warmup (TASK-78):
  stack plane views, recycled spectral buffers, stack TNS/MS/Pulse,
  in-place PNS, reused encoder payload. Default `speech()` path **0**
  heap/frame; stereo split **~0**; 5.1 **1.5**/frame (was 57); encode
  **7**/frame (was 13). Owned one-shot output still allocates.
  Goldens unchanged (`lab/baseline/ALLOC.md`).
- Prepared codec workspace reuse (TASK-77): `Decoder::reset` /
  `Encoder::reset` drop signal/config (overlap, HE/PS/PCE, LCG, rate
  credit, lookahead) and keep allocated KBD windows, psy spreading,
  spectral/PCM planes, and buffer capacity. No global cache. Encoder
  reset is 0 setup allocs (−98% vs rebuilding `LcEncoder`). LC
  second-session allocs 79→66 (per-frame remainder is TASK-78); HE SBR
  still rebuilt (TASK-79). Goldens unchanged (`lab/baseline/RESET.md`).
- Frame/stream channel and timing metadata (TASK-61): `Channel`, `Layout`,
  `FrameMeta` (`Copy`, 8-plane labels, no per-frame heap). `Frame::meta`,
  `DecodedAac`/`StreamInfo` `{core_rate, layout, priming, remainder}`.
  ADTS priming/remainder stay `None`; M4A with `elst` reports skip/tail.
  MPEG 5.1 = FL FR FC LFE BL BR. Speech downmix is `Layout::SpeechMono`.
  `EncodeInfo::layout` is `Mpeg(1|2)`. `speech()` default unchanged.
- Opt-in intensity stereo (TASK-76): `EncodeOptions::with_intensity`.
  Default off — goldens unchanged. Long-window HF (`≥ 6 kHz`, `|ρ| ≥ 0.85`)
  right-channel `INTENSITY_HCB`/`HCB2` + `is_pos`. Ablation
  (`lab/quality/IS.md`): 64k panned 8 kHz ILD 0.300→0.297, L SNR **+0.50 dB**,
  ADTS −0.6%; anti-phase corr −1.0; ambience not collapsed. **No-go as
  0.x default** (stereo goldens; only HF panned at low rate).
- Opt-in LC perceptual noise substitution (TASK-75):
  `EncodeOptions::with_pns`. Default off — goldens unchanged. Long-window
  noise-like HF bands (`NOISE_HCB` + `noise_nrg`, det_math, TNS-span
  skipped). Ablation (`lab/quality/PNS.md`): 1 s white noise emits PNS
  (RMS match, priming-SNR −13 dB — not a PNS metric), ADTS +0.1% (ABR
  pad), sine/harmonic 0.00 dB (**no-go** as 0.x default: no ≥5% rate
  saving and no ≥0.3 dB quality gain).
- Opt-in bandwise leftover-bit scalefactor refine (TASK-74):
  `EncodeOptions::with_band_refine`. Default off — one global sf offset
  unchanged. Long frames only; ≤16 `sf[b]-=1` on underfunded bands
  (`qmax < TARGET_Q`) under the frame budget. Ablation
  (`lab/quality/REFINE.md`): noise +0.07 dB priming-SNR, sine 0.00 dB
  (**no-go** as 0.x default, < 0.3 dB gate).
- Exact AAC section-partition DP (TASK-73): `plan_books_dp` matches an
  independent exhaustive oracle on n≤8 and never exceeds greedy cost.
  Production `plan_books` stays greedy — 64 LCG channels saved **0.36%**
  section+spectral bits (< 2% gate). Goldens unchanged
  (`lab/quality/SECTION.md`).
- Opt-in short-window grouping (TASK-71): `EncodeOptions::with_short_group`.
  Default off — 8 groups of 1, `enc48t` goldens unchanged. Consecutive
  windows merge when energy is within 6 dB. Independent ICS parse-back.
  Ablation (`lab/quality/GROUPING.md`): 18.5% lower grouping/section/sf
  bits on the TASK-72 click, 0.0 dB click-region error, **no-go** as 0.x
  default (total payload −0.9%; quality unchanged).
- Preregistered AAC listening protocol (TASK-14): isolated
  `lab/listen` plus `scripts/listen_protocol.py`. MUSHRA vs BS.1116
  selection, hidden labels, 3-point noninferiority, n=69 from
  planning SD 10. Dry-run uses SYNTHETIC scores and is **not**
  listening evidence. TASK-109 stays blocked (no participants,
  holdout bytes not obtained, no Apple host).
- Opt-in short-window TNS (TASK-72): `EncodeOptions::with_short_tns`.
  Default off — long TNS and `enc48t` goldens unchanged. Per-window
  LPC order ≤ 7, 2 dB gate, syntax parsed by `tns::TnsData`. Ablation
  (`lab/quality/SHORT_TNS.md`): +2.3 dB click-region error, **no-go**
  as 0.x default (< 3 dB). Grouping still 8×1 (TASK-71).
- Causal pre-echo position sweep (TASK-70): `encode_preecho_tests` names
  in-frame pos < 448 as the failing subset. Lookahead vs causal is
  19.5–37 dB on shipped `encode_with`. LongStart `target_q` boost was
  **+0.6 dB** (no-go); `ATTACK_RATIO` 4 flags silence. No new default,
  no extra causal latency (`lab/quality/PREECHO.md`).
- Opt-in Johnston SFM tonality (TASK-69): `EncodeOptions::with_tonality`.
  Default off — production goldens unchanged. Noise-like bands get
  0.25× `target_q`; the coded mask is unchanged. Ablation
  (`lab/quality/TONALITY.md`): no-go as 0.x default (aggregate
  priming-SNR including sine/tremolo < 0.3 dB; mix +0.5 dB).
- Opt-in Terhardt ATH floor (TASK-68): `EncodeOptions::with_ath`.
  Default off — production goldens unchanged. Band floor becomes
  `max(mask, −60 dB relative, ATH at 0 dBFS = 96 dB SPL)` via `det_math`.
  Ablation (`lab/quality/ATH.md`): no-go as 0.x default (noise 128k and
  lecture priming-SNR drop >0.5 dB). Silence stays uncoded.
- Sample-accurate M4A seek (TASK-58): `M4aSeek` maps presentation samples
  through `elst`, resets LC/SBR/PS state, and prerolls before emit. LC
  mono/stereo uses 2 AUs (measured vs linear ≤ 2 LSB s16). HE/PS and
  3.0–5.1 replay from AU 0 (in-band SBR header; 5.1 coupling did not
  settle in 2 AUs on `mc51`). Not a universal delay constant.
- Seekable M4A decode (TASK-57): `decode_seek` / `decode_seek_with` /
  `decode_seek_streaming` load only `moov` then `seek`+read each sample.
  `mdat` before or after `moov` works; `elst` trim matches slice decode.
  `read` / `read_with` take this path for M4A (no whole-file load). `co64`
  indexes parse. Presentation-time seeking is TASK-58.
- Generic `Read` ADTS/LOAS decode (TASK-56): `Decoder::feed_read`,
  `decode_read` / `decode_read_with` (collecting) and
  `decode_read_streaming` (callback). Resident compressed buffer stays
  the streaming cap; `Interrupted` is retried; other I/O is `AacError::Io`.
  Truncated tails match push `finish`. M4A on a non-seekable reader is
  `Unsupported(M4aPush)` (TASK-57).
- Raw AAC access-unit decode (TASK-52): `Decoder::from_asc(asc, opts)`
  plus `decode_au` of complete `raw_data_block()` payloads (no ADTS/LATM
  wrap). LC/HE/PS PCM matches the equivalent framed goldens. Mid-stream
  ASC change is `Unsupported(AscChange)`; mixing `feed` and `decode_au`
  is `Unsupported(RawAccessUnit)`. Empty AU is `Truncated`. `AudioSpecificConfig`
  stays crate-private.
- Matchable `AacError` classes (TASK-51): `Unsupported`, `Truncated`
  (`at` byte offset when known), `Malformed`, `Lifecycle`, `InvalidPcm`.
  Engine failures map by variant, not by string. `Format` / `Decode` /
  `Encode` remain leftovers. `#[non_exhaustive]` match `_` still required.
  M4A push decode and streaming M4A encode are `Unsupported`, not
  `Format`/`Encode`. Truncated ADTS with no complete frame is `Truncated`,
  not `NotAac`.
- Stateful Decoder/Encoder lifecycle fuzz (TASK-49): ordinary tests
  replay `corpus/fuzz/lifecycle.txt` and assert chunked vs one-shot
  PCM/bytes, sticky fail/reset, corrupt-frame finite PCM, and encoder
  NaN/`|x|>1` as `InvalidPcm` (not roundtrip). Isolated
  `lab/fuzz` `syom_fuzz_state` is never spawned by `cargo test`.
- Bounded parser-fuzz smoke corpus (TASK-48): ordinary tests replay
  `corpus/fuzz/` plus goldens through `decode_with` / sniff and the
  BitReader, ASC, ADTS, LATM and PCE parsers. Mutational campaign is
  isolated `lab/fuzz` (std only, not a workspace member, never spawned
  by `cargo test`). Not a safety proof.
- LC ABR (TASK-66): leftover per-frame budget is unused bytes after
  `ID_END` (decoder stops at END; PCM-neutral). 10 s sine/noise/tremolo/
  lecture hit payload/valid ±3% (measured 1.002–1.003). Silence is not
  stuffed. ADTS stays `0x7FF` (no CBR reservoir). Leftover-band fill at
  TARGET_Q was measured and rejected (sine residual SNR ~0 dB). Encoder
  goldens reminted (`enc48{,m,t,l}`); lavc s16 PCM unchanged. Report:
  `corpus/rate/REPORT.md`.
- Encoder rate contract (TASK-65): `bitrate_bps` is a per-frame
  ceiling (capped VBR, ADTS `0x7FF`, one-frame `credit`, not a bit
  reservoir). TASK-66 spends leftover budget so long non-silent
  tracks meet ±3% payload/valid. CBR reservoir and TVBR stay no-go
  for 0.x.
- `DecodeOptions::audio()`: split channels with the same 2 h / 192 kHz
  lecture caps as `speech()` (TASK-50). `decode` / `read` /
  `DecodeOptions::default()` stay speech-mono. `unbounded()` remains the
  no-ceiling helper. Public config/output/error types are
  `#[non_exhaustive]` so TASK-51/61/65 can extend them.

### Changed

- Encoder PCM domain (`NaN`/`Inf`/`|x|>1`, empty, plane mismatch) is
  `InvalidPcm`, not leftover `Encode` (TASK-51).
- Streaming `Decoder` and `Encoder` lifecycle (TASK-45 / F18): open →
  `feed`/`finish`; parser, limit, PCM, or callback errors are **failed**;
  successful `finish` is **finished**. Further `feed`/`finish` error until
  `reset()`. `finish` takes `&mut self` (no longer consumes the instance).
  Counters include frames already handed to a callback that then failed.

### Fixed

- Zero-count FIL fill elements decode as the empty extension they are
  (TASK-127): `fill_element()`'s `while (cnt > 0)` loop never enters
  `extension_payload()` when the count resolves to 0, so the element is
  a 4-bit no-op — syom rejected it as `Malformed(Sbr)`, killing every
  later access unit. Seen in the wild in the GPAC-muxed Akamai BBB DASH
  HE-AAC vector; all 94 AUs now decode and match lavf at max 1 LSB /
  rms 0.70 (`lab/fmp4/REPORT.md` §4.1, goldens `bbb_fil.*`).
- LC stereo speech mono is the documented mean of the two planes again
  (TASK-122): the fast-mono path kept the left CPE channel untouched on
  ADTS one-shot, push streaming and M4A (HE stereo and LC multichannel
  already mixed). The CPE right channel is now folded into the mono mix
  in place — no extra allocation.
- M4A write rejects overflowing v0 box sizes, `stco` offsets, sample
  sizes and `stsd` 16.16 rates instead of wrapping `u32` (TASK-44).
  Ceiling is `u32::MAX` bytes (`co64`/largesize later). 88.2/96 kHz M4A
  is an error until a wider sample-entry field exists; 64 kHz and below
  stay valid.
- M4A decode honours `elst` presentation end (TASK-43): skip `media_time`,
  then emit at most `segment_duration` at the output rate. Empty edits,
  multiple edits, `media_rate ≠ 1`, and inexact timescale conversion are
  `Format` errors (never a silent prefix skip). One-shot and streaming
  slice agree. ffmpeg PCM dumps that skip priming only are compared on
  the presentation prefix; they are not reminted.
- M4A encode writes Apple QA1636 timeline into `elst`/`mvhd`/`mdhd`
  (TASK-42): presentation duration is valid source samples N (movie
  timescale = sample rate, so N=1 is not truncated to 0 ms); priming is
  `elst.media_time`; remainder is the unplayed `mdhd` tail
  (`coded − priming − valid`).

### Added

- Isolated encoder quality lab (`lab/quality`, TASK-16): same-clip LC
  64/128 kbps synth matrix (syom vs FFmpeg 9.0.1 vs ffmpeg 7.0.2 CLI),
  independent lavc PCM, alignment SNR. Held-out split unused. Apple and
  HE encode unresolved. Ordinary tests never run the lab.
- LC encode overlap drain (TASK-41): finish emits one extra zero MDCT so
  the last source samples reconstruct. `EncodeInfo` reports `priming`
  (1024), `remainder` (pad in the last content block), and
  `coded_samples`. One-shot and push stay byte-identical. Encoder goldens
  reminted (`enc48{,m,t,l}`) with lavc s16; ADTS decoded length is
  `(ceil(N/1024)+1)*1024`.
- Encoder priming / tail contract (TASK-40): decoded ADTS length is
  `ceil(N/1024)*1024` and is **not** valid duration. A last-sample impulse
  on 1024-aligned input is omitted (overlap not drained), confirmed by
  syom, oxideav-aac and lavc. `EncodeInfo.samples` is source length.
  M4A still signals `elst.media_time = 1024`; a one-frame file is empty
  after that skip. No encoder repair (TASK-41).
- Matched-output LC/HE/PS performance baseline (TASK-15): `cargo bench
  --bench baseline` runs the same `run_preflight` as Criterion, then 20
  timed reps (median / p95 / bootstrap CI). Historical BENCH.md decode
  rows stay labeled; ranking uses this table. `perf` stacks unavailable
  (`perf_event_paranoid=4`). Ordinary tests never time the harness.
- Isolated scoring lab (`lab/score`): independent-decode alignment, valid
  duration, actual ES bitrate, SNR/max-abs. Silence, delay, truncation,
  channel-swap and channel mismatch are diagnostics, not high quality
  scores. PEAQ BS.1387 and ViSQOL are unavailable on this host and are
  not claimed. Ordinary tests never link the scorer.
- Isolated glint-audio 0.11.0 lab (`lab/glint`): crates.io pins
  `df09912e…` / `b8c79457…`, MIT, C++17 native core vendored in
  `glint-audio-sys`. Wrapper vs C ABI copy overhead is a separate lane.
  AAC-LC only; no Cargo feature drops MP3/Opus. `GLINT_MODE=fixed` is
  off. Quality `speed|normal|best`. Ordinary tests never link glint.
- Isolated FAAC 1.31.1 encode lab (`lab/faac`): knik0/faac tag `faac-1.31.1`,
  SHA-256 `3191bf1b…`, LGPL kept off the product crate. In-process ADTS
  AAC-LC (`LOW`, MPEG-4, float PCM, TNS off). Requested `bitRate` is per
  channel and maps to `quantqual`; a 2 s 440 Hz sine at 64/128/192 kbps
  per channel all land ~43–44 kbps actual. Independent syom decode is
  finite 97280 samples/ch vs 96000 input. HE encode is unavailable on
  this release. Ordinary tests never link FAAC.
- Isolated fdk-aac-rust 0.2.3 lab (`lab/fdk-aac-rust`): crates.io pin
  `607e6ba5…`, Fraunhofer license kept off the product crate. Default
  `ffi` feature is native FDK C++ (blocked here). Measured path is
  `--no-default-features` (Rust port). LC sine48 decode matches 13312
  samples at 48 kHz; HE/PS ADTS via `AacLcDecoder` is core-rate only.
  Not a second independent FDK oracle (same 2.0.3 lineage as TASK-7).
  Ordinary tests never link it.
- Independent memory budgets (TASK-23 / F07): named constants and checked
  accounting (`MemoryBudgets`) for finite compressed input (1 GiB),
  collected planar f32 (4 GiB), channels (8), M4A index/metadata
  (2^20 entries / 16 MiB), and resident workspace (8 MiB). Duration is
  not a memory cap. Streaming `feed` must not inherit a lifetime
  compressed-byte cap (enforcement: TASK-24 collection, TASK-25
  streaming). `speech()` Mono / 7200 s / 48 kHz is unchanged.
- One-shot decode and M4A demux enforce those collection budgets before
  `reserve`/`resize` (TASK-24). Over-budget output, channel count, table
  entries, and box depth are `AacError::Limit` (`planned == max` allowed,
  `max + 1` not). `DecodeOptions::memory` holds the numbers; `speech()`
  duration and `ChannelMode` are unchanged. Streaming workspace remains
  TASK-25.
- Streaming `Decoder::feed` no longer inherits the 1 GiB lifetime
  compressed-byte cap (F07 / TASK-25). Resident buffer, declared AU
  length, and codec workspace are fenced independently of duration;
  one-shot `decode` still uses the finite 1 GiB input budget. Chunked
  vs whole-slice feeds stay sample-identical.
- Truncated or malformed SBR `fill_element` payloads are decode errors
  even before the first successful HE frame (F14 / TASK-39). A first
  SBR payload without `bs_header_flag` is `SbrFreqBandInvalid`, not LC
  success. HE declared without FIL still 2×-upsamples; a missing FIL
  after HE stays 2×. Truncated `ps_data()` is `PsDataInvalid`, not a
  silent hold. he48/ps48 goldens and chunked feeds are unchanged.
- Offline AAC evaluation corpus (`corpus/manifest.json`) with in-tree golden
  hashes, deterministic boundary PCM, named licensed natural excerpts (not
  vendored), a coverage table, and `scripts/verify_corpus.py` that fails on
  bit-flips, missing required assets, train/holdout recording overlap, and
  inconsistent metadata.

### Changed

- Decoder Criterion benches (`benches/aac.rs`) run an untimed equivalent-PCM
  preflight (`syom::decode_cmp::run_preflight`) before timing: native rate,
  channel count, finite samples and consumed length must match. A failed
  candidate aborts the group. Unavailable profiles and mismatched lengths
  (HE vs core-only) are non-comparable and produce no throughput number.
  The primary lane is planar split at native rate; speech downmix and
  discard-output are separately named. Historical BENCH.md decode rows
  stay labeled until a matched-output baseline replaces them. Product
  `decode` / `speech()` defaults are unchanged.
- Invalid `DecodeOptions` duration/rate limits (NaN, −∞, negative duration,
  zero `max_sample_rate`, zero `max_decode_sample_rate` with a finite
  duration) now error as `AacError::InvalidLimits` before decode work.
  `+∞` remains the explicit unbounded contract. NaN/−∞ no longer open an
  unlimited frame budget.
- `read` / `read_with` check the compressed file size (and cap the subsequent
  read) against `DEFAULT_MAX_INPUT_BYTES` before allocating the whole file.
- README install version is 0.6; oxideav-aac is documented as an ADTS
  LC+SBR/PS decoder (not a parser); encoder M/S is per-band; BENCH.md is
  linked as historical unequal-work evidence.
- Oracle provenance ledger (`corpus/oracles/provenance.json`) for committed
  goldens: SHA-256, comparison policy (deterministic vs PNS), mint command
  when known, and explicit gaps where ffmpeg versions were never recorded.
  No silent remint; ordinary tests still do not spawn ffmpeg.
- Encoder Criterion groups run an untimed preflight (`encode_cmp`) that
  records ADTS/payload bytes, decoded duration and achieved bitrate, aborts
  on failed encode/decode, and labels rate mismatch >1% as non-matched
  (not an equal-rate cell). HE encode remains an unavailable separate cell.
- Isolated native libavcodec lab (`lab/libavcodec`, FFmpeg 9.0.1 pin):
  in-process ADTS access-unit decode, container decode, and LC ADTS encode.
  Not a workspace member; ordinary tests never link or spawn ffmpeg.
- Isolated native FDK lab (`lab/fdk`, fdk-aac v2.0.3 pin, Fraunhofer license
  kept off the product crate): LC ADTS decode/encode adapter, afterburner
  off, delay reported. In-process smoke (zig-c++ GNU, 170 TUs): sine48
  13312 samples, HE/PS goldens length-match syom at 48 kHz, LC encode
  128 kbps is 32426 B / 97280 samples (independent lavc). syom rejects
  that FDK ADTS FIL (`extension_payload invalid`). Ordinary tests never
  link FDK.
- Isolated-process memory runner (`benches/mem_iso.rs`): one OS process per
  peer/case, reporting baseline RSS, peak RSS delta, peak live heap, alloc
  count/bytes and retained live separately for one-shot vs streaming. The
  in-process `mem` bench is unchanged historical.
- LATM/LOAS `AudioMuxElement` delivers every subframe (`numSubFrames+1`
  access units) in payload order. A missing extra subframe is an error,
  not a silent one-AU success. Single-subframe goldens are unchanged.
- LATM `latmGetValue` is the 2-bit length prefix used by FFmpeg/FDK/FAAD2
  (`bytesForValue` then `(n+1)*8` bits). `audioMuxVersion=1` other-data
  length uses that form; an ASC that overruns `ascLen` is an error.
- ADTS `protection_absent=0` frames verify `crc_check` with the ISO/IEC
  11172-3 CRC-16 (poly `0x8005`, init `0xFFFF`, no inversion) over the
  13818-7 header and raw-data-block protected bits. Mismatch is
  `AdtsCrcMismatch`, distinct from truncation. CRC-free streams are
  unchanged. The MPEG-4 Audio §1.8.4.5 generator is not used here.
- ADTS frames with `number_of_raw_data_blocks_in_frame` > 1 decode every
  `raw_data_block()` (1..=4). CRC-present payloads skip the header
  position table (`7+2N` bytes) and per-block 16-bit fields; CRC values
  are not checked (TASK-29). Single-block goldens are unchanged.
- Footprint/API cost (TASK-18): empty product deps, Rust 1.97, a
  measured release rlib/consumer binary, and a go/no-go that keeps
  `speech()` mono until a versioned API decision. Artifact:
  `corpus/footprint/`.
- Isolated FAAD2 2.11.3 decode lab (`lab/faad2`, GPL-2.0-or-later kept
  off the product crate): ADTS in-process `FAAD_FMT_FLOAT`, metadata and
  a classified disagreement report vs FFmpeg 9.0.1. Ordinary tests never
  link or spawn FAAD2.
- Encode PCM must be finite and in `[-1, 1]`. NaN, infinities, and
  `|x| > 1` are `Encode` errors on both one-shot and push APIs. There is
  no silent clip. ±0, subnormals, and full-scale ±1 remain valid.
- LC encode enforces 6144 bits per channel per 1024-sample frame,
  independent of the ADTS 8184-byte length ceiling. A requested bitrate
  that cannot fit that cap is `Encode` (not an over-limit frame). The
  rate loop still drops top bands to stay inside the cap; encode_cmp
  reports achieved vs requested bitrate. Default 128 kbps at 48 kHz and
  encoder goldens are unchanged.
- M4A/LATM ASC SBR/PS flags seed the decoder: `sbr_present` activates HE
  without waiting for a FIL payload. Output rate is the declared 1× or 2×
  core rate (not a hardcoded 2×). 1× keeps core sample count; 2× without
  payload is pure upsample. Other ratios are `Format`. ADTS still discovers
  SBR from the bitstream. Downsampled high-band reconstruction stays 32-band
  QMF (not on the product path).
- SBR QMF and header history are keyed by channel-element `(kind, tag)`.
  FIL SBR attaches to the preceding SCE/CPE/LFE. Duplicate or
  missing-element SBR extensions are `Format`. HE reconstruction runs
  before speech downmix; mixed output is the non-LFE mean of the
  reconstructed planes. Stereo and multielement HE keep independent
  state. 1× passthrough and he48 goldens are unchanged.
- Independent CCE is applied after IMDCT (`dest += gain * cce_pcm`) with
  CCE overlap keyed by element tag. Unity independent coupling of a
  silent SCE matches the coupling ICS decoded as SCE across frames.
- Dependent CCE is reconstructed on spectral coefficients (FFmpeg
  `apply_dependent_coupling`) at BEFORE_TNS or BETWEEN_TNS_AND_IMDCT.
  A silent CCE is a no-op; unity coupling of a silent SCE matches the
  coupling-channel ICS decoded as SCE. Missing targets are `Format`.
  Non-CCE streams are unchanged.
- Multichannel filterbank overlap is keyed by channel-element `(kind, tag)`,
  not bitstream encounter order. Legal reordering keeps each identity's
  history; identities missing from a frame are dropped so a later tag reuse
  or mono/stereo/layout change starts cold. Duplicate identities in one
  access unit are `Format`. cfg 1–2 still use `fb_l`/`fb_r`.
- In-band and ASC-seeded PCE must declare LC, match the stream sample-rate
  index, and list exactly the channel elements in the access unit. Missing,
  extra, or wrong-type tags, and layouts above 5.1, are `Format` errors
  rather than silent extra planes or synthesized silence. Default
  `channel_configuration` 1–6 mapping is unchanged.
- ASC `channel_configuration=0` now parses the embedded
  `program_config_element` (LC object type, matching core rate, unique
  tags). M4A/LATM seed the decoder map from it; in-band PCE still wins.
  No ISO 14496-26 PCE bitstream is claimed.
- HE `AudioSpecificConfig` parse follows Amd 2/9: explicit AOT 5/29
  consume a core rate and an extension rate (not one rate halved);
  implicit `0x2b7` reads `extensionSamplingFrequencyIndex`; `0x548`
  sets PS. Downsampled SBR (1×) is distinct from dual-rate (2×).
- AAC syntax inventory (`corpus/conformance/`): clause acquisition status
  (ISO 14496-3/26 and 13818-7 full text not obtained), advertised-tool
  coverage matrix, and independently authored ASC/ADTS/LATM/PCE examples
  with expected fields or errors. No conformance certificate. Parser
  follow-ons that can start without purchasing ISO text are listed in
  `go-nogo.md`. Ordinary tests never fetch standards or spawn ffmpeg.

## [0.6.0] - 2026-09-06

### Added

- Encoder TNS (Temporal Noise Shaping) on long-window frames (`enc_tns`):
  per channel, an all-pole LPC (autocorrelation + Levinson–Durbin, order
  ≤ 12 trimmed at trailing |k| < 0.1) of the MDCT spectrum over the psy
  model's coded-band span is quantized to the 4-bit TNS coefficient table
  and the analysis (whitening) FIR runs on the raw L/R spectra before the
  M/S transform — the mirror of the decoder's tool order (TNS inverse
  after the M/S undo). The span is emitted as an order-0 spacer filter
  plus the active filter; every band in the span is force-coded (the
  whitened residual carries real energy across the whole span — a dropped
  band would feed zeros into the decoder's recursion). The psy coded-band
  decisions and masking thresholds come from the original (pre-TNS)
  spectrum via a snapshot with the M/S transform replayed onto it; peaks
  and quantization use the filtered residual. A filter is emitted only
  when the quantized filter's measured prediction gain clears 2 dB —
  steady tones/sweeps barely whiten and stay off (as with lavc); tremolo
  and speech onsets fire. Short frames keep `tns_data_present = 0` (v1).
  The decision path is libm-free (`det_math` + IEEE-exact ops), so the
  byte-exact golden asserts still pin cross-platform output. A/B (test
  knob `set_tns`): lecture speech at 64 kbps stereo +4.0 dB overall SNR
  (+6 dB worst segment), tremolo neutral. Re-minted goldens: `enc48{,m,t}`
  (the `enc48` fixture's silence tail is now an 880 Hz / 8 Hz tremolo so
  the byte-exact golden exercises TNS-on mid-stream).
- Opt-in one-frame attack lookahead for the encoder:
  `EncodeOptions::with_lookahead(true)` (default off — the causal behavior
  and all existing goldens are unchanged). The attack detector runs one
  frame ahead of the encode, so an attack anywhere in frame N+1 makes
  frame N a LongStart (its start-window slope covers the pre-attack tail)
  and frame N+1 an EightShort: a click landing in the first ~448 samples
  of a frame — the causal detector's weak spot, previously coded by the
  LongStart's flat-region long transform — is now coded entirely on short
  windows. Pre-echo in the 10 ms before such an early click after silence
  drops a further 32.7 dB in the A/B measurement (lookahead vs causal,
  both with block switching on). Costs one extra frame (1024 samples) of
  latency in both one-shot and push encode; the push `Encoder` holds the
  frame and flushes it at `finish`, staying byte-exact with one-shot
  `encode_with` for the same options and any feed chunking. New lavc
  golden `src/goldens/enc48l.*` (transient fixture, lookahead on) pins the
  bitstream byte-exactly and decode-matches ffmpeg within the usual
  tolerance.

### Changed

- Encoder M/S stereo is now decided per scalefactor band instead of
  whole-pair per frame. A band goes M/S when its side energy is 3× under
  the weaker original channel (`3·Σs² < min(Σl², Σr²)`, f64 sums — the
  per-band refinement of the previous whole-frame energy comparison);
  mixed frames emit `ms_mask_present = 1` with the per-band `ms_used`
  bitmask (per (window, band), 8 groups of 1 window, on short frames),
  and unanimous frames still collapse to the 2-bit whole-pair masks 0/2.
  Bands where M/S costs more than L/R (e.g. a channel-quiet band, where
  L/R codes one channel and M/S would code two) now stay L/R. The decision
  runs once per frame before the rate loop on the MDCT spectra and is
  platform-deterministic (`+` / `*` only). Re-minted goldens: `enc48t.*`
  is byte-identical (fully correlated fixture stays all-M/S); `enc48{,m}.*`
  drifted only in the decorrelated noise-burst frames.

## [0.5.0] - 2026-09-06

### Added

- Streaming encode API: push `Encoder` (resumable, ADTS only) —
  `Encoder::new(sample_rate, channels, &EncodeOptions)`, then `feed` planar
  f32 chunks of any size and the `on_frame` callback fires once per
  completed AAC frame with a borrowed ADTS-wrapped `EncodedFrame`;
  `finish` encodes the zero-padded tail frame and returns `EncodeInfo`
  tallies (rate / channels / `aac_frames` / `samples` / `bytes`). Output
  is byte-exact with one-shot `encode_with` on the same PCM for any feed
  chunking. M4A is rejected at construction (`stco` / sample sizes need
  the finished totals; use `encode_with`), mirroring the push `Decoder`'s
  ISOBMFF rejection.
- Encoder block switching: a deterministic attack detector (high-passed
  128-sample sub-block energy surge, >8× the running two-frame mean, OR'd
  across channels for the shared CPE window) drives a causal
  OnlyLong → LongStart → EightShort → LongStop state machine — no
  lookahead, no added latency. Transients are now coded on eight 128-bin
  short windows (no `scale_factor_grouping`: 8 groups of 1 window);
  steady content stays OnlyLong byte-for-byte (the `enc48{,m}` goldens
  did not change). Pre-echo in the 10 ms before a click after silence
  drops 19.3 dB in the A/B measurement. A new transient lavc golden
  (`src/goldens/enc48t.*` — castanet clicks over a tone bed) pins the
  short-window bitstream byte-exactly and decode-matches ffmpeg within
  the usual tolerance (measured 1 LSB s16 / ~71 dB SNR).

### Fixed

- Encoder output is now byte-identical across platforms (fixes the two
  `lavc_matches_our_decode_of_our_*` oracle tests on x86_64 CI). Every
  transcendental in the encode decision path routes through the new
  `engine/det_math` — `exp2`/`log2`/`atan`/`sincos` built from
  exactly-rounded ops over literals, `|x|^0.75` via two sqrts — the
  oracle fixture's sweep sine uses `det_math::sincos`, and the aarch64
  NEON MDCT/IMDCT butterflies no longer fuse multiply-add (a 1-ulp
  mismatch with the scalar x86_64 path). The `enc48{,m}` goldens were
  re-minted; the oracle tests keep a byte-exact tripwire plus the
  decode-equivalence tolerance check on the fresh encode.

## [0.4.0] - 2026-09-06

### Added

- AAC-LC encoder: `encode` / `encode_with` / `write` / `write_with` take
  planar f32 PCM (mono or stereo, any ADTS-table sample rate) and produce
  ADTS or M4A (`EncodeOptions`: `EncodeContainer::Adts` default / `M4a`,
  `bitrate_bps` default 128 kbps). Long windows only (KBD analysis, no
  block switching yet), per-frame whole-pair M/S, a Bark-spreading psy
  model (flat 18 dB SMR), CBR-ish via a per-frame global scalefactor
  offset search plus a bounded one-frame bit credit. Errors surface as
  the new `AacError::Encode` variant. Committed lavc goldens
  (`src/goldens/enc48{,m}.*`) pin ffmpeg-decoded PCM against our own
  decode (≤ 2 LSB s16, ≥ 55 dB SNR; measured 1 LSB / ~80 dB).
- Streaming decode API: `Decoder` (resumable push decoder for ADTS and
  LATM/LOAS byte streams — `feed` arbitrary chunks, the `on_frame`
  callback fires once per decoded AAC frame, `finish` flushes and drops a
  partial trailing frame) and `decode_streaming` (slice-based, any
  container incl. M4A/ISOBMFF). Frames are delivered as `Frame` — planar
  f32 in [-1, 1] borrowing decoder scratch, valid for the callback only —
  with `StreamInfo` tallies at the end. The callback returns `Result` so
  consumers can abort mid-stream. Peak PCM RAM is O(frame). Push decoding
  rejects M4A input (`moov` needs random access).
- Multichannel AAC-LC: ADTS/ASC `channel_configuration` 3–6 (3.0 / 4.0 /
  5.0 / 5.1) with correct Center/LFE placement, and in-band
  `program_config_element()` (channelConfiguration 0) channel mapping. Split
  mode emits planes in the libavcodec layout order (5.1 = FL FR FC LFE BL
  BR) or, for PCE streams, in PCE declaration order (front, side, back,
  LFE). CCE elements are consumed without disturbing other channels; full
  CCE gain-element application remains out of scope.
- `goldens/mc{30,40,50,51}.*` multichannel fixtures (per-channel sines) with
  lavc s16 goldens; tests enforce per-plane peak ≥ 1000, max abs ≤ 1 LSB and
  SNR ≥ 70 dB in split mode, plus the speech mono rule on 5.1.
- `goldens/ps48.*` HE-AACv2 fixture with real stereo content (440 Hz L /
  880 Hz R, explicit in-band `EXTENSION_ID_PS`) plus lavc s16 goldens for
  the M4A (elst-trimmed) and ADTS containers, and tests enforcing 1 LSB /
  SNR ≥ 70 dB, non-degenerate stereo output, and explicit PS signalling.

### Changed

- One-shot `decode_with` now layers on the streaming core. Two visible
  consequences: ADTS input with leading junk resyncs to the first valid
  frame instead of answering `NotAac`, and an over-long LATM stream fails
  mid-decode with `TooLong` instead of a decode-class "latm: too long".
- M4A `elst` encoder delay is skipped pre-emission (a skip counter over
  decoded frames) instead of a post-hoc PCM shift; decoded output is
  unchanged.
- Bench refresh (BENCH.md, 2026-09-05): symphonia dev-dep bumped to 0.6
  and oxideav-aac 0.1.7 added as a decode peer (the published tarball
  does decode — ADTS LC + SBR + PS). syom leads LC ADTS/M4A wall, HE
  (~30×/18× vs oxideav), and 5.1.
- Compatible crate versions in `Cargo.lock` (`cc` 1.4.5,
  `wasm-bindgen` 0.2.127). Product `[dependencies]` stay empty.

### Fixed

- One-shot mono decode no longer pays streaming-layer overhead: ADTS/LATM
  mono frames decode straight into the output plane (no per-frame scratch
  or callback copy), `decode_streaming` iterates the input slice zero-copy
  instead of buffering it, and ADTS output planes are pre-sized from a
  frame-header walk. LC ADTS wall 225 → 216 µs (symphonia 0.6: 222 µs),
  allocs/iter 32 → 27, alloc bytes/iter 146 → 70 KiB (BENCH.md,
  2026-09-05).

- Multichannel frames (≥ 3 channels or a PCE) decode each channel through its
  own filterbank state; previously the shared L/R filterbanks bled
  overlap-add tails across channels, and `speech()` mono on > 2 channels
  returned only the last element (e.g. LFE on 5.1). Speech mono is now one
  documented rule: the arithmetic mean of the decoded non-LFE planes.
- LATM/LOAS `StreamMuxConfig()` `crcCheckSum` is verified (CRC-8,
  §1.8.4.5) instead of discarded; a mismatch rejects the stream.
- PS first-envelope H-matrix interpolation now starts one slot before the
  frame (`(n + 1)/(n_0 + 1)`, Annex 8.A / §8.6.4.6.4) instead of at slot 0
  (`n/n_0`). HE-AACv2 output now matches lavc within 1 LSB on real stereo
  content (was up to 11 LSB / 67 dB SNR whenever the stereo cues moved;
  dual-mono fixtures could not see it).

## [0.3.0] - 2026-09-03

### Changed

- Product is a zero-dep AAC library (`cargo add syom`). Output is planar
  `f32` at native rate. Caps: `DecodeOptions::speech()` / `unbounded()`.
- Crate root is decode / sniff / options. ISO-BMFF demux (`parse_aac_track`)
  is crate-private.
- LC decode is f32 from spectrum through IMDCT/OLA. ICS/spectrum
  buffers are reused across frames. On the committed LC fixtures,
  in-process wall time is ≤ Symphonia and ≪ rusty_aac; allocs and
  peak RSS are ≤ both linked decode peers (`BENCH.md`).

### Added

- HE-AAC v1/v2 (SBR/PS) and LATM/LOAS on the same decode path. Committed
  lavc native goldens (max abs ≤ 1 LSB, SNR ≥ 70 dB) for lecture M4A,
  44.1 M4A, ADTS sine, TNS, PNS-heavy LC, HE ADTS/M4A/LATM, and LC LATM.
  Runtime does not spawn ffmpeg.
- Owned IMDCT/FFT (no rustfft). PNS polarity matches lavc PCM.

### Fixed

- MP4 audio `elst.media_time` is honoured (AAC encoder delay).
- Implicit SBR `0x2b7` probe requires `sbrPresentFlag == 1` so LC ASC
  padding `56 e5 00` is not treated as HE.
- `speech()` / `decode()` apply SBR on HE-AAC. The mono fast path no
  longer returns core-rate silence.
- `unbounded()` / Split emits two planes on mono LC+SBR (lavc implicit
  HE-AACv2 PS), including LATM/LOAS. Goldens are interleaved lavc s16.
- After SBR is active, a malformed SBR fill is `Err`, not dropped.
- Mono mix-down after SBR/PS is one rule (ADTS, M4A, LATM). LATM
  honours the duration cap while decoding, not after the whole PCM.



[Unreleased]: https://github.com/ekhodzitsky/syom/compare/v0.8.1...HEAD
[0.9.0]: https://github.com/ekhodzitsky/syom/compare/v0.8.1...HEAD
[0.8.1]: https://github.com/ekhodzitsky/syom/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/ekhodzitsky/syom/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/ekhodzitsky/syom/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/ekhodzitsky/syom/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/ekhodzitsky/syom/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/ekhodzitsky/syom/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/ekhodzitsky/syom/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ekhodzitsky/syom/releases/tag/v0.3.0
