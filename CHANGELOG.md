# Changelog

This project uses SemVer for the Rust package and CLI. During 0.x,
incompatible changes increment the minor version; compatible fixes increment the
patch version. JSON and checkpoint schemas have independent versions. Entries
state migration requirements; a package upgrade alone does not change exact ASR
JSON or canonical transcripts. Source is public; crates.io publication stays disabled.

## Unreleased

## 0.15.0 — 2026-11-02

- Refresh the syom runtime from clean upstream source candidate 0.9.0 at
  60199081659c9fe8ee9bdf7bc07f304d5bf3c8b6. This is an unreleased source
  candidate, not a claimed registry release. Retain only the local exact-state
  checkpoint and strict presentation-completion extensions; add no runtime crate.
- Preserve upstream PNS-correct cold seeks, explicit prefix-replay planning and
  bounded cold Huffman initialization. Qualify PNS seek PCM equality and refuse
  missing history or insufficient replay budgets. Direct interval acquisition
  remains disabled: the exact plan still requires the prefix.
- Change the media-backend identity to v3. Old jobs, including v2, require their
  original implementation for resume or a new work directory; refusal preserves
  checkpoints, media and raw ASR. Saved transcript formats are unchanged.
- Retain the earlier PNS/phase qualification as dated evidence, with current
  behavior documented separately. Keep historical codec migration tooling from
  silently running its old dependency replacements against the new production pin.

## 0.14.0 — 2026-11-01

- Stop qualified AAC-LC range decoding after the complete fixed-grid chunk
  covering an explicit end. Replay from the beginning preserves exact codec and
  resampler state. Unsupported codec state and ranges reaching EOF
  retain full decoding. Decoding starts at zero; it does not skip audio before
  the requested start.
- Stop qualified SABR acquisition using a complete direct-reference SIDX whose
  byte extents cover the full representation. Validate the selected PCM before
  publishing prefix source evidence and adopting staged WAVs. Keep full acquisition
  for unsupported cases, preserve failed candidates and verify cached prefixes
  without URLs or tokens. The public Rust Downloader.download API stays full-only.
- Version prefix canonical/coverage artifacts as schema3. Their range.decoded_prefix
  records through_frame and source_frames_basis=container_index; duration and
  source_frames are indexed estimates, not proof of suffix decoding. Partial
  acquisition adds range.downloaded_prefix; agent/batch summaries report
  download=prefix, downloaded_bytes/source_bytes, decode=prefix and decoded extent. Response envelopes,
  whole-video schema1 and full-decoded ranged schema2 remain unchanged.
- Use ranged checkpoint schema6 to block unsafe downgrade; read schema5 and
  migrate active jobs on mutation. Keep old exact transcripts and ASR JSON intact.
  Prefix decode receipts use internal schema2. Rust SelectedRange literals now
  need decoded_prefix: None and downloaded_prefix: None for full processing;
  old serialized ranges default to no prefix. No new dependency or binary release.

## 0.13.1 — 2026-10-31

- Retain decoded WAVs only for fixed-grid chunks intersecting --start/--end.
  Keep complete audio acquisition, full decoding and duration verification;
  network traffic and decoding CPU are unchanged. Exact GigaSTT input and
  original timestamps are preserved, including cuts inside decoder chunks.
- Budget pending PCM disk space for the selected chunks and resume exact decoder
  state without requiring intentionally omitted WAVs. Missing retained chunks
  still fail verification. Existing owned media stays until validated transcript
  publication. No dependency, JSON/checkpoint format or binary release change.

## 0.13.0 — 2026-10-31

- Add read-only describe and describe COMMAND with typed logical CLI input schemas,
  argument mappings, required parameters, enums/bounds, fixed/default-resolution
  policies, constraints, response variants and field-based recovery guidance.
  Embed the existing response contract and only the referenced schema definitions;
  no models, configuration inspection or network are required.
- Make the parser validate command/option specifications before runtime discovery
  or job creation. Shared definitions drive numeric defaults, response modes,
  diagnostic aliases, model/export choices and domain bounds. Invalid arguments
  may now fail earlier; existing accepted invocation forms are retained.
- Include the embedded contract in production source fingerprinting. Discovery
  describes interface support, not installed runtime/model health or YouTube
  access. It does not introduce JSON-input execution. No new dependency, binary
  release, transcript or checkpoint format change.

