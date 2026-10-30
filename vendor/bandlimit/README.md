# bandlimit

Bandlimited rational resampling to planar `f32`.

[![ci](https://github.com/ekhodzitsky/bandlimit/actions/workflows/coverage.yml/badge.svg)](https://github.com/ekhodzitsky/bandlimit/actions/workflows/coverage.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

A resampler in Rust. Integer rates, one rational ratio, planar `f32`.
Equal rates copy every sample, including non-finite values. Unequal rates
reject the first non-finite sample and write nothing. The library pulls
no crates and does not link SoXR, Speex, r8brain, rubato, FFmpeg, or
gigastt-resample. Private. Not published.

## Examples

48 kHz to 16 kHz, one shot:

```rust
fn main() -> Result<(), bandlimit::Error> {
    fn rate(hz: i64) -> bandlimit::Rate {
        match bandlimit::Rate::new(hz) {
            Ok(rate) => rate,
            Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
        }
    }

    let output = bandlimit::convert(
        &[0.0, 0.25, -0.25],
        rate(48_000),
        rate(16_000),
    )?;
    assert_eq!(output.len(), 1);
    Ok(())
}
```

`Stream::new` fixes the ratio. `feed` keeps a converting chunk until
`finish`. `write` appends into a caller-owned buffer. `convert_planes`
is one `convert` per plane.

## Rates to 16 kHz

These eight mono ratios are the speed and memory claim, measured on one
Apple M1 Pro. Full cells: [scorecard](docs/scorecard.md). Kernel notes:
[exact-phase](docs/exact-phase.md).

| In | Filter |
|---|---|
| 8 kHz | 33-tap halfband |
| 22.05 kHz | Kaiser, 235 taps, 640 phases |
| 24 kHz | Blackman-Harris 2, 373 taps |
| 32 kHz | Kaiser, 339 taps |
| 44.1 kHz | Kaiser, 467 taps, 320 phases |
| 48 kHz | Kaiser, 443 taps |
| 96 kHz | cascade, public length 365 |
| 192 kHz | cascade, public length 391 |

Each of those eight is faster than SoXR, Speex, r8brain, rubato, FFmpeg,
and gigastt on the recorded median, and its warm peak is strictly under
each peer. A whole-buffer `Stream::write` plus `finish` cleared the same
speed gate. Those cells are in [write](docs/write.md) and do not replace
the `convert` tables. Other ratios resample. They are not in that claim.
The same rate copies every sample.

## Install

```toml
[dependencies]
bandlimit = { path = "." }
```

rustc **1.88**. Edition 2024. No runtime crates.

[scorecard](docs/scorecard.md) | [write](docs/write.md) |
[quality](docs/quality.md) | [latency](docs/latency.md) |
[contract](docs/contract.md) | [exact-phase](docs/exact-phase.md) |
[research](research/README.md) | [CHANGELOG](CHANGELOG.md)

The public contract is not frozen. No `s16`. MIT OR Apache-2.0.
