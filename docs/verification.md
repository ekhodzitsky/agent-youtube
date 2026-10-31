# README verification and remaining gaps

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

[Project overview](../README.md) · [Documentation](README.md)

Current source is public as agent-youtube; the executable remains agent-youtube. The MIT
license is now present at the root, CI is dispatch-only and hosted Actions are
disabled. Crates.io publication stays disabled. Sections below preserve dated
reviews, including superseded privacy/license findings; see current readiness.

## Partial acquisition qualification — 0.14.0, 2026-11-01

Two real YouTube recordings were downloaded completely as references, then
independently acquired through the ranged production CLI. Both partial sources
matched the corresponding bytes of their complete reference. Every selected PCM
byte matched a separate full streaming decode, before recognition.

| Recording | Requested interval (seconds) | Complete media bytes | Actual partial media bytes | Reduction |
| --- | --- | --- | --- | --- |
| 3:07:51.0124375 | 3600.123–3660.123 | 182,409,911 | 59,636,979 | 67.31% |
| 52.96475 | 1.001–12.501 | 858,695 | 647,586 | 24.59% |

These are committed compressed-media bytes, excluding HTTP/UMP overhead and
retries. Server responses may include fragments beyond the minimum requested
prefix. Each trial was normally cancelled after durable prefix qualification;
elapsed times to cancellation were 472.8 and 66.1 seconds respectively, including
acquisition, qualification and CLI setup. These times are observations, not a
matched network throughput comparison. On the saved long reference, full
streaming decoding took 64.5 seconds and prefix decoding with durable selected
chunks took 21.0 seconds; different persistence work limits that comparison.

Both jobs subsequently resumed with actual GigaSTT 2.22.0 and the multilingual
CTC model, while strace forced every attempted socket creation to fail with
ENETDOWN. Neither process tree attempted a network connection; local IPC remained
available. The jobs published six and two original ASR JSON files respectively.
Transcript and raw-artifact hashes validated, all eight result/manifest/transcript/
coverage documents passed the JSON contract, and owned media was removed only
after publication. ASR accuracy was not measured by this check.

The optimization requires an explicit end and a complete direct-reference SABR
SIDX with supported codec/timeline properties. It acquires and decodes from zero,
preserving codec and resampler history; it does not seek directly to the selected
start. Full acquisition or decoding remains the fallback for unsupported inputs.
Canonical and coverage schema3 report the prefix extent and indexed source
duration explicitly. They do not attest decoding the unused suffix.

Local regressions exercise 403/429/503 recovery, server waiting intervals,
corrupt retained prefixes, separate-process resume, rejected old-binary downgrade,
unsupported codecs, insufficient candidate PCM, and replacement sources without
adopting candidate chunks or decoder state. Real trials establish these two
observations in the current network, not universal YouTube availability. Original
media, transcripts and trial records remain private local evidence.

Final local gates passed: 432 Rust tests with 22 opt-in tests ignored, formatting,
strict Clippy, 74 Python checks and 28 documentation link targets. The real-source
checks above ran separately. The previously observed intermittent ASR notification
count mismatch remains outside this change; a passing run does not fix that issue.

## Selected PCM storage qualification — 0.13.1, 2026-10-31

An existing complete 182,409,911-byte recording decoded to 180,336,199 mono
16 kHz frames (3:07:51.0124375). For 3600.123–3660.123 seconds, all selected
PCM bytes matched a separate full streaming decode. Seven intersecting fixed-grid
WAVs occupied 2,240,308 bytes, compared with 360,672,398 bytes of whole-recording
PCM, excluding WAV headers. This reduces WAV storage by 99.38%; it is not a
whole-work-directory reduction. Decoder checkpoints occupied another 41,292,867
bytes, and compressed source storage remained unchanged.

The comparison reused a local recording; no fresh network acquisition or ASR
was performed. Both paths decoded the full presentation. The streaming reference
discarded unselected PCM and took 80.9 seconds; range retention with durable
checkpoints took 104.0 seconds. These are observed local times with different
persistence work, not evidence of faster decoding. The opt-in
real_long_range_retention_profile test repeats the comparison when
AGENT_YOUTUBE_RANGE_PROFILE_SOURCE names a local recording longer than the
selected interval. Output remains local.

