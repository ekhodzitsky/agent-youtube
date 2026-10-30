# syom

AAC to planar `f32`.

[![crates.io](https://img.shields.io/crates/v/syom.svg)](https://crates.io/crates/syom)
[![docs.rs](https://img.shields.io/docsrs/syom)](https://docs.rs/syom)
[![ci](https://github.com/ekhodzitsky/syom/actions/workflows/ci.yml/badge.svg)](https://github.com/ekhodzitsky/syom/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An AAC codec in Rust. AAC-LC, HE-AAC v1/v2, and AAC-LD. ADTS, LATM/LOAS,
M4A, and bounded fragmented MP4. Encode defaults to AAC-LC at 128 kbps,
ADTS or M4A. HE and LD are opt-in. Output is planar `f32` at the
bitstream rate (HE is twice the core rate). The product path pulls no
crates. ffmpeg is an offline oracle, not a runtime dependency.

## Examples

```rust
fn main() -> syom::Result<()> {
    let pcm: Vec<f32> = (0..48_000).map(|i| 0.3 * (i as f32 * 0.06).sin()).collect();
    let adts = syom::encode(&[&pcm], 48_000)?;
    let back = syom::decode_with(&adts, &syom::DecodeOptions::audio())?;
    assert_eq!((back.channels.len(), back.sample_rate), (1, 48_000));
    Ok(())
}
```

`decode` is the speech default (mono mix, 2 h at up to 48 kHz; higher
rates keep that sample budget). `DecodeOptions::audio()`
keeps the coded channels. From a path: `syom::read`. Push decode and
encode: `Decoder::feed`, `Encoder::feed`. The
[guide](https://docs.rs/syom/latest/syom/guide/) has one compiling
example per task.

| task | section |
|---|---|
| one-call encode / decode, speech vs full fidelity | 1 |
| borrowed PCM, `Read` / `Write` / `Seek` | 2 |
| priming, tail, exact length | 3 |
| push `Encoder` / `Decoder`, bounded memory | 4 |
| raw access units and `AudioSpecificConfig` | 5 |
| probe, sample-exact M4A seek | 6 |
| LC, quality VBR, HE v1/v2, surround | 7 |
| limits and typed errors | 8 |
| profiles and containers | 9 |
| behaviour changes since 0.6.0 | 10 |

## Decode and encode

| | ADTS | M4A | LATM/LOAS | fMP4 |
|---|---|---|---|---|
| AAC-LC decode | yes | yes | yes | yes |
| HE-AAC v1/v2 decode | yes | yes | yes | yes |
| AAC-LD decode (512) | no | yes | yes | yes |
| LC encode | yes | yes | yes | no |
| HE v1/v2 encode | yes | yes | yes | no |
| AAC-LD encode | no | yes | yes | no |

LC encode is the default; M/S is per-band. `with_he`, `with_he_v2`, and
`with_ld` are opt-in. ADTS cannot signal AAC-LD. Fragmented MP4 uses the
same ASC path as M4A, so AAC-LD decodes there too; there is no separate
LD fMP4 golden. Surround LC is 3, 4, 5, 6, or 8 planes
(`channel_configuration` 3–7). Seven planes have no layout.
No resample.

## Compared with rusty_aac, Symphonia, oxideav-aac

| | syom | rusty_aac 0.5 | symphonia 0.6 | oxideav-aac 0.1.7 |
|---|---|---|---|---|
| Job | AAC codec | LC decoder | media pipeline | ADTS decoder |
| Default deps | none | none | several | oxideav-core |
| HE-AAC v1/v2 decode | yes | signalled | LC core | ADTS |
| AAC-LD | yes | no | no | no |
| M4A / LATM / fMP4 | yes | no | M4A, no fMP4 | no |
| LC / HE / LD encode | LC; HE and LD opt-in | no | no | no |
| Output | planar `f32`, native rate | interleaved f32 | packets | interleaved i16 |
| Duration / RAM caps | yes | no | no | no |
| One call `decode(&[u8])` | yes | no | no | `decode_all` |

## Speed

Linux x86_64, AMD Ryzen AI 9 HX 370, rustc 1.97.1, `taskset -c 0`,
profile.bench (thin LTO), 20 reps after 4 warmup, 2026-10-01. Same
bitstream and the same output shape. Sample checksums differ. A peer
that cannot produce that shape is left blank. Older rows of a different
protocol: [BENCH.md](BENCH.md).

| Workload | syom | symphonia 0.6.1 | rusty_aac 0.5.0 | oxideav-aac 0.1.7 |
|---|---|---|---|---|
| LC ADTS, 48 kHz mono, 13312 samples | **72.2 µs** | 123 µs | 179 ms | 175 ms |
| HE-AAC v2 ADTS, 48 kHz stereo, 53248 samples | **4.61 ms** | | | 369 ms |
| LC 5.1 ADTS, 48 kHz, 20480 samples | **640 µs** | | | 1.67 s |

## Install

```toml
[dependencies]
syom = "0.7"
```

rustc **1.89**.

[guide](https://docs.rs/syom/latest/syom/guide/) | [BENCH.md](BENCH.md) | [CHANGELOG](CHANGELOG.md)

No resample. MIT. AAC patents: [NOTICE](NOTICE).