## 0.12.0 — 2026-10-31

- Add --start/--end to transcribe and batch. Accept seconds or HH:MM:SS with
  millisecond precision; keep timestamps on the original video timeline. Download
  and validate the complete audio, then send only selected PCM to GigaSTT.
- Persist requested ranges in job/checkpoint identity. Resume reuses saved bounds;
  changed bounds require a new work directory even after successful publication.
  Ranged checkpoints use schema5 so older binaries reject their ASR span semantics;
  whole-video checkpoints keep schema4.
  Clamp the end to actual decoded audio; reject empty or out-of-bounds selections
  without deleting owned media. Pause-adjusted cuts remain inside the selection.
- Ranged canonical transcripts and coverage artifacts use schema2 with explicit
  requested and actual frame bounds; whole-video files retain schema1 unchanged.
  Compact responses keep their schema2 envelope and add human-readable range
  metadata and range-scoped coverage/gap diagnostics. Exports retain original
  timestamps; Markdown and VTT also identify the processed range.
- Rust Options/Transcript literals now need range: None for whole-video work;
  Outcome exposes the optional resolved range. Old saved JSON
  invocations/configurations default to no range. No dependency changes or binary
  release; the published 0.10.1 executable does not include these source features.

## 0.11.0 — 2026-10-31

- Add JSONL batch transcription with caller IDs, immutable batch membership and
  options, independent durable jobs, ordered metadata-only results and resumable
  partial failures. Repeating the command verifies completed transcripts without
  repeating acquisition or recognition. New batches require an empty owned root.
- Bound parallel pipelines with --jobs (1..8, default 1) and serialize recognition
  within each batch. Cancellation of one job stays local; Ctrl+C stops scheduling
  the whole batch and preserves owned partial media. Retained child locks block
  restart until existing jobs are idle. Server retry guidance remains enforced.
- Keep existing job, transcript and ASR formats unchanged. Batch JSON uses schema1;
  incomplete batches return a nonzero exit and per-item errors in details.batch.
  No new runtime dependency. The existing 0.10.1 binary lacks the new command;
  use a 0.11.0 source build until a new binary release is published.

## 0.10.1 — 2026-10-31

- Provide a Linux x86_64 GNU binary archive with executable identity, project and
  third-party license notices. Binary installation is the README default;
  source builds remain supported. Release checksums have a detached GPG signature.
- Document glibc 2.39 as the binary requirement. This packaging release does not
  change transcription, checkpoint formats or experimental security boundaries.

## 0.10.0 — 2026-10-30

- Make setup provision the selected pinned model and GigaSTT automatically when
  --model-dir is omitted. Store models inside the owned runtime root. Explicit
  local model/archive options remain supported; --offline prohibits requests.
- Stream model/archive downloads with exact sizes, pinned SHA-256, bounded ranges
  and durable prefix receipts. Resume after process death, prove prefixes when
  Range is ignored, and retain/refuse changed or foreign data. Persist server
  cooldowns and reject unparseable timing guidance instead of retrying early.
- Return ready/next-action/bundle-size fields and JSON progress for setup. Existing
  completed runtime manifests, exact ASR and transcripts remain unchanged. No new
  dependencies or job-state migration. Default setup now uses network access;
  scripts that require local-only behavior must pass --offline and local assets.

## 0.9.1 — 2026-10-30

- Allow progressing bounded HTTP bodies to finish within 30 seconds instead of
  the previous five-second total-body deadline. Retain the 45-second per-call
  budget and existing byte, trust, cooldown and audio identity limits.
- Report bounded body-read failures with a sanitized phase, typed reason and
  attempt byte count. Do not log raw error messages, response content or URLs.
  Regressions reproduce the old progressing-body timeout, truncated input and
  private-message redaction. No new dependencies or job/schema migration.

## 0.9.0 — 2026-10-30

- Add --model-variant e2e_rnnt|ml_ctc|ml_ctc_large to setup, doctor and transcribe.
  Prepared runtimes and saved jobs retain the selected model; file checks and
  fingerprints use the correct encoder/vocabulary or RNN-T bundle.
- Refuse changed models before checkpoint recovery or cleanup, including completed
  jobs. Missing selection in old jobs/runtime state defaults to e2e_rnnt. Exact
  ASR JSON and saved transcripts remain unchanged.