Regression checks exercise a real checkpoint/journal reopen after interruption,
exact PCM, omitted prefix WAVs, refusal of a missing retained WAV, zero remaining
PCM allocation, half-open chunk boundaries, cross-grid cuts, open-ended ranges,
out-of-bounds starts and incomplete presentation rejection before recognition.
Network download and full decoding remain unchanged; partial network acquisition
is not implemented by this storage optimization.

Final local gates passed: 423 Rust tests with 19 opt-in tests ignored, formatting,
strict Clippy, 74 Python checks and 28 documentation link targets. The long-source
profile above passed separately. An earlier run observed an intermittent existing
ASR notification-test count mismatch; that unrelated test remains unchanged,
and its subsequent pass does not establish that the intermittent failure is fixed.

## Machine discovery qualification — 0.13.0, 2026-10-31

Local gates pass: 421 Rust tests, 18 opt-in ignored; strict Clippy/formatting,
74 Python checks and 28 documentation link targets. Discovery returns all 12
public command/diagnostic descriptions or one selected description. Generated
negative CLI requests exercise over 50 enum, numeric, timestamp and missing-field
cases without creating work files. Formal schemas and incompatible runtime/range
arguments are tested, including response version references and global help.
A real loopback pipeline's saved options match the advertised fixed/default-policy
values and its response version matches the discovered default mode.

A retained 0.13.0 development executable was run with bubblewrap network isolation,
unset HOME, empty tool PATH and no mounted models or user configuration. Host /usr/lib and /lib64 were shared (glibc 2.43); PATH and /usr/bin were empty,
and no toolchain or recognizer was configured in that environment. Describe succeeded with zero working files
created and zero stderr bytes. Its result was byte-identical to the same executable
run in the ordinary environment. Embedded response and per-command input schemas
passed local Draft 2020-12 checks. The embedded contract is included in production
source fingerprinting so schema changes cannot retain the same source fingerprint.

This verifies static discovery and CLI conformance, not model readiness, inference,
YouTube availability or a new portability/security claim. No binary release or
hosted workflow was triggered; the published binary remains 0.10.1.

## Recognition range qualification — 0.12.0, 2026-10-31

Local gates pass: 417 Rust tests, 18 opt-in ignored; strict Clippy and formatting,
74 Python checks and 28 documentation link targets. Regressions cover strict
millisecond parsing, overflow, invalid/empty bounds, cross-grid PCM hashes, clamping,
pause-adjusted cuts, absolute JSON/MD/SRT/VTT timestamps, saved-option resume,
immutable finished selection, batch range metadata and legacy whole-video formats.
Ranged jobs use checkpoint5, canonical2 and coverage2; whole-video jobs keep their
previous formats. A failing-before/passing-after checkpoint-version assertion
prevents older executables from accepting new ASR span semantics.

A retained schema5 development build, before commit, downloaded the complete
jNQXAC9IVRw audio: 309288 bytes, decoded to 305018 mono 16 kHz frames (19.063625 s).
Only frames 80000..240000, original seconds 5..15, reached GigaSTT 2.22.0 / ml_ctc.
The job was interrupted at the recognition stage, retaining media and checkpoint5. The
retained 0.11.0 executable refused it as unsupported checkpoint schema; hashes of
state, invocation, download receipt, source metadata and source audio stayed equal.
The new executable resumed real recognition, published a 10-second raw ASR result,
and retained absolute timestamps and range-scoped coverage. Exact raw/canonical
hashes, selected input bounds and cleanup were checked; 27 emitted/artifact records
matched the machine schema. A later development build adding resolved range to
batch item metadata revalidated/reused this job without altering ASR/transcript
bytes; its recovery response also matched the schema.

This verifies one short real-video range, not recognition accuracy or a measured
network reliability rate. Full acquisition and decoding still occur. Binary
release remains 0.10.1; these features require a source build. No binary release or
hosted CI was triggered for the feature.

## JSONL batch qualification — 0.11.0, 2026-10-31

