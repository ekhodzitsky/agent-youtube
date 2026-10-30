# Release 0.5.1 full-audio acceptance — 2026-10-29

These measurements describe 0.5.1. The newer [0.6.0 automatic-recovery policy](automatic-recovery.md) changes new-job defaults.

The pinned 15-video corpus produced **14 fully downloaded and decoded recordings**
on the current Linux network: **12 completed on the first invocation, two after
retained-prefix resume, one source-unavailable failure**. All 15 distinct videos
were attempted after the continuation; none was replaced or dropped.
Total complete audio is **15.571 hours**, with a maximum of **4.448 hours**.
This is a selected-corpus observation, not a YouTube-wide success probability or SLA.

[Numeric evidence](acquisition-051-results.json) binds the exact 0.5.1 release
binary, release-mode research probe, helper, corpus and source revision. The probe
calls the production downloader and native decoder and uses the frozen release
executable for its Rust Web worker. Recognition was not run. The only source change
in this qualification is an additional opt-in loopback test hook; production
download logic, dependency versions, default retry policy and schemas are unchanged.

## Complete-file verification

Every success was reread after the trials: matching video/representation receipt,
exact declared source byte count, whole-file SHA-256, complete native decode, fully
read mono PCM16/16k WAV payload and duration agreement within two seconds of the
coarse source metadata. All 14 representation identities and compressed-file hashes
also match the independent [0.3.13 acquisition cohort](acquisition-qualification.md).
That is additional integrity evidence, not a speed or availability ranking.

| Video | Complete audio | Decoded seconds / result |
| --- | --- | ---: |
| `of4B3aw7MXo` | Yes | 52.964750 |
| `2teKcIDANI4` | Yes | 852.799313 |
| `a2hbJlPxj4M` | Yes, after resume | 13757.683812 |
| `BaW_jenozKc` | No | youtube_source_unavailable |
| `jNQXAC9IVRw` | Yes | 19.063625 |
| `aqz-KE-bpKQ` | Yes | 634.624000 |
| `M7lc1UVf-VE` | Yes | 1343.692375 |
| `aircAruvnKk` | Yes | 1119.991313 |
| `Ilg3gGewQ5U` | Yes, after resume | 766.746125 |
| `rfscVS0vtbw` | Yes | 16011.598375 |
| `VMj-3S1tku0` | Yes | 8751.949250 |
| `zjkBMFhNj_g` | Yes | 3587.738438 |
| `IiKtcKEdUFE` | Yes | 5235.960500 |
| `LXb3EKWsInQ` | Yes | 313.840937 |
| `kqtD5dpn9C8` | Yes | 3605.641000 |

The source-unavailable code is the observed anonymous refusal in this network;
it does not establish global deletion or impossibility for another authorized
account or region. No credentials, paid service or second network were added.

## Natural interruptions and early throttling

The first scheduled attempt waited for the historical campaign lock and then
skipped all 15 cases because its shared server cooldown was active. After expiry,
a separate phase made one watch request and received HTTP302 to the external
YouTube rate-limit challenge with Retry-After 60; its remaining 14 cases were
skipped before media requests. Both original reports remain intact.

The final cohort began at least ten minutes after the last observed challenge.
This quiet interval preceded renewed access; it is not a guaranteed cooldown
prescription. The same release artifacts were used throughout, and shared state
was never cleared or bypassed.

Within that cohort, two real SABR streams returned `download_no_progress`,
`retryable: true`, with one-second retry guidance. New processes resumed the
unchanged owned media/receipts and completed full verification:

| Video | Preserved prefix bytes | Active resume + decode seconds |
| --- | ---: | ---: |
| `a2hbJlPxj4M` | 1,148,661 | 363.140 |
| `Ilg3gGewQ5U` | 1,133,147 | 26.877 |

These are process wall times including remaining acquisition and decoding,
excluding caller/analysis delay; they are not first-byte recovery latency. The
initial process failures remain in the evidence.

A conservative local controller initially paused on the first one-second local
no-progress hint and labeled subsequent skips as cooldown skips. The underlying
failure had no HTTP status or active global cooldown. A separate continuation
attempted only those unattempted IDs after successful resume; the raw report was
not rewritten. A nullable-label bug in that local continuation was corrected after
preserving its original driver/report and all completed cases. Neither runner
correction changed the downloader or converted an incomplete file into success.

## Controlled recovery checks

All seven controlled checks over actual remaining YouTube media completed with
preserved prefixes and full decode:

- Injected transport HTTP403, expired source and expired PO-token paths recovered.
  The token-expiry case recorded successful token refresh and completed media.
- A changed audio-track candidate returned `download_audio_track_changed`; a
  changed representation returned `download_representation_changed`. Neither was
  appended to retained media. A new process with only the synthetic mutation
  disabled resumed a compatible source and preserved the original prefix.
- A stalled stream renewed its source once. A persistent stall returned
  `download_no_progress` with retained media; a new process completed it.

These are explicit test injections, not claims that YouTube naturally returned
HTTP403, expired a token or changed a representation in these trials. All seven
final files were independently reread and match the earlier complete short source.

Three further tests streamed a retained **222,653,848-byte, 3.821-hour** real
recording through an owned loopback HTTP oracle using bounded file reads:

- SIGKILL after a committed 1 MiB prefix; a new process completed the exact source.
- HTTP body truncation after that prefix; the downloader recovered automatically.
- HTTP429 with Retry-After 1; owned media survived, the requested pause was honored,
  and a new process completed the exact source.

Every final compressed hash and decoded duration matched the independent source.
The initial body-truncation runner expected a returned error; the client instead
recovered automatically. The corrected runner records the injected fault and its
committed prefix before accepting that stronger successful outcome. Loopback
outcomes prove controlled data-plane behavior, not YouTube availability or TLS.

## Agent operation and bounds

For unattended CLI jobs, explicitly configure an appropriate durable retry window,
for example `--retry-window-seconds 3600`. The default remains zero: bounded
internal recovery can return a retryable failure for the caller to schedule and
resume. This experiment used new-process caller-driven resume for the two natural
no-progress outcomes; it does not claim that the default CLI automatically did so.

Trials ran sequentially with five seconds between videos, a 600-second per-process
bound and the original 2,700-second cohort deadline retained by the continuation.
Controlled fault batches had their own finite per-process bounds. These are
research bounds, not product duration limits. Media and failed-stage evidence
remain in owned ignored local runs; transcripts and exact ASR outputs were not
modified. No hosted CI was enabled.

`cargo fmt --check`, strict all-target Clippy, 357 Rust tests and 56 Python tests
passed; 16 opt-in Rust tests remain excluded from the ordinary suite. Selected
opt-in procedures were executed separately as described above.

## Reproduction and retained limits

Private raw reports, frozen executables and drivers are under
`runs/acquisition-051-20261029`, `runs/acquisition-051-postcooldown-20261029` and
`runs/acquisition-051-quiet-20261029`. The tracked qualification helpers provide
full-file verification, controlled access and stall checks. The new ignored
test `download::tests::research_loopback_retained_media` accepts an explicitly
owned loopback oracle, expected duration and a new or resumed owned directory;
it is never a CLI origin override or production download dependency.

Public finished AAC/M4A recordings in one network are the measured scope.
Account, age, region, removed-source, unsupported-format and server-wide refusal
conditions remain distinct limits. These results do not prove recognition
accuracy, all possible durations, every client/CDN path or unconditional access.
The separate [TLS security disposition](tls-hardening.md) also remains in force.