- Report recognizer_language=mul for multilingual models, ru for e2e_rnnt;
  source_language remains null. CTC output has no added punctuation/ITN.
- Rust embedding change: Options literals require model_variant. Use
  ModelVariant::default() for the existing Russian model. Older binaries may
  reject the new state fields; retain producing binaries for unfinished jobs.
  No new runtime dependencies. Model provisioning/checksum helper covers CTC.

## 0.8.2 — 2026-10-30

- Recover from Web preparation/token-refresh HTTP403 through an available
  compatible client instead of immediately aborting. Preserve Retry-After/429
  cooldowns, audio identity and prefix verification, and retained partial media.
  Add a failing-before/passing-after regression and incompatible-source checks.
- Document the BotGuard API key as publicly distributed by YouTube, with
  first-party provenance and explicit limits; do not change or conceal its value.
  No dependency, JSON/checkpoint schema or job migration changes.

## 0.8.1 — 2026-10-30

- Report chapter watch-request failures as metadata failures without claiming
  nonexistent retained media or resume artifacts. Preserve typed transport/TLS
  codes, retryability, status and Retry-After information. Cover truncated HTTP
  bodies with a failing-before/passing-after CLI/schema regression. A live 0.8.0
  repeat exposed the temporary body-read failure; it is retained in the evidence.

## 0.8.0 — 2026-10-30

- Add `chapters URL`: return author-published description timestamps and titles
  as schema1 JSON from one bounded watch-page request. No audio acquisition,
  PO-token/player evaluation, work directory, recognizer or model setup is needed.
  Include provenance and distinguish not_found from denied/malformed metadata.
  Validate video identity, completed duration, timestamp order/ranges and limits;
  never invent chapters or label automatic navigation as author-written.
- Refresh the public root README against agent-dev-kb README standard v1 with
  the metadata-only first useful result, current scope and scoped historical
  recovery measurements. Record missing coverage/current benchmark evidence.

## 0.7.0 — 2026-10-30

- Establish the independent agent-youtube crate, executable and Rust library
  (`agent_youtube`). Rename build/test environment variables, download cache,
  locks, temporary ownership paths, worker protocol flag and managed-runtime marker.
- Breaking integration change: invoke agent-youtube, update Rust imports and
  rebuild integrations. Manifest schema2 now uses application_version. Checkpoints
  require application=agent-youtube; use fresh work/runtime directories and finish
  earlier unfinished jobs with their original executable. Do not edit job state
  or canonical/raw recognition JSON to migrate it.
- Publish a reviewed independent root history and new version designation;
  remove earlier public tags. Keep original history and evidence in private local
  backups. No automatic release upload or hosted CI is enabled.
- Regenerate public TLS signing/certificate fixtures with the new project name;
  private fixture-generation keys remain ignored. Historical numerical summaries
  use neutral identity labels and retain original measurement values/digests;
  their unedited originals remain local. This is not fresh network/ASR qualification.

## 0.6.2 — 2026-10-30

- Publish reviewed cleaned source as agent-youtube in an independent repository.
  Keep the prior executable/package names, command contracts, checkpoint formats and exact
  recognition/transcript outputs compatible. Update repository URLs and public
  documentation; preserve dated measurements and disclosed TLS/access limits.
  The original repository remains a private archive.
- Keep hosted Actions disabled and workflows dispatch-only. Crates.io publication
  and release artifact uploads remain disabled; local release preparation stays
  available without uploading anything.

- Prepare repository history for review by removing local-only tracker/MCP
  records and internal message identifiers, with private backups and mappings.
  Strengthen nested-output ignore rules and replace private research links with
  portable provenance. Recording selection, runtime source and exact ASR outputs
  are unchanged; historical measurement digests retain their original identities.

## 0.6.1 — 2026-10-30

- Repair the machine-readable contract for speech/input coverage, setup, retry
  status and waiting events. Add executable Draft 2020-12 checks over emitted
  CLI results/artifacts/events and invalid variants, running in local checks.
- Retain the exact modern RustCrypto provider source with a manifest-only
  paste-to-pastey replacement; remove the unmaintained build dependency without
  changing cryptographic algorithms or application TLS protections. The RSA
  advisory and upstream production warning remain disclosed, without suppression.