Local gates pass: 408 Rust tests, 18 opt-in ignored; strict Clippy, formatting,
74 Python qualification checks and 28 documentation link targets. Loopback CLI
fixtures exercise partial failures, immutable membership/options, duplicate and
bounded input, safe ID paths, root/result symlinks, retained locks, cancellation,
process restart and validated reuse. The fake recognizer uses a nonblocking OS
lock to fail if two recognition processes overlap. Cancelling one item leaves its
sibling running; Ctrl+C stops the batch and a new process completes saved jobs.
Per-job download state and Web worker lifetime are isolated; HTTP pooling and the
persistent per-user server cooldown remain shared.

A retained 0.11.0 development executable (dirty working tree before commit) with
source fingerprint c9fd7c643d201412ddd9e01ce9794f18b6189011 and SHA-256
7e4b8ea8b791f5a9ea839556cf9b9dd9fc36b9dfd28f413adc9fbad2429bc70f
processed a three-item batch with two workers and GigaSTT 2.22.0 / ml_ctc:

| Item | Video ID | Decoded duration | Contiguous 16 kHz frames | Result |
| --- | --- | ---: | ---: | --- |
| English | jNQXAC9IVRw | 19.063625 s | 305018 | Completed |
| Russian | of4B3aw7MXo | 52.964750 s | 847436 | Completed |
| Invalid URL | No YouTube ID | Not downloaded | None | batch_url_invalid, nonretryable |

Both valid jobs verified the complete download, decoded audio, raw ASR hashes and
transcript hash, then cleaned owned media. Repeating the same command reused both
validated transcripts with no metadata/download/decode/recognition events. Both
invocations returned exit 1 for the invalid item, with ordered results and counts;
45 emitted JSON records and both reports matched the schema. These are functional
checks on two short videos in the current network, not word accuracy, performance
or general availability measurements. The published binary remains 0.10.1;
0.11.0 batch support currently requires a source build.

## Automatic setup qualification — 0.10.0, 2026-10-30

Normal setup now provisions the pinned runtime and selected model automatically
inside its owned root. Explicit --model-dir keeps local model provisioning;
--offline prohibits requests. Successful JSON reports state=ready,
next_action=transcribe and download_bundle_bytes (pinned files, not measured
traffic). Existing completed manifests remain immutable.

The empty-root network test selected ml_ctc, with 246,178,931 pinned model/archive
bytes. The setup process was SIGKILLed after its receipt committed 1,061,452 model
bytes. A new process resumed from the verified prefix, completed the model and
runtime downloads, verified their compiled SHA-256 values and published the
manifest. Offline reuse and doctor then passed. The new installation acquired
and transcribed jNQXAC9IVRw with real GigaSTT: 19.063625 seconds, 305,018 contiguous
input frames, validated transcript and post-publication cleanup. Continued setup
took 214.854 seconds on that host/network; this is one interrupted installation,
not a throughput claim or a platform-wide success rate.

Local regressions cover truncated-response continuation in a new process,
correct ranges, full-response prefix proof when Range is ignored, wrong ranges,
changed/foreign/linked partials, final hash failures, refused ready-file overwrite,
uncommitted-tail rollback, persistent numeric/unparseable cooldowns, offline
missing files and offline publication of a complete verified cache. Catalog names
are checked against the recognition adapter's required files. Extraction and
certificate/URL validation regressions remain active. No runtime dependency added.

Reproduce the main path from a new owned directory:

```sh
agent-youtube setup --runtime-dir runs/new-install/runtime --model-variant ml_ctc
agent-youtube doctor --runtime-dir runs/new-install/runtime
agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --runtime-dir runs/new-install/runtime --work-dir runs/new-install/job
```

The qualification used a development worktree build, not a post-tag clean build.
Its executable and original JSON/ASR evidence stay private. The final source also
has isolated regression coverage for fully downloaded cache publication in offline
mode and local I/O error classification. Historical measurements below retain
exact versions; new setup does not rewrite previous transcripts or raw ASR.

## HTTP body timeout qualification — 0.9.1, 2026-10-30

The preceding live http_transport failures were reproduced with typed diagnostics:
a response read timed out after receiving 698,458 bytes. Ureq's timeout_recv_body
is a total response-body budget, not an inactivity timeout; the previous five
seconds could expire while bytes kept arriving. The implementation now allows
30 seconds for a bounded body, retaining the 45-second overall per-call budget.
No provider, cipher policy, certificate validation, URL scope, byte limit, retry
count, source identity or cooldown rule was relaxed.

