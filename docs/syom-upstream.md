# syom upstream integration

[Documentation index](README.md) · [Project overview](../README.md)

## Published syom 0.10.3 - source 0.17.0

Current source selects exactly `syom = "=0.10.3"` from crates.io. Cargo.lock
records the published archive checksum; no syom Git dependency, local source
override or vendor copy is used. The registry package records upstream revision
`2be19a2abbaea2261f82819d5e90bba5b8d28ed9` for qualification provenance.

The release fixes fractional M4A edit durations, including a millisecond movie
clock at 44.1 kHz, and standard Apple gapless tag interpretation. Fractional
presentation durations round down to native samples; qualified sub-tick,
sub-millisecond overhang is capped at available flat-file media after priming.
Missing packets and larger inconsistencies remain errors. No PCM is padded.
The upstream changes can intentionally affect presentation lengths or trimming;
historical cross-version equality measurements below retain their dated scope.

The media backend changes to **v5** because persistent checkpoints are tied to
the exact syom version. Older jobs, including v4, are refused before mutation.
Keep their original executable for resume or use a new work directory. Existing
transcript/export formats remain readable. bandlimit's resampler checkpoint
extension remains separate, and no runtime dependency is added by syom.

Verified 2026-11-09 on Linux x86_64: seven synthetic stereo AAC inputs at
8/16/22.05/24/32/44.1/48 kHz retain identical full PCM16 hashes and frame counts
against the former 0.10.0 candidate. All fourteen same-backend, separate-process
checkpoint resumes match uninterrupted output. Native PNS state and resampler
history remain covered by the application's seven-rate tests.

The fractional-duration regression first reproduced the old codec's rejection
of a 44.1 kHz presentation on a millisecond movie clock. With 0.10.3, both full
and resumable decode match an independently authored integer-sample reference
window through indexed EOF. An additional movie tick exceeds the rounding
allowance and fails while preserving source bytes. These checks do not claim
universal cross-version equality: Apple tag and fractional-window fixes can
intentionally change previously incorrect presentation trimming.

The v4/v5 cross-build refusal check preserves every saved job file, after which
the original executable completes that job. Formatting, strict all-target
Clippy, 436 Rust tests (24 opt-in tests ignored), 75 Python tests, Rust 1.94
all-target checking and 31-document link validation pass. The locked license
collector reports no missing texts. New trials use synthetic fixtures; historical
real-recording results below were not rerun for this update. Raw local evidence
is under `runs/syom-0103-upgrade/`.

## Historical 0.10.0 candidate migration - source 0.16.0

The following record describes the qualification performed on 2026-11-03; its
versions and numerical results do not describe fresh 0.10.3 trials.

The selected codec is unmodified syom **0.10.0 source candidate** at
`1383825e013f95ee565eb094122f3ec6fa2f7193`, pinned by Git revision in Cargo.toml
and Cargo.lock. This candidate is not a published crates.io release. The local
`vendor/syom` fork is removed; builds fetch the immutable upstream source or use
Cargo's Git cache. bandlimit retains its separate resampler checkpoint extension,
and ryf remains a registry dependency. No runtime crate or feature is added.

## State and completion integration

Upstream now provides the required persistent AAC-LC state and strict M4A
presentation checks. The application uses its qualified mono-output checkpoint
API, retaining both channel filterbank histories and PNS state where supported.
Other profiles/history retain exact replay. Upstream state is opaque and bound
to its implementation/options; the application still validates the source,
backend and saved-state integrity before restoration. A source geometry tag
inside the codec does not authenticate the payload or checkpoint.

The upstream `SYOMLC01` checkpoint is 8,422 bytes and replaces the consumer's
`YTLC0001` format. Old bytes are not converted. Tests reject the old magic and
changed decoder budgets without moving the fresh decoder position, then restore
valid state and compare native PCM and resampled output at seven rates.

`PresentationCompletion` distinguishes bounded completion/qualified rounding
from indexed EOF on an unbounded source. The latter does not certify a declared
recording duration; the pipeline retains its independent recording-duration
check. Accepting indexed EOF requires that the complete available index was
consumed with zero remaining indexed samples. Partial range acquisition keeps
its existing verified-prefix contract and never claims full-source verification.
Strict EOF now errors directly on an oversized bounded presentation. The rounded
presentation regression uses an interior AU checkpoint because upstream refuses
state capture at EOF. No PCM is padded to satisfy container metadata.

## Migration policy

The media backend changes from v3 to revision-qualified **v4**. Jobs created by a
different backend are refused before mutation, including completed-job reuse.
Use the original executable for those jobs or start a new work directory.
Source media, checkpoints and raw ASR are preserved. Canonical transcripts and
export formats are unchanged; existing transcripts can be exported independently
of the original media backend. No real speech is re-recognized by this migration.

## Verification

Local qualification is on Linux x86_64. The retained historical
[0.15.0 comparison](syom-upgrade.md) describes the former fork and is not a claim
that its checkpoint format is compatible. Current local evidence covers the
consumer's exact-state, completeness, partial-prefix and old-job boundaries.

Verified 2026-11-03. All ten full PCM16 comparisons against the previous v3
backend match in sample count and SHA-256; all twenty same-backend process-restart
comparisons match uninterrupted output.

| Scope | Inputs | Full PCM equality | Fresh-process resume equality |
| --- | ---: | ---: | ---: |
| Synthetic stereo at seven input rates | 7 | 7/7 | 14/14 |
| Synthetic ten-minute audio | 1 | 1/1 | 2/2 |
| Retained real 52.96475-second audio | 1 | 1/1 | 2/2 |
| Retained real 3:07:51.0124375 audio | 1 | 1/1 | 2/2 |

Short and rate fixtures use debug tests; the ten-minute and multi-hour cases use
optimized tests. These are equality checks, not cross-profile speed measurements.
The multi-hour output contains 180,336,199 frames with SHA-256
`0f64098a8f995801cdb4c2a79a9fb87dd648d78baf0bbe554223984d129aadcd`.
A real 59,636,979-byte prefix reproduces the full-decode selection at
3600.123..3660.123 seconds with PCM SHA-256
`38dee12f7cb7ebc02bd10dba90d4d586d50fd6722859c4751f4c5e5893a75238`.

The cross-build old-job refusal check leaves every saved file unchanged; the
original build can subsequently finish the job. Two completed real transcript
exports also preserve every original job file. No media or ASR output is
rewritten by these checks.

Application gates pass: formatting, strict all-target Clippy, 435 Rust tests
(24 existing opt-in tests ignored), 75 Python tests and documentation links.
Rust 1.94 checks all targets. An isolated archive of the exact upstream revision
passes 1,185 library tests with seven existing ignores. The locked dependency
notice collector reports no missing texts. Raw inputs, binaries, hashes and
command logs remain local under `runs/syom-upstream-0100/`; the incomplete initial
debug long-file trial is excluded from the comparison counts above.

No sibling checkout is modified, hosted CI is not dispatched, and no crate or
binary release is published by this migration.