- Supply the previously proposed MIT text as root LICENSE under the owner's
  audit-remediation instruction. Keep repository/crate privacy and manual CI.
- Align installation paths, official model checksums/size, current dependency
  information and release/support navigation. Preserve dated historical evidence.
- Refuse parent Git inventories in extracted documentation checks; add explicit
  source-tree validation and checkout-independent regression fixtures.
- Prepare complete private Git source archives (including required forks), optional
  exact-commit binaries and integrity/notices manifests. Cargo package is not the
  supported distribution route. No ASR, canonical transcript, checkpoint or CLI
  response bytes are migrated; the schema now describes existing emitted fields.

## 0.6.0 — 2026-10-30

- Enable automatic outer recovery for new CLI jobs with a 3,600-second cumulative
  waiting budget. Explicit `--retry-window-seconds 0` retains immediate outer
  failure behavior. New commands can now wait/retry instead of returning the first
  exhausted transient download error; this is an intentional CLI behavior change.
- Add private retry-ledger schema2: charge durable waits before dispatch, preserve
  remaining budget across cancellation/restart, and clear deadlines during healthy
  download, decode and ASR. Existing network and inactivity limits still apply.
- Preserve old disabled/missing-option jobs and version1 absolute deadlines.
  Repeated commands inherit saved policy; missing enabled ledgers fail closed.
  Add policy/remaining-budget status fields; agent response, transcript and media
  checkpoint schemas remain unchanged.
- Verify single-invocation prefix recovery after HTTP429 and a stalled SABR stream,
  plus healthy processing beyond the retry budget, legacy behavior, cancellation,
  exhaustion and validated publication reconciliation. See migration documentation.

- Qualify 0.5.1 full audio on the pinned corpus: 12 first-invocation completions,
  two retained-prefix recoveries and one source-unavailable refusal. Preserve
  early challenge/cooldown phases and independently reverify all final media.
- Add an opt-in owned loopback probe for real multi-hour media. Verify SIGKILL,
  automatic body-truncation recovery and HTTP429 delay/resume; retain separate
  live source/token/track/representation and stall fault proofs. Document an
  explicit retry window for unattended CLI use; the 0.5.1 experiment used its unchanged runtime defaults.


- Add a multi-hour default-setting ASR restart scenario and stage-aware resource
  measurement. Omit test overrides in the default scenario, retain historical
  30-second-chunk reproduction, and distinguish actual GigaSTT RSS/OS threads
  from whole-tree RSS. Correct the short-run retry deadline in the harness;
  product processing, deadlines and checkpoint formats are unchanged.

## 0.5.1 — 2026-10-29

- Reject RSA certificate and handshake verification keys below 2048 bits before
  big-integer work; retain the upstream 8192-bit maximum and all signature checks.
  Cover 1024/2047-bit refusal, 2048/3072-bit acceptance and complete chains with
  independent public fixtures. Weak-key endpoints now fail validation; no job or
  transcript migration is needed.
- Replace panic-prone key-exchange glue with fallible calls to the same locked
  RustCrypto primitives. Preserve X25519/P-256/P-384 validation and zeroization.
  Return retryable `tls_random_unavailable` or `setup_tls_random_unavailable`
  instead of panicking on OS entropy failure; retain media for resume.
- Restore the standard rustls AES-GCM budget of 2^24 outgoing records per key.
  Verify TLS1.3 key rotation and TLS1.2 closure with a reduced test budget against
  an independent local oracle. This does not limit audio length.
- Keep the strictly pure-Rust crypto path and all dependency versions unchanged;
  expose existing primitives directly and enable X25519 secret construction.
  The RSA advisory and upstream experimental warning remain disclosed.

## 0.5.0 — 2026-10-29

- Select modern upstream RustCrypto at an exact Git revision with its qualified
  locked graph. Preserve TLS 1.2 and 1.3, public RSA certificate validation and
  checked X25519/P-256/P-384 exchange. Require the patched crypto-bigint
  family before enabling NIST secret arithmetic; reject invalid/compressed points. Move unconditional private-key refusal into the
  application instead of relying on a vendored patch. Wire group policy expands from X25519-only: the former restriction caused
  TLS1.2 ECDSA-server refusal. Media, ASR and checkpoint schemas are unchanged.