A loopback server sends one byte per second for six seconds. The old configuration
fails with reason=timeout after five received bytes; the fixed configuration reads
the complete six-byte body. Other regressions verify truncated-body phase/byte
counts and sanitize typed timeout errors without copying messages or URLs.
http_transport from bounded body reads includes transport_phase=response_body,
reason from a fixed allowlist and body_bytes_received. These are bytes from the
failed attempt, not durable acquisition progress. No response contents are logged.

The reference was yt-dlp 2026.10.26, published 2026-10-26, official Python zipapp
SHA-256 e51e69e89489f199530f13d32a4342da3ddb2cf34842f364603a066db05afbf3,
checked against the GitHub asset digest. With ignored user config, no cache,
bestaudio[ext=m4a] and default clients (no PO-provider plugin), both full downloads
succeeded on this host/network. FFmpeg as a research oracle decoded both complete
files without errors: 19.0635625 seconds and 52.9879375 seconds. Native and reference
may select different valid AAC/MP4 representations; differing container hashes
are not treated as proof of corruption or as a speed comparison.

The failed native English job retained its receipt/owned data and was resumed with
the fix. It downloaded all 309,288 bytes, decoded 19.063625 seconds, completed real
GigaSTT ml_ctc recognition and published a validated transcript with full contiguous
input coverage. Cleanup completed after publication. This observation is a fresh
network acquisition from the retained failed source, not the cached-recording
loopback qualification in 0.9.0. A fresh Russian job acquired all 858,695 bytes,
decoded 52.96475 seconds and completed GigaSTT ml_ctc_large recognition with full
contiguous sample coverage and post-publication cleanup. Native and reference
files have different hashes and slightly different decoded durations; exact
compressed-packet/PCM equivalence was not checked. No identical-byte or matched
speed claim is made. Both native runs used development
binary SHA-256 c5ed3acb49006b6fbd99af9a715d63096c8e79754ea784e5dd9df2b3cc9a8e0d,
source fingerprint 67db1462845e3dc81ce2daed61c3c3f559d611ef, based on 1dcde85 with
source_dirty=true. These observations establish their exact runs, not universal
access. Local fmt, strict all-target Clippy, 386 Rust tests (18 opt-in ignored),
72 Python tests and the 28-document check passed. Hosted Actions remained disabled.

## Multilingual integration qualification — 0.9.0, 2026-10-30

GigaSTT stays pinned to 2.22.0. Setup, doctor and transcribe now select e2e_rnnt,
ml_ctc or ml_ctc_large. Runtime manifests and checkpoints bind the model; old
missing fields default to e2e_rnnt. A changed model is rejected before recovery,
including completed jobs. Prepared-runtime selection is inherited when omitted;
an explicit conflicting variant is rejected. A Rust Options literal must add
model_variant, which is why the source version increments to 0.9.0.

The model provisioning helper checks the multilingual file hashes from the pinned
upstream source. It does not download models. JSON reports model scope as mul or
ru and source_language as null, without claiming language detection.

| Complete recording | Selected model | Decoded audio | Interruption / new-process resume |
| --- | --- | ---: | --- |
| jNQXAC9IVRw, English | ml_ctc | 19.063625 s | Validated; first raw ASR result preserved |
| of4B3aw7MXo, Russian | ml_ctc_large | 52.9879375 s | Validated; first raw ASR result preserved |

These are complete previously retained YouTube recordings served by the loopback
fixture, with the real pinned GigaSTT executable and model files. Cancellation
was requested after the first valid batch result; the production CLI resumed in
a new process, preserved that exact result, published a validated transcript and
cleaned owned media. They establish integration and recovery for those recordings,
not recognition accuracy or a fresh YouTube availability rate. Both separate live
acquisition attempts on the current network failed with retryable http_transport
after their internal retries; their job data remains private for resume.

