# Release acquisition qualification — 2026-10-29

For the newer release, see [0.5.1 full-audio and recovery acceptance](acquisition-051-qualification.md).

A finite fresh-acquisition sweep of the pinned 15-video corpus completed with
release 0.3.13 on one Linux network: **14/15 attempted downloads
fully verified**, 1 failures, 0 skipped. This is
93.3% of scheduled cases; it is not a representative
YouTube-wide success probability or an availability SLA.

[Exact numeric evidence](acquisition-qualification-results.json) pins the release
binary, release-mode probe, original helper, driver and corpus hashes. The probe
uses the production downloader/decoder and the release executable for its Web
worker. It does not run recognition. The separate [release qualification](release-readiness.md)
records actual GigaSTT transcription and SIGKILL recovery.

## Completeness and failures

Success requires a matching video/representation receipt, exact declared media
byte count, full compressed-file SHA-256, valid mono 16 kHz PCM/WAV payload,
complete decoder completion and duration agreement with source metadata. Every
success was reverified from retained media after the sweep; metadata or initial
bytes alone cannot count. Longest verified recording: **4.448 hours**.
Total decoded audio across successful cases: **15.571 hours**.

| Video | Full audio | Decoded seconds / failure |
| --- | --- | ---: |
| `of4B3aw7MXo` | Yes | 52.964750 |
| `2teKcIDANI4` | Yes | 852.799313 |
| `a2hbJlPxj4M` | Yes | 13757.683812 |
| `BaW_jenozKc` | No | youtube_source_unavailable |
| `jNQXAC9IVRw` | Yes | 19.063625 |
| `aqz-KE-bpKQ` | Yes | 634.624000 |
| `M7lc1UVf-VE` | Yes | 1343.692375 |
| `aircAruvnKk` | Yes | 1119.991313 |
| `Ilg3gGewQ5U` | Yes | 766.746125 |
| `rfscVS0vtbw` | Yes | 16011.598375 |
| `VMj-3S1tku0` | Yes | 8751.949250 |
| `zjkBMFhNj_g` | Yes | 3587.738438 |
| `IiKtcKEdUFE` | Yes | 5235.960500 |
| `LXb3EKWsInQ` | Yes | 313.840937 |
| `kqtD5dpn9C8` | Yes | 3605.641000 |

A source-unavailable code is the observed refusal from the selected production
paths, not proof that the video is globally unavailable to every account or
network. No failed case was removed from the denominator or replaced. The
15-item corpus was selected before this run and includes differing durations,
languages and selection-intent categories; it is not a random sample.

## Bounds and reproducibility

Trials ran sequentially with at least five seconds between attempts, a 600-second
per-trial bound and a 2,700-second series budget. Shared server cooldown state was
checked before each trial; a rate-limit signal stops later network attempts.
Media, receipts and failures remain in owned ignored local runs; earlier cohorts
were not overwritten. A harness timeout would be inconclusive for product
capability, not a duration limit in agent-youtube.

The current release is a separate artifact cohort from the historical 25/28
October native/reference comparison. It does not complete a three-day same-cohort
claim and does not rank competitors. Only the current network was available.

The sweep used the tracked `experiments/qualification/corpus.json` and `qualify.py`
with an owned driver under `runs/tls-availability-20261029`. Original helper and
driver hashes are retained. A subsequent helper regression fixes timeouts before
reading absent/stale result JSON; the already-loaded original sweep was not
mutated or relabeled. The 42 Python qualification tests pass with that fix.
Reproduce with fresh owned directories and explicit frozen binary/probe paths;
`qualify.native` accepts only full-completion evidence via `verify_native`.

These acquisition results do not resolve the [TLS production-security limitation](tls-security-review.md),
ASR accuracy, regional/account restrictions or public-distribution prerequisites.
