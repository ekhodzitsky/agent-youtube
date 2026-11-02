# syom

A zero-dependency Rust library for decoding AAC to planar `f32` and encoding
AAC. Read files or bounded streams, preserve channel layout, and pass PCM
directly to your application at the native sample rate.

[![crates.io version](https://img.shields.io/crates/v/syom?style=flat)](https://crates.io/crates/syom)
[![crates.io monthly downloads](https://img.shields.io/crates/dm/syom?style=flat)](https://crates.io/crates/syom)
[![Main CI: last hosted run](https://github.com/ekhodzitsky/syom/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/ekhodzitsky/syom/actions/workflows/ci.yml?query=branch%3Amain)
[![Library line coverage: 92.71%, local 2026-11-02](https://img.shields.io/badge/library%20lines-92.71%25-brightgreen?style=flat)](https://github.com/ekhodzitsky/syom/blob/main/lab/baseline/COVERAGE.md#local-measurement-2026-11-02)
[![Declared MSRV: Rust 1.89](https://img.shields.io/badge/MSRV-1.89-blue?style=flat)](Cargo.toml)
[![MIT license](https://img.shields.io/badge/license-MIT-blue?style=flat)](LICENSE)
[![Published API documentation](https://img.shields.io/docsrs/syom?style=flat)](https://docs.rs/syom)

## Quick start

Requires Rust **1.89+**. Cargo downloads the published **0.8.1** crate;
the example generates its own audio and needs no media files or native
codec installation.

```sh
cargo new syom-demo --bin
cd syom-demo
cargo add syom@0.8.1
```

Replace `src/main.rs` with:

```rust
fn main() -> syom::Result<()> {
    let pcm: Vec<f32> = (0..48_000)
        .map(|i| 0.3 * (i as f32 * 0.06).sin())
        .collect();
    let adts = syom::encode(&[&pcm], 48_000)?;
    let audio = syom::decode_with(&adts, &syom::DecodeOptions::audio())?;
    assert_eq!((audio.channels.len(), audio.sample_rate), (1, 48_000));
    println!("{} channel at {} Hz", audio.channels.len(), audio.sample_rate);
    Ok(())
}
```

Run `cargo run --release`. Expected output: `1 channel at 48000 Hz`.

`DecodeOptions::audio()` preserves channels; `decode()` defaults to a mono
speech mix with duration and memory limits. Output is not resampled.
For files use `read`; for incremental input/output use `Decoder::feed`
and `Encoder::feed`. LC encoding defaults to 128 kbps ABR with per-band
M/S. HE and LD encoding are opt-in.

[API guide](https://docs.rs/syom/latest/syom/guide/) ·
[Usage, formats and validation](https://github.com/ekhodzitsky/syom/blob/main/docs/usage-and-validation.md) ·
[Changelog](CHANGELOG.md)

## Choosing a library

Published-package snapshots for applications handling AAC:

| Library | Integration boundary | HE-AAC v1/v2 decode |
|---|---|---|
| syom 0.8.1 | Built-in ADTS, M4A and LATM readers; planar `f32` | Supported |
| rusty_aac 2.0.1 | Caller extracts M4A access units; interleaved `f32` | Supported |
| Symphonia 0.6.1 | Multi-format media pipeline and audio buffers | Native AAC-LC core only |

Choose Symphonia when one pipeline must handle other codecs and containers.
Choose rusty_aac when its codec profiles or interleaved output suit an
existing demuxer. Changing libraries can require different channel layout,
gapless trimming and buffer handling.
[Detailed comparison and primary sources](https://github.com/ekhodzitsky/syom/blob/main/docs/comparison.md)
record pinned versions and verification dates, including oxideav-aac.

## Measured decoding time

Historical medians; **lower is better**. These measure **syom 0.8.0**, not
the current development candidate:

| ADTS workload | syom 0.8.0 | rusty_aac 2.0.1 | Symphonia 0.6.1 |
|---|---:|---:|---:|
| LC mono, 48 kHz, 13312 samples | 59.6 µs | 152.3 µs | 135.2 µs |
| HE v2 stereo, 48 kHz, 53248 samples | 1.211 ms | 1.120 ms | Core-only; excluded |

Linux x86_64, Ryzen AI 9 HX 370, rustc 1.97.1; one core, release thin LTO,
8 warmups and 101 measured calls, 2026-10-28. Adapters collect the same
planar shape; checksums differ. The busy-host sample favors different
libraries on different profiles and establishes no quality ranking.
[Method, all cells and raw results](https://github.com/ekhodzitsky/syom/blob/main/docs/comparison.md#historical-decoding-measurements)
preserve unfavorable cases. [BENCH.md](BENCH.md) retains older results.

## Status and limits

The **0.9.0 unpublished candidate** adds budgeted exact M4A seeking for
LC/LD mono/stereo and fixes cold decoder stack initialization. Exact plans
require prefix replay, not arbitrary sparse access. For this checkout use
`syom = { version = "0.9", path = "../syom" }`;
[the candidate guide](https://github.com/ekhodzitsky/syom/blob/main/src/guide.rs)
explains migration and replay limits.

Version, downloads and docs badges describe the registry; CI shows the
last hosted `main` run. Hosted checks are paused. Candidate validation is
local Linux x86_64, including Rust 1.89; other-platform parity is not
established. The local [library-line coverage report](https://github.com/ekhodzitsky/syom/blob/main/lab/baseline/COVERAGE.md#local-measurement-2026-11-02)
records 92.71% on 2026-11-02; integration tests, doctests and branch coverage
are outside that measurement.

Physical-mobile, full ISO conformance and listening qualification remain
open. Encoder quality leadership is unproven; LC encoding still trails
FDK on the long speech cases. See the
[validation record](https://github.com/ekhodzitsky/syom/blob/main/docs/usage-and-validation.md#validation-and-integration)
for consumer evidence and remaining limits.

[Questions and bugs](https://github.com/ekhodzitsky/syom/issues) ·
[Contributing](https://github.com/ekhodzitsky/syom/blob/main/CONTRIBUTING.md) ·
[MIT license](LICENSE) · [AAC patent notice](NOTICE)