Reproduction: opt-in research_multilingual_recordings_resume_with_real_gigastt
in tests/pipeline.rs, with AGENT_YOUTUBE_MULTILINGUAL_PROBE pointing to a new owned
runs subdirectory, AGENT_YOUTUBE_MULTILINGUAL_MODELS/ENGINE selecting the models
and pinned executable, and AGENT_YOUTUBE_MULTILINGUAL_ENGLISH/RUSSIAN selecting
complete recordings for the two video IDs above. Use cargo test --locked --test
pipeline research_multilingual_recordings_resume_with_real_gigastt -- --exact
--ignored --nocapture. No raw recording or transcript belongs in Git.

## Historical README presentation — 0.8.2, 2026-10-30

The root now follows ryf's concise input/output introduction and example-first
structure. The first screen identifies YouTube transcripts for AI agents, local
recognition, and the current adapter's model restriction. Summaries and reasoning
are explicitly downstream agent work. English prose and the four existing badges
are retained. Examples use the current transcribe/export/status/resume/chapters
commands; the displayed chapter entry is an exact excerpt of the retained live
result, not a fabricated transcript.

GigaSTT supports multilingual models (Russian, English, Kazakh, Kyrgyz, Uzbek).
The adapter hardcodes e2e_rnnt and language ru; the root and installation guide
now identify this as an integration limitation. Upstream model documentation
and the three comparison source READMEs were rechecked on 2026-10-30.

Purpose, hook, alternatives and constraints pass editorial/source review.
The chapter quick start remains the complete model-free first-run path; the main
transcription example links to the full model/runtime provisioning sequence.
Commands were checked against current CLI help and source, not rerun as a fresh
install or live ASR session. Markdown was rendered for structural review and
links checked locally; no reader study establishes a measured five-second
comprehension result. Historical recovery results and missing coverage/current
benchmarks remain unchanged; benchmark and badge acceptance is still partial.

## Historical README v1 acceptance — 0.8.1, 2026-10-30

The current root adapts agent-dev-kb docs/readme-v1/{README,template}.md at
revision d92256be2cb2c92c686bd5c1de2341a61ba1a0cb. The opening describes only
implemented metadata/transcription operations; the broader project name does not
promise channel, playlist, caption or search commands. Detailed setup, retry,
export, migration and licensing guidance remain in existing guides.

| V1 check | Result | Evidence / boundary |
| --- | --- | --- |
| Purpose | Pass | English opening identifies JSON CLI, author timestamps, local Russian recognition and recovery |
| Hook | Pass | Complete metadata command and observed 35-entry result; early first useful operation |
| First run | Pass for command behavior | Current 0.8.0 local Rust CLI fetched rfscVS0vtbw and returned 35 timestamps, duration 16012 seconds, first Introduction at 0 seconds; source install command reviewed, not rerun on a clean host |
| Alternatives | Pass for documented scope | Three primary upstream READMEs reopened on 2026-10-30; text-source/model choices, no measured superiority claim |
| Benchmarks | Partial for current-release scope | Named historical 0.3.10 / GigaSTT 2.21.0 completion/recovery observations retained with exact units and versions; no fresh 0.8.0 ASR, comparative speed or representative success-rate measurement |
| Badges | Partial | Accurate source/MIT/declared source-build MSRV and disabled CI; line/branch coverage unmeasured, hosted MSRV qualification disabled |
| Constraints | Pass | Access/TLS maturity, Russian ASR, description provenance, absent captions/playlists/live support, ownership migration and resource limits explicit |
| Presentation | Pass for structural review | Concise root, at most three navigation links, narrow tables, locally rendered Markdown and parsed SVG badges; no browser pixel-review claim |

The metadata result is one source observation, not a new availability guarantee.
Watch-page request handling, absence, malformed lists/metadata, video identity,
HTTP403/429, retry guidance and closed response schema are exercised offline.
No new audio or ASR run is needed to verify the chapter-only quick start. A later
0.8.0 request failed with retryable http_transport while reading the watch page;
this is retained as an observed failure, not omitted from availability evidence.
0.8.1 corrects the inherited media-retention wording for metadata errors and
preserves error codes, retryability and server guidance; it adds no access guarantee.
Coverage and missing current-release recognition benchmarks remain measurement
work; polished documentation does not establish unrestricted v1 evidence coverage.

## Badge sources