- Replace crypto-bigint 0.5.5 with patched 0.7.10. The RSA timing advisory remains
  for rsa 0.10.0-rc.19 and raw audit still fails; no advisory suppression or
  unrestricted production-security approval is introduced. Upstream's experimental
  warning remains relevant to this strictly private candidate.
- Compare the TLS1.3-only composition and both-version upstream provider on 0.4.0:
  real TLS handshakes, GitHub setup, trust fixtures and complete audio/resume.
  Keep candidate artifacts isolated and record exact identities and support limits.

## 0.4.0 — 2026-10-29

- Add JSON-only explicit `setup` for pinned project-local GigaSTT 2.22.0 on
  Linux x86_64 GNU. Verify archive/executable SHA-256, preserve notices, fingerprint
  existing models and publish the runtime manifest last. No global installation,
  model replacement or automatic dependency download during transcription.
- New CLI invocations discover `tools-local/runtime` when present and fail closed
  on invalid prepared state. Add explicit `--runtime-dir`; retain explicit
  recognizer overrides and saved-job resume. Migration: remove a mistaken
  incomplete runtime directory or prepare it explicitly; it no longer falls back
  silently to PATH. ASR, response and checkpoint schemas remain unchanged.
- Add pure-Rust gzip/tar readers with seven new locked package versions and
  whitelist-only bounded extraction. TLS provider and advisory disposition are
  unchanged. Record actual setup reuse, doctor and live transcription checks.

- Add reproducible GigaSTT 2.22.0 ASR hard-death/restart qualification on a real
  17.56-minute recording: preserve completed raw bytes, submit only remaining
  chunks, verify contiguous full-frame coverage and measure sampled RSS/disk.
  Add regressions for transcript-reader bounds and staging-observer cleanup races;
  production recognition, schemas and checkpoint formats are unchanged.

- Update the project-local recognizer setup to GigaSTT 2.22.0, verifying the
  official Linux archive checksum, prerequisite checks and full live recognition
  with existing e2e_rnnt INT8 models. Preserve historical 2.21.0 measurements,
  saved job settings and exact transcripts; no global installation change.

## 0.3.14 — 2026-10-29

- Restore the normal production release after a research build overwrote its
  executable. Isolate codec, vendor and TLS qualification targets; verify normal
  executable hashes around TLS trials. Reject inherited parent-repository Git
  provenance for nested source snapshots.
- Classify typed certificate and TLS compatibility failures as sanitized permanent
  JSON errors, preventing ineffective transport retries. Preserve existing media
  backend, checkpoint schemas, raw ASR and canonical transcript behavior.
- Qualify an isolated modern pure-Rust provider with public RSA verification:
  full acquisition and resume succeed, but TLS 1.2 support is absent and one
  RustSec advisory remains. Keep the production provider unchanged. Record live
  release transcription, completed-job reuse and local verification evidence.
- Make the ASR watcher regression respect its existing notification or polling
  mode when OS notification setup is unavailable; production ASR is unchanged.


- Record a separate 0.3.13 release acquisition sweep: 14/15 fully verified audio
  downloads, one source-unavailable refusal, no skips/timeouts; longest recording
  4.448 hours. Keep the original longitudinal cohort and runtime identity separate.

- Preserve a qualification probe's timeout cause before attempting to read an
  absent or stale completion result. Add a regression and retain original frozen
  acquisition evidence separately from subsequent harness fixes.

- Add independent public signature/certificate fixtures and trust-boundary tests
  for the configured TLS provider. Qualify an isolated pure-Rust alternative and
  record its actual SABR/CDN acquisition failure, RSA-chain incompatibility and
  the current upstream production warning. Production TLS dependencies and behavior are unchanged.

- Add a reproducible locked dependency and bundled-asset notice collector with
  verified upstream text supplements. Record a fully redacted Git/HEAD scan,
  private historical records, a proposed MIT grant and advisory disposition.
  No runtime change, public distribution or history rewrite is performed.

- Qualify patched upstream codecs in an isolated application build with shared
  input, complete PCM, process-restart and RSS measurements. Retain production
  forks: PCM differs for some inputs, and strict old-job immutability fails
  because invocation control and preflight logs change before backend refusal.
  Add opt-in probes and migration evidence; runtime and checkpoint formats stay
  unchanged.

