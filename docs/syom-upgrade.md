# syom refresh qualification - source 0.15.0

Verified 2026-11-02 on Linux x86_64. The selected runtime is syom **0.9.0 source
candidate**, archived from clean upstream revision
`60199081659c9fe8ee9bdf7bc07f304d5bf3c8b6`. Its changelog marks it unreleased.
No registry publication, new YouTube availability result or ASR accuracy
improvement is claimed by adopting it.

## Source and local extensions

Only syom changes. bandlimit 0.1.7 and registry ryf 0.7.3 remain unchanged.
Cargo.lock changes the application and syom versions without adding a package.
The codec has no runtime dependencies. Builds need no sibling checkout.

The fork retains strict indexed-presentation completion with bounded movie-clock
rounding and the qualified LC codec-state checkpoint API. The upstream exact seek
API saves replay-based logical positions, not serialized codec history. Our
extension refuses checkpointing exact-mode or failed state instead of bypassing
the upstream contract. Other upstream Rust files remain byte-identical to the
archived revision. Test media/corpora are not bundled.

[Per-file provenance](../vendor/syom/upstream.json) records original/bundled hashes,
the revision and patch digest. [The current patch](../tools/upstream-codecs/syom-0.9.0.patch)
and [vendor record](../vendor/README.md) describe the retained delta. The sibling
repository was read only; no upstream change was committed or published.

## Complete decode and restart comparisons

The same bytes were processed by the previous 0.14.0 backend and the new backend.
Seven synthetic stereo AAC-LC sources cover 8/16/22.05/24/32/44.1/48 kHz with
fractional 16 kHz output tails. Additional inputs are ten minutes of synthetic
audio, a retained real 52.96475-second recording and a retained real
3:07:51.0124375 recording.

| Scope | Inputs | Equal complete frame counts | Equal full PCM across backends | Same-backend process-restart PCM equality |
| --- | ---: | ---: | ---: | ---: |
| Synthetic rates | 7 | 7 | 7 | 14 / 14 |
| Synthetic ten minutes | 1 | 1 | 1 | 2 / 2 |
| Real recordings | 2 | 2 | 2 | 4 / 4 |

Each restart trial stops after a durably saved one-second decoder checkpoint and
continues in another process. All twenty same-backend comparisons match the full
uninterrupted PCM16 hash. [Sanitized numeric evidence](syom-upgrade-results.json)
records input/output hashes, frames and peak RSS. The multi-hour source decoded
to 180,336,199 frames; its current full-decode test process peaked near 19.85 MiB.

Nine inputs use debug tests, the multi-hour input uses optimized library tests
in both builds. Initial slow debug multi-hour trials were stopped; their partial
output is not counted as successful qualification. No cross-profile speed ranking
is made. These inputs do not establish universal cross-version PCM equivalence.

The retained real 59,636,979-byte compressed prefix was decoded with the new
backend for 3600.123..3660.123 seconds. All selected samples match its full decode:
SHA-256 `38dee12f7cb7ebc02bd10dba90d4d586d50fd6722859c4751f4c5e5893a75238`.
Source length remains an explicit index estimate on this partial path. Regular
pipeline tests also exercise indexed SABR acquisition, staging/fallback, corrupt
prefix refusal and separate-process resume without HTTP.

## Seek behavior and limits

The old PNS counterexample was made an equality regression and failed on 0.7.1.
With the new source it passes at 16/44.1 kHz, PNS on/off, for native float samples
and selected 16 kHz PCM. Cold seeks replay unverified history; a short overlap
value does not mean preceding bytes are skipped.

The new exact plan reports PrefixReplay for a nonempty late interval. Downstream
tests verify that planning performs no media I/O, insufficient AU budgets are
refused before I/O or position change, and missing first-required payload bytes
fail rather than produce approximation. Failed exact state cannot become a
reusable local checkpoint. The resampler still needs its global phase. Production
keeps prefix/full acquisition; independent interval downloading is not enabled.

## Job migration and preserved transcripts

The media-backend fingerprint changes from v2 to a revision-qualified v3. A job
with another fingerprint is refused before recovery/mutation, including a
completed job submitted again through transcribe/resume. Use its original
executable/runtime for resume or start a new work directory. Do not edit saved
backend identity to force compatibility, even when a small comparison corpus matches.

The old-build/new-build refusal trial preserved every snapshotted job file, and
the original build could subsequently finish it. Two completed real schema3
transcripts exported with the new CLI; their entire original job directories,
including raw ASR JSON, remained byte-identical. Transcript, coverage and response
schemas are unchanged. No real speech was re-recognized.

## Reproduction and verification scope

The ignored audio::qualification::migration_probe supports generate/full/interrupt/
resume modes with AGENT_YOUTUBE_CODEC_PROBE, AGENT_YOUTUBE_CODEC_MODE and
AGENT_YOUTUBE_CODEC_SOURCE. Use the same generated sources in both builds, retain
their test executables before switching source and compare resulting reports.
The real_prefix_update_qualification test accepts a fresh owned probe directory,
full source, full same-backend PCM reference, prefix byte count and requested
millisecond bounds. It streams the prefix and selected PCM comparison. Original
media, executable hashes, detailed commands and reports stay local.

The matching patched upstream library suite passed 1,167 tests with seven ignored;
two additional checkpoint/completion regressions passed. Application gates and
minimum-version results are recorded in the verification report and local task.
Other platforms and every new upstream feature were not independently qualified.
No distribution binary or hosted CI was produced.

Final application checks passed: 435 Rust tests with 24 opt-in tests ignored,
75 Python checks, formatting, strict all-target Clippy and 29 documentation link
targets. Rust 1.94.0 compiled every application test target and passed 18 audio
tests, with five opt-in audio tests ignored. Real-source tests and the cross-build
refusal trial ran separately. Dependency notice collection found no missing text.