The root badges describe the main source checkout, not an uploaded binary release
or a hosted test run. SVG labels make no third-party requests.

| Badge | Source and meaning |
| --- | --- |
| Source 0.13.0 | Cargo.toml package version; crates.io publication is disabled |
| License MIT | Root LICENSE; dependencies and model terms remain separate |
| CI disabled | GitHub Actions disabled by owner instruction; workflow remains dispatch-only |
| Minimum Rust 1.94 for source builds | Manifest rust-version; prior local MSRV qualification, not a passing hosted 0.8.0 claim |

Downloads badge: inapplicable to this distribution state. There is no crates.io
package or uploaded downloadable release artifact with a meaningful distribution
count; public Git source/tag access is not a user/install count. docs.rs is not
used. Coverage badge: applicable but omitted because no measured line/branch report
exists. Neither a fabricated number nor a decorative unknown badge fills that gap.

## Historical verification records

Baseline: agent-youtube 0.3.3, source commit `2afc6e3`, reviewed 2026-10-27 against
root README standard v1. This documentation change does not publish a release,
change recognition, or certify new live access.

## Historical badge sources

Badges are local SVG status labels, not live dashboards. They make no third-party
request. Update them with their sources when state changes.

| Badge | Source and meaning |
|---|---|
| Source 0.3.3 | [Cargo.toml](../Cargo.toml), not a published-release claim |
| Downloads: private / N/A | `publish = false`; no applicable public package metric |
| CI: manual only | [Workflow](../.github/workflows/check.yml) uses `workflow_dispatch`; no passing hosted run implied |
| Coverage: not measured | No current line/branch coverage report found; test success and ASR input coverage are different metrics |
| Rust 1.94+ | Manifest `rust-version`; local locked all-target check and manual workflow job |
| License: MIT declared | Manifest declaration, subject to the qualification below |

Coverage records a missing measurement, not compliance with the standard's
coverage requirement. Public downloads and docs.rs publication do not apply to
this strictly private CLI. No fake percentage or static passing-build label is used.

## Licensing

Cargo declares MIT, but no root LICENSE file was present at review. This edit
does not invent a copyright holder, grant a license, or authorize distribution.
The repository and crate must remain private. Vendored components retain their
notices under [vendor](../vendor/README.md) and `experiments/botguard/vendor/`;
external model terms also apply. The badge is a manifest declaration, not proof
that licensing documentation is complete.

## Evidence map

| Claim | Evidence | Scope |
|---|---|---|
| JSON CLI and exports | CLI help, contract tests, [schema](../schemas/contract.schema.json) | Local behavior, no new live acquisition |
| Recovery and cleanup | [Recovery guide](storage-and-recovery.md), pipeline tests | Qualified paths, not unconditional success |
| Russian local ASR | [Installation](installation.md), pipeline configuration | Requires GigaSTT and models |
| Alternatives | [Sources and contracts](comparison.md) | Capabilities, no overall winner |
| Benchmark figures | Local original JSON, [numerical extract](benchmark-results.json) | Historical 2026-10-24 samples |
| Security constraints | [Development notes](development.md) | Existing unresolved findings retained |

## Checks for this edit

Verified locally on 2026-10-27:

- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test` passed: 303 tests, zero failures, 11 opt-in tests ignored.
- `cargo +1.94.0 check --locked --all-targets` and `cargo build --locked` passed.
- The built CLI returned successful JSON for help, version, and doctor.
  Doctor used the installed GigaSTT 2.21.0, not the recommended 2.22.0;
  it checked readable model files and flags, not inference or live access.
- SRT export from an existing canonical transcript produced timestamped output
  and JSON success in a fresh temporary directory. A second export refused
  overwrite. The source transcript's SHA-256 was unchanged.
- All seven former root sections remain in the guides, with only relative-link
  relocation and introductory context added. Local Markdown link targets exist.
- Markdown renders to HTML with two root tables and six badge images; badge
  SVGs parse correctly. This is a structural render check, not browser pixel QA.
- Root prose/tables contain approximately 470 words excluding code and link
  destinations. Benchmark aggregates match retained original numerical data.

No hosted CI, live YouTube acquisition, model download, or new inference was
performed. Quick-start network success depends on source availability and assets.

## Remaining standard v1 gaps

- Test coverage is not measured; no percentage can be supplied honestly.
- No root license text accompanies the manifest declaration.
- Historical benchmarks do not measure the recommended GigaSTT 2.22.0 setup
  or rank recognizers on a matched corpus; their limits are explicit.
- Historical raw evidence is local-only. Numerical results and the reproduction
  method are portable, but a clean clone has no original run artifacts.

Local Backlog data and `.grok/config.toml` were removed from tracking while
retaining files on disk. The tracked documentation is self-contained; local
historical evidence is not included in a clean checkout.

## Audit remediation qualification — 2026-10-28

Version 0.3.8 adds regression coverage for deadline/publication reconciliation,
96 MiB transcript input under a 256 MiB process limit, and private child-error
handling. A live HTTPS fetch from YouTube verified the constrained provider's
TLS path; it is not a full-audio availability trial.

The isolated vendor library suites passed: syom 1121 (7 ignored), bandlimit 41
(1 ignored), ryf 202 (2 ignored). syom uses the pinned test revision documented
in vendor/README.md. That expanded suite exposed six M4A rounding failures,
now covered by an independent synthetic fixture and strict negative cases.
Checkpoint resume and replay produce identical PCM for the rounded LC case.
These suites do not establish universal codec coverage or ASR completeness.

The lockfile shrank from 263 to 258 packages. The obsolete webpki and unmaintained
paste findings are removed. Two upstream advisories remain (rsa and crypto-bigint)
with affected private-key paths disabled; raw cargo audit still reports them.
There are no blanket advisory exclusions. See development.md for the boundary.

## README v1 refresh — 2026-10-29

Applied agent-dev-kb `docs/readme-v1/{README,template}.md` at revision
`d92256be2cb2c92c686bd5c1de2341a61ba1a0cb` to agent-youtube 0.3.12. Earlier sections
above remain historical reviews, not current badge claims.

| V1 check | Result | Evidence / remaining qualification |
| --- | --- | --- |
| Purpose | Pass | Opening defines the JSON agent workflow and local GigaSTT boundary |
| Hook | Pass | Early recovery table supports the checkpoint/publication promise |
| First run | Pass | Isolated locked install, doctor, fresh full short-video transcription, SRT export and status all exited 0 with JSON success on 2026-10-29 |
| Alternatives | Pass | Caption retrieval and yt-dlp + Whisper checked against primary READMEs on 2026-10-29; no superiority claim |
| Benchmarks | Pass for stated scope | Exact decoded durations, completion denominators and recovery outcomes from two historical observations; no throughput ranking or representative success probability |
| Badges | Partial | Source version and local MSRV are backed by files/checks; CI is manual-only. Coverage and actual root license text remain missing |
| Constraints | Pass | Access failures, scope, timestamp estimates, local dependencies and private status are explicit |
| Presentation | Pass for structural review | English Markdown rendered locally; headings, tables, code blocks, links and badge SVGs checked. No browser pixel review claimed |

Downloads and published-release badges are inapplicable to this private,
unpublished project and are omitted. No docs.rs badge applies to this private CLI.
The source-version badge describes the checkout. Unmeasured coverage and the
manifest-only MIT declaration are documented as missing work, not represented by
fabricated measurement/license badges. MSRV was checked locally; hosted CI remains
disabled by owner instruction. No copyright/license grant was invented. Full
standard compliance remains **partial** until the applicable evidence gaps close.

The tested recognizer was installed GigaSTT 2.21.0 with the existing INT8 e2e_rnnt
models. The README names this verified setup rather than implying that source
review of 2.22.0 qualifies its binary. New quick-start run evidence is local-only
under `runs/readme-v1-20261029/`; original timestamps and ASR JSON are preserved.
The historical numerical recovery extract is [retained separately](recovery-results.json).

## TLS scope update — 0.5.0

[The current comparison](tls-provider-scope.md) verifies actual TLS 1.2/1.3
handshakes, GitHub setup and full audio recovery before selecting pinned modern
RustCrypto. Historical audit counts above are not current lockfile claims. The
RSA advisory and upstream experimental warning remain; no security waiver is added.