- Correct the earlier bandlimit finalization defect claim: non-integral lengths
  violate its documented contract. Verify existing grid padding, exact tail
  trimming and checkpoint recovery at seven rates; require successful padded
  finalization in upstream patch tests. Runtime, backend identity and checkpoint
  formats are unchanged, so existing jobs remain compatible.

- Prepare review-only syom/bandlimit checkpoint patches against named upstream
  commits, with clean-application tests and a standalone bandlimit finalization-contract
  regression. Production codec forks are unchanged.
- Refresh the agent-oriented project description, topics and root README against
  agent-dev-kb README v1, with verified setup and scoped job-recovery evidence.

## 0.3.13 — 2026-10-29

- Qualify the local Linux release candidate with live full transcription,
  SIGKILL/download recovery and a two-hour synthetic decode/restart probe.
  Record scoped production evidence and unresolved public-distribution/security
  prerequisites. Repository privacy and manual-only workflows are unchanged.

- Reject an unfinished job with an incompatible media backend under the run lock,
  before retry state, invocation control or recognizer preflight can change saved
  files. Completed transcripts remain reusable across backends. Same-backend jobs
  retain their existing checkpoint format and media identity.

## 0.3.12 — 2026-10-28

- Replace the unmodified vendored ryf copy with crates.io `=0.7.3`, pinned by
  checksum and using the same SIMD-only features. All 40 copied library source
  files match the published release; no active transitive dependency is added.
  Remove the redundant snapshot and vendor-qualification entry. Audio backend,
  checkpoint and JSON schemas remain unchanged; existing jobs can resume.

- Document the GigaSTT responsibility boundary, session reuse and the recovery
  tradeoff of external checkpoint cuts, verified against source and real CLI
  probes. No recognition behavior, dependencies or schemas changed.

## 0.3.11 — 2026-10-28

- Keep assembled ASR endpoints within their integer sample spans. Fractional
  offsets previously could round one ULP past the recording end and prevent
  publication. Invalid relative ASR times still fail validation.
- Preserve floating-point confidence and timestamps when reading ASR JSON by
  enabling `serde_json` roundtrip parsing. This fixes observed one-ULP drift and
  adds no dependencies. Existing raw and published transcripts are unchanged;
  JSON/checkpoint schemas remain compatible. New assembly uses exact f64 parsing.
- Repair the opt-in ASR reference fixture for the source-duration evidence contract.
  Extend qualification to synthetic sequential speakers, pauses, attenuation and
  noise, with raw/canonical/compact fidelity assertions.

## 0.3.10 — 2026-10-28

- Report process launch failures with `process_spawn_failed`, structured program,
  OS error and cause. Missing executables and denied execution are nonretryable
  until the environment is repaired; unknown OS causes retain null retryability.
  JSON and checkpoint schemas are unchanged; callers must accept the new code.
- Document structured agent decisions and recovery after forced termination without
  final JSON; qualify ordinary CLI contention, capacity refusal and process restart.

- Extend research-only qualification to media 403, source/token expiry and
  incompatible-source refusals, plus cross-process server cooldown and real
  CLI/GigaSTT interruption-to-publication checks. Runtime behavior is unchanged.

- Add test-only SABR stall injection, renewal/prefix evidence and a strict live
  qualification runner, including persistent failure followed by process restart.
  Research hooks do not change runtime behavior or JSON/checkpoint schemas.

## 0.3.9 — 2026-10-28

- Renew stalled SABR source context once after bounded no-progress retries and
  attestation refresh. Resume the verified representation without discarding
  retained audio; repeated stalls terminate and server cooldowns remain binding.
- Add restart regressions for successful renewal, persistent stalls, changed
  replay bytes and rate limiting. JSON and checkpoint schemas are unchanged.

## 0.3.8 — 2026-10-28

- Reconcile durable transcript publication before retry deadlines and complete
  owned-media cleanup without repeating acquisition or recognition.
- Bound encoded transcript units before allocation and publication; oversized
  legacy values now return resource_limit while originals remain untouched.
  Total transcript length and recording duration remain uncapped.
- Keep subprocess output in private logs rather than public JSON failures.
- Separate publication recovery, ASR lifecycle, checkpoint ownership validation
  and YouTube source parsing; classify source refusals by typed error codes.
- Accept only sub-tick, sub-millisecond M4A duration rounding after complete
  indexed-frame decoding; preserve exact PCM counts and reject larger deficits.
  Expanded upstream qualification exposed this HE-AACv2 regression.
