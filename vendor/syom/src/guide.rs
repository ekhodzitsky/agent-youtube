//! Task-oriented guide (TASK-107). Every example here is self-contained —
//! it makes its own PCM — so it runs from the published crate as well as
//! from the repository.
//!
//! # 1. One call each way
//!
//! ```
//! // A second of stereo: planar f32 in [-1, 1], one Vec per channel.
//! let l: Vec<f32> = (0..48_000).map(|i| 0.3 * (i as f32 * 0.06).sin()).collect();
//! let r: Vec<f32> = l.iter().map(|x| 0.5 * x).collect();
//! let adts = syom::encode(&[l, r], 48_000)?;            // AAC-LC, 128 kbps, ADTS
//!
//! // `decode` is the SPEECH default: mono mix. 2 h at rates up to 48 kHz;
//! // rates up to 192 kHz are accepted, but the sample budget stays 7200×48_000.
//! let speech = syom::decode(&adts)?;
//! assert_eq!(speech.channels.len(), 1);
//! // Keep the coded channels: `audio()` (same caps) or `unbounded()`.
//! let full = syom::decode_with(&adts, &syom::DecodeOptions::audio())?;
//! assert_eq!((full.channels.len(), full.sample_rate), (2, 48_000));
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! Nothing is resampled, ever: PCM comes out at the stream's rate
//! (`sample_rate`; for HE-AAC that is twice `core_rate`). The only
//! downmix is the documented speech mono (mean of the non-LFE planes);
//! every other mode returns the coded planes in the order of
//! [`crate::mpeg_channels`].
//!
//! # 2. Borrowed PCM, `Read` and `Write`
//!
//! ```
//! use std::io::Cursor;
//! let pcm = vec![0.1f32; 8192];
//! let planes: [&[f32]; 1] = [&pcm];                       // no copy is made
//! let mut file_like = Vec::new();
//! let info = syom::encode_write(&mut file_like, &planes, 44_100, &syom::EncodeOptions::adts())?;
//! assert_eq!(info.samples, 8192);
//!
//! // Any `Read` for ADTS / LATM; M4A needs `Read + Seek` (the index is at the end).
//! let from_read = syom::decode_read(Cursor::new(&file_like))?;
//! assert_eq!(from_read.sample_rate, 44_100);
//! let mut m4a = Cursor::new(Vec::new());
//! syom::encode_write_m4a(&mut m4a, &planes, 44_100, &syom::EncodeOptions::m4a())?;
//! let from_seek = syom::decode_seek(Cursor::new(m4a.into_inner()))?;
//! assert_eq!(from_seek.channels[0].len(), 8192);           // M4A trims priming and tail
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! # 3. Timing: priming, tail, exact length
//!
//! An AAC encoder delays the signal (LC 1024 samples, HE 3018 output
//! samples, AAC-LD 512) and pads the last frame. A one-shot ADTS encode stores that
//! delay in a leading `iTunSMPB` tag. Decode returns the source length
//! and reports both edges when the decoded sample count matches the tag.
//! `probe` reports the same tag once the frames in the buffer fill it.
//! LATM has no delay tag, so its decoded length is whole frames and
//! starts at the priming sample. M4A carries an edit list, which wins
//! over a tag. A flat M4A or one-shot fragmented MP4 with no edit list
//! trims from `iTunSMPB` in `moov` under the same count check.
//!
//! ```
//! let pcm = vec![0.2f32; 5000];
//! let adts = syom::encode(&[&pcm[..]], 48_000)?;
//! let m4a = syom::encode_with(&[&pcm[..]], 48_000, &syom::EncodeOptions::m4a())?;
//! let opts = syom::DecodeOptions::audio();
//! let a = syom::decode_with(&adts, &opts)?;
//! let m = syom::decode_with(&m4a, &opts)?;
//! assert_eq!(a.channels[0].len(), 5000);
//! assert_eq!((a.priming, a.remainder), (Some(1024), Some(120)));
//! assert_eq!((m.channels[0].len(), m.priming), (5000, Some(1024)));
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! # 4. Streaming with bounded memory
//!
//! ```
//! use syom::{DecodeOptions, Decoder, EncodeOptions, Encoder};
//! let pcm = vec![0.1f32; 10_000];
//! let mut enc = Encoder::new(48_000, 1, &EncodeOptions::adts())?;
//! let mut stream = Vec::new();
//! for chunk in pcm.chunks(777) {                           // any chunking, same bytes
//!     enc.feed(&[chunk], |f| { stream.extend_from_slice(f.au); Ok(()) })?;
//! }
//! let info = enc.finish(|f| { stream.extend_from_slice(f.au); Ok(()) })?;
//! assert_eq!((info.samples, info.priming), (10_000, 1024));
//!
//! let mut dec = Decoder::new(DecodeOptions::audio());
//! let mut samples = 0usize;
//! for packet in stream.chunks(1500) {                      // network-sized pieces
//!     // `Frame` borrows the decoder's buffers: copy what you keep.
//!     dec.feed(packet, |frame| { samples += frame.samples; Ok(()) })?;
//! }
//! let done = dec.finish(|frame| { samples += frame.samples; Ok(()) })?;
//! assert_eq!(done.samples as usize, samples);
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! A frame reaches the callback on the byte (decode) or sample (encode)
//! that completes it. A callback error fails the instance; `reset` reuses
//! it. The push decoder takes ADTS, LATM and fragmented MP4 (init segment
//! + fragments, any chunking); flat M4A stays random-access — see
//!   [`crate::decode_seek_streaming`] or [`crate::M4aSeek`].
//!
//! `Encoder::feed` emits raw access units, so the concatenation above has
//! no delay tag and the decoded length stays whole frames. `encode`,
//! `encode_with`, `write` and `encode_write` prepend the tag.
//!
//! # 5. Raw access units (RTP, your own container)
//!
//! ```
//! use syom::{DecodeOptions, Decoder, EncodeOptions, Encoder};
//! let pcm = vec![0.1f32; 4096];
//! let mut enc = Encoder::new(48_000, 1, &EncodeOptions::raw())?;
//! let asc = enc.asc().to_vec();                            // AudioSpecificConfig
//! let mut aus: Vec<Vec<u8>> = Vec::new();
//! enc.feed(&[&pcm[..]], |f| { aus.push(f.payload.to_vec()); Ok(()) })?;
//! enc.finish(|f| { aus.push(f.payload.to_vec()); Ok(()) })?;
//!
//! let mut dec = Decoder::from_asc(&asc, DecodeOptions::audio())?;
//! let mut frames = 0;
//! for au in &aus {
//!     dec.decode_au(au, |_| { frames += 1; Ok(()) })?;
//! }
//! assert_eq!(frames, aus.len());
//! // Or re-frame them: `wrap_adts_au`, `wrap_loas_au`, `mux_raw_lc_m4a`.
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! # 6. Probing and seeking
//!
//! ```
//! use std::io::Cursor;
//! use syom::{DecodeOptions, M4aSeek, ProbeDuration, ProbeTrim};
//! let pcm: Vec<f32> = (0..48_000).map(|i| 0.2 * (i as f32 * 0.05).sin()).collect();
//! let m4a = syom::encode_with(&[&pcm[..]], 48_000, &syom::EncodeOptions::m4a())?;
//! let p = syom::probe(&m4a)?;                              // no PCM is decoded
//! assert_eq!(p.duration, ProbeDuration::Exact { samples: 48_000 });
//! assert!(matches!(p.trim, ProbeTrim::Exact { priming: 1024, .. }));
//! // Untagged ADTS and LATM: duration and trim stay `Unknown`.
//!
//! let mut seek = M4aSeek::open(Cursor::new(m4a), DecodeOptions::audio())?;
//! seek.seek(24_000)?;                                      // presentation samples
//! let mut got = 0u64;
//! seek.decode(|f| { got += f.samples as u64; Ok(()) })?;
//! assert_eq!(got, 24_000);                                 // sample-exact, pre-roll hidden
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! Untagged ADTS and LATM leave duration and trim `Unknown`. A leading
//! `iTunSMPB` tag whose sample counts equal the frames in the buffer
//! fills both.
//!
//! # 7. Choosing an encoder mode
//!
//! | need | options | notes |
//! |---|---|---|
//! | default | `EncodeOptions::default()` | LC, 128 kbps ABR, causal |
//! | transparent-leaning | `high_quality()` | LC 192 kbps |
//! | constant quality | `with_quality(0..=10)` | LC only, no rate target |
//! | about 48–64 kbps | `low_rate()` / `with_he(true)` | HE v1: half-rate core + SBR |
//! | about 16–40 kbps stereo | `with_he_v2(true)` | mono core + parametric stereo |
//! | 512-sample delay | `with_ld(true)` | AAC-LD, LOAS or M4A, not ADTS, not the default |
//! | 3.0 – 7.1 | 3, 4, 5, 6 or 8 planes | LC only; 5.1 = FL FR FC LFE BL BR |
//!
//! `bitrate_bps` is whole-stream. HE needs a 16–48 kHz input whose half is
//! an AAC rate. Containers: [`crate::EncodeContainer`] `Adts`, `Latm`,
//! `M4a`, `Raw` (push encoder only). One-shot ADTS output starts with the
//! delay tag. `Encoder::feed` does not.
//!
//! ```
//! use syom::EncodeOptions;
//! let l: Vec<f32> = (0..32_768).map(|i| 0.3 * (i as f32 * 0.04).sin()).collect();
//! let r: Vec<f32> = l.iter().map(|x| 0.3 * x).collect();
//! let v2 = EncodeOptions::adts().with_he_v2(true).with_bitrate_bps(32_000);
//! let small = syom::encode_with(&[&l[..], &r[..]], 48_000, &v2)?;
//! let lc = syom::encode(&[&l[..], &r[..]], 48_000)?;
//! assert!(small.len() * 3 < lc.len());
//! let six = vec![l.clone(); 6];                            // 5.1
//! let out = syom::decode_with(&syom::encode(&six, 48_000)?, &syom::DecodeOptions::audio())?;
//! assert_eq!(out.layout, syom::Layout::Mpeg(6));
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! # 8. Limits and errors
//!
//! Decoding hostile input is bounded by [`crate::DecodeOptions`]: duration
//! and rate caps plus [`crate::MemoryBudgets`] (input, output, index,
//! workspace, channels). Exceeding one is [`crate::AacError::Limit`] or
//! `TooLong`, never an allocation spree. Errors are typed; match the
//! class, not the text, and keep a `_` arm (`#[non_exhaustive]`).
//!
//! ```
//! use syom::{AacError, DecodeOptions, PcmReject};
//! assert!(matches!(syom::decode(b"not aac at all"), Err(AacError::NotAac)));
//! let too_loud = vec![1.5f32; 2048];
//! assert!(matches!(
//!     syom::encode(&[too_loud], 48_000),
//!     Err(AacError::InvalidPcm(PcmReject::Amplitude))       // no silent clipping
//! ));
//! let pcm = vec![0.1f32; 48_000 * 3];
//! let adts = syom::encode(&[pcm], 48_000)?;
//! let short = DecodeOptions::audio().with_max_duration_secs(1.0);
//! assert!(syom::decode_with(&adts, &short).is_err());       // cap, not truncation
//! # Ok::<(), syom::AacError>(())
//! ```
//!
//! # 9. What is and is not supported
//!
//! Decode: AAC-LC, HE-AAC v1 (SBR), HE-AAC v2 (PS); ADTS, M4A / ISOBMFF,
//! LATM / LOAS, raw access units with a config; mono to 7.1 and streams
//! with a Program Config Element. Bounded fragmented MP4 (one unencrypted
//! AAC track, init `moov` + `moof`/`trun`, DASH init + segments) decodes
//! on the slice, seekable and push (`Decoder::feed`, any chunking) paths;
//! truncated fragments fail typed
//! [`crate::AacError::Truncated`]. Not decoded:
//! channel configurations 8–15 (both are
//! [`crate::AacError::Unsupported`]), AAC-ELD, USAC, multi-track or
//! encrypted fMP4, DRM. 960-sample LC decodes from LATM/LOAS, M4A, and a
//! raw access unit. HE-AAC on a 960-sample core is
//! [`crate::AacError::Unsupported`], not a 960-line LC frame.
//! AAC-LD (AOT 23, 512- or 480-sample frames) decodes from LATM/LOAS, M4A,
//! bounded fragmented MP4, and a raw access unit with an ASC. An M4A `elst` trims priming and the tail
//! in samples, and seeking replays one overlap frame. A flat M4A or
//! one-shot fragmented MP4 with no edit list trims from `iTunSMPB` in
//! `moov` when the counts match; an edit list wins. A push fragmented
//! MP4 decode does not read that tag. `probe` reports
//! [`crate::ProbeProfile::Ld`]. ADTS cannot signal AOT 23. The decode is
//! qualified against FDK and libxaac, not libavcodec (it rejects these
//! streams) and not ISO 14496-26 vectors (not obtained).
//!
//! Encode: LC (ABR, opt-in CBR, or quality VBR), HE v1, HE v2, surround
//! LC, and opt-in AAC-LD (`with_ld`, 512-sample frames, LOAS or M4A,
//! priming 512; not ADTS and not the default). `with_cbr` is LC mono or
//! stereo: a bit reservoir inside 6144 bits per channel. Default ABR keeps
//! ADTS fullness at `0x7FF`. Not encoded: PCE layouts, HE surround, ELD,
//! 480-sample LD.
//!
//! # 10. Behaviour changes since 0.6.0
//!
//! - Minimum Rust is 1.89 (was 1.97).
//! - 3, 4, 5, 6 or 8 planes now encode as surround; they used to be an
//!   error. 7 planes and more than 8 still are.
//! - Channel configurations 8–15 are a typed error on decode; they used to
//!   decode as unlabeled planes.
//! - LC rate control changed (noise-to-mask allocation): the same options
//!   produce different, better-allocated bytes.
//! - Opt-in AAC-LD encode: `EncodeOptions::with_ld` (LOAS or M4A).
//! - New: `with_quality`, `with_he`, `with_he_v2`, `high_quality`,
//!   `low_rate`, `EncodeContainer::Latm`, `encode_write`,
//!   `encode_write_m4a`, `wrap_loas_au`, 7.1 decode, borrowed-plane encode.
//! - One-shot ADTS encode prepends an `iTunSMPB` tag. Decode trims when
//!   the counts match, and `probe` reports the tag when the buffered
//!   frames fill it. Push `Encoder` frames stay raw access units.
//! - Parametric-stereo IID gains are a const table from `exp10_f64`.
