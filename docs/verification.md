# README verification and remaining gaps

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

[Project overview](../README.md) · [Documentation](README.md)

Current source is public as agent-youtube; the executable remains agent-youtube. The MIT
license is now present at the root, CI is dispatch-only and hosted Actions are
disabled. Crates.io publication stays disabled. Sections below preserve dated
reviews, including superseded privacy/license findings; see current readiness.

## Current README presentation — 0.8.2, 2026-10-30

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
| Source 0.8.2 | Cargo.toml package version; crates.io publication is disabled |
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