- Enforce production panic/unsafe lint rules with scoped OS exceptions.
- Remove obsolete webpki and unused private-key/NIST ECDH support from the
  vendored client crypto provider; replace unmaintained paste with pastey.
  RSA/crypto-bigint advisories remain explicitly qualified, not suppressed.
- Integrate portable documentation, add index-aware link checks and isolated
  pinned upstream codec qualification. JSON and checkpoint schemas are unchanged.


- Update developer instructions to agent-dev-kb 0.10.0 with verified managed
  Rust principles and local-only data rules. Stop tracking local backlog and
  Grok configuration while retaining all local files and existing Git history.

## 0.3.7 — 2026-10-28

- Add an isolated research probe for independent Silero speech evidence and
  bounded targeted recognition, with pinned runtime requirements and omission,
  silence and alignment tests. Preserve originals and separate retry evidence.
- Record false candidates caused by ASR timestamp estimates; do not enable
  automatic transcript replacement or claim verified speech completeness.
  Production dependencies, behavior and JSON/checkpoint schemas are unchanged.

## 0.3.6 — 2026-10-28

- Clarify compact JSON coverage as `audio_processed`; speech completeness remains
  explicitly `unverified`, independently of successful audio processing.
- Add streaming ASR timestamp-gap diagnostics to compact and metadata responses:
  count gaps of at least five seconds, retain at most 32 examples, and identify
  text segments without word alignment. These are review candidates, not VAD,
  silence classification or proof of missing speech. No gaps is not a guarantee.
- Preserve exact upstream JSON, canonical transcripts and existing checkpoint
  formats. Schema 2 gains additive fields; no migration or new dependency.

## 0.3.5 — 2026-10-28

- Try the WEB player API after a bot-denied watch response using that same
  page's validated client context, visitor, timestamp and anonymous cookies.
  Reuse the existing MWEB recovery path with matching WEB headers; retain
  restrictions, identity checks and server-directed waiting.
- Test both clients against context mismatches, wrong videos and known/unknown
  cooldown intervals. No new dependency or schema migration is required.

## 0.3.4 — 2026-10-28

- Add offline regression coverage for HTTP 503 after committed SABR audio,
  checking bounded recovery, server-directed waits and exact retained bytes.
- Recover an initial SABR DNS failure through at most one validated, advertised
  CDN alternative per source session. Preserve the logical request and retain a
  successful alternative; HTTP refusals, TLS failures and server waits do not
  trigger host rotation. Ambiguous I/O failures retain existing retry behavior.
- Add an opt-in test-only DNS fault for full-audio qualification. No new
  dependencies or public JSON/checkpoint schema migration is required.

## 0.3.3 — 2026-10-27

- Classify research-only downloader request counts by endpoint type so media
  segments can be distinguished from bootstrap and attestation traffic. Resume
  and corpus reports preserve unknown categories for older measurements.
- Production request behavior, dependencies and public JSON/checkpoint schemas
  are unchanged. Interpreter-worker requests remain outside these counters.

## 0.3.2 — 2026-10-27

- Honor server Retry-After intervals on YouTube challenge redirects instead of
  always retrying after 60 seconds. Preserve unknown-delay handling for malformed
  headers and the existing fallback when no header is supplied.
- Preserve available HTTP status and Retry-After seconds in research failure
  reports, distinguishing challenge redirects from HTTP 429 without logging
  credential headers or signed URLs. Existing frozen experiments remain unchanged.
- No dependency or public JSON/checkpoint schema migration is required.

## 0.3.1 — 2026-10-25

- Pin a broader private qualification corpus and add finite local multi-date
  comparisons with frozen tool artifacts, full decode/integrity proof, explicit
  cooldown skips and honest unmeasured metrics. Test-only counters measure
  downloader request attempts and verified replayed audio bytes.
- Fix research resume aggregation to report final cause and both process durations
  while retaining the initial interruption separately. Production acquisition,
  transcript schemas and dependencies are unchanged.

## 0.3.0 — 2026-10-25

- Add opt-in durable automatic job retries with --retry-window-seconds. Preserve
  the original absolute deadline and next attempt across resume, honor cooldowns,
  expose live waiting status and cooperatively stop active work at the deadline.
  Permanent/integrity/ASR errors and unparseable Retry-After are not auto-retried.
- Rust API: Options literals now require retry_window_seconds (0 disables it).
  Old saved invocation JSON defaults the missing field to0. Existing CLI defaults,
  exact ASR/canonical JSON and checkpoint schemas remain unchanged. Status adds
  waiting/deadline_exceeded and retry timestamps; new retry state uses private schema1.
  No production or transitive dependency was added.

## 0.2.4 — 2026-10-25

- Document the verified conditional availability contract, complete-media evidence
  and external limits; clarify qualified client switching after route failures.
- Retry a still-valid direct media source once after HTTP 403, waiting five seconds
  before renewing its context. The retry consumes the existing failure budget;
  repeated refusal still renews the current client before excluding it. Signed
  expiry, attestation lifetime, Retry-After, cancellation, committed ranges and
  ETag validation remain enforced. No dependency or schema migration is required.

## 0.2.3 — 2026-10-25

- Verify decoded duration against the selected source before ASR/publication and
  before completed-job reuse/status. Preserve evidence on missing duration or a
  mismatch beyond two seconds. Download receipts and leases retain selected
  duration; conflicting initial/selected lengths fail closed instead of accepting
  a shorter preview. Legacy evidence requires matching original metadata. Existing exact
  transcripts/ASR are never rewritten; unverified old jobs can now fail closed.
- Redact plain PO-provider content-binding messages and binding JSON fields in
  research logs while preserving provider event evidence. This changes only the
  local qualification tooling. No dependency, checkpoint or public JSON schema
  migration is required; duration failures add typed error codes.

## 0.2.2 — 2026-10-25

- Try remaining qualified anonymous clients after route-level HTTP403 or typed
  temporary bootstrap/transport failure, both for initial selection and retained
  representation recovery. The source retry budget stays bounded; content
  restrictions, server cooldowns and invalid metadata remain terminal.
- Include HTTP status and route in watch-page failures. No production dependency,
  checkpoint migration or public JSON schema change is required.

## 0.2.1 — 2026-10-25

- Recover an MWEB watch-page bot denial through the player API using validated
  same-page anonymous client context. Restrictions and server cooldowns still stop
  recovery; returned video/audio identity and media integrity remain checked.
- Search all qualified compatible clients before retiring a changed audio
  representation. Cross-client resume still proves every committed prefix byte;
  terminal restrictions, track/video changes and cooldowns stop recovery safely.
- Add explicit isolated bgutil HTTP-provider and MWEB options to the local
  qualification harness, pair comparison trials by video, and record provider
  loading, token requests and successful retrieval separately. Reference internal
  retries are disabled so the harness can stop on server cooldowns.
- Preserve PO-provider event namespaces during credential redaction. Package and
  comparison changes leave checkpoint schema4 and public JSON schemas unchanged.

## 0.2.0 — 2026-10-25

- Restart an incompatible YouTube acquisition in a separate owned namespace,
  retaining the preceding media. Limit each work directory to three acquisitions,
  including across process restarts. Integrity failures and server cooldowns do
  not authorize this restart.
- Distinguish failed cross-client prefix proof (`download_source_mismatch`) from
  representation changes. The low-level downloader continues to preserve and
  reject incompatible files; the pipeline manages independent attempts.
- Add JSON `status`, `resume` and cooperative `cancel`. Saved invocations use
  job schema1; a durable generation counter prevents stale cancellation from
  affecting subsequent runs. Idle status verifies retained evidence and transcript.
  `resume` preserves the transcribe response schema with command `resume`.
- Add a local, stdlib-only comparative qualification harness with complete decoded
  audio checks, executable hashes, interruption/resume proof and explicit failure/
  cooldown accounting. It introduces no production dependency.
- Reject video/track substitution across independent acquisitions with
  `download_audio_track_changed`.
- Read checkpoint schemas 1–4 and migrate to schema4 on mutation. Schema4 records
  retired acquisitions for safe delayed cleanup. Downgrade of unfinished schema4
  runs is unsupported; older binaries fail closed. Exact ASR/canonical schema1,
  compact schema2, and diagnostic schema1 are unchanged.

## 0.1.0

- Initial private resumable Rust audio acquisition and local Russian transcription,
  with JSON responses, validated durable evidence and owned-media cleanup.
