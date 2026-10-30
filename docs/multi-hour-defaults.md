# Multi-hour transcription with default ASR settings

Historical 0.5.0-era qualification. New jobs since 0.6.0 use a 3600-second
waiting budget; [current semantics](automatic-recovery.md) supersede the old zero
default below. This report does not claim a new 0.6.1 multi-hour run.

This qualification exercises the current private Linux release on the pinned
Russian source `a2hbJlPxj4M` (about 3.82 hours). It checks the complete URL-to-JSON
workflow, actual media coverage and hard-death recognition recovery. It does not
measure speech accuracy or establish an unlimited-duration/access guarantee.
Exact finalized identities and resource measurements are in
[the result record](multi-hour-defaults-results.json).

## Observed result — 2026-10-29

The default-setting scenario completed **13,757.6838125 seconds** (3 h 49 min
17.68 s), or **220,122,941 frames at 16 kHz**, across **46 chunks**. The complete
222,653,848-byte source was verified and decoded. Three completed ASR files
survived SIGKILL unchanged; the new process submitted exactly the remaining 43.
All final hashes and contiguous frame spans passed independent verification.
Status confirms validated publication/cleanup, no staged or chunk WAV remains,
and completed-job reuse preserves transcript, manifest and all 46 raw files.

| Stage / invocation | Sampled stage time, s | Peak tree RSS, MiB | Peak CLI RSS, MiB | Peak owned allocated file blocks, MiB |
| --- | ---: | ---: | ---: | ---: |
| Fresh acquisition | 372.05 | 367.59 | 20.71 | 212.48 |
| Decode | 88.76 | 38.48 | 38.48 | 626.23 |
| Recognition before kill | 26.08 | 685.73 | 20.31 | 634.23 |
| Recognition after resume | 371.94 | 395.86 | 10.92 | 637.27 |
| Publication after resume | 0.91 | 10.92 | 10.92 | 645.93 |

The phase wall times sum to **860.99 s** (14 min 21 s), including download and
qualification overhead but excluding the earlier failed attempt. This host had
24 logical CPUs available at the initial check; these timings are not a portable
throughput claim. The separate 500 ms engine observer saw GigaSTT peaks of
**542.92 MiB** before the kill and **380.71 MiB** on resume, with **50 total OS
threads** in both invocations. Its lower sampling frequency may miss transients
seen by the 100 ms whole-tree sampler; independent maxima cannot be subtracted.

Owned allocated file storage peaked at **645.93 MiB** and ended at **9.03 MiB**
for retained publication/control evidence. Model/cache storage and the earlier
failed job are outside those figures. That failed job retains a verified
141,236,227-byte committed prefix; it was not cleaned or silently reused after
its configured deadline expired.

Local fmt, strict all-target Clippy, 349 Rust tests (15 opt-in ignored), 56 Python
qualification tests and portable documentation links pass. No production code,
version, model or ASR/checkpoint format changed; tooling is now recorded in the
0.6.0 changelog. No GitHub Actions were run.

## Scenario and invariants

The runner's `--default-settings` mode omits recognizer, chunk, encoder-thread and
retry-window overrides. The ordinary CLI discovers its prepared runtime. Saved
configuration must show 300-second chunks, automatic encoder threads and the
expected GigaSTT executable identity. This differs from the earlier 17.56-minute
[30-second-chunk experiment](gigastt-restart.md), whose measurements retain their
original configuration and scope.

After complete acquisition and decoding, the runner waits for at least three
complete full-chunk ASR files. It freezes the CLI and child processes, records
exact hashes, sends SIGKILL only to the owned process tree and verifies teardown.
The original media and valid outputs must remain unchanged after the kill. A
separate resume process adopts saved results and submits only the remaining
spans. No partial inference state is reconstructed inside an unfinished chunk.

Final checks validate every raw ASR hash, canonical transcript/coverage hashes,
ordered contiguous non-overlapping input frames and agreement with the decoded
frame count/duration. Owned media/chunks may be cleaned only after validated
publication. Completed-job reuse must preserve exact transcript, manifest and
raw bytes. Coverage proves which frames were processed, not that every word was
recognized correctly.

## Measurement boundaries

The main runner samples the owned CLI/descendant tree about every 100 ms. It
assigns measurements to the latest structured progress stage and reports parent,
children and summed RSS separately. Shared pages may be counted twice. Stage
wall-time attribution has sampling/progress-event granularity and includes waits;
it is not a CPU profiler.

Owned job file sizes/allocated blocks are sampled about once a second and
counted by unique inode. Hard-linked batch inputs therefore do not double disk
payload. Directory metadata, model provision/cache storage and unrelated jobs
are excluded. The companion observer samples only the owned GigaSTT batch
process, records RSS and total OS threads, and derives observed CPU seconds from
proc accounting. Total OS threads are not the encoder-thread setting.

All values are observed sampled peaks, not OS-enforced budgets or general
resource ceilings. Workload, hardware, model/runtime caches and machine load
matter. No unrelated build/test suite is run during live measurement.

## Reproduce

Provision the [managed runtime/models](installation.md), then select a fresh
ignored run directory:

```sh
python3 experiments/qualification/asr_restart.py \
  --output runs/default-multi-hour-new \
  --binary target/release/agent-youtube \
  --gigastt tools-local/runtime/gigastt-2.22.0 \
  --video a2hbJlPxj4M --default-settings --timeout 7200
```

Here `--gigastt` identifies the expected child executable for the qualification;
the default-settings scenario does not pass that override to the product CLI.
For engine-specific samples, run the companion while the driver is live:

```sh
python3 experiments/qualification/asr_resources.py \
  --driver DRIVER_PID --run runs/default-multi-hour-new \
  --gigastt tools-local/runtime/gigastt-2.22.0
```

The phase timeout is an external qualification budget. In this historical build, default product
`retry_window_seconds` was zero; model/ASR inactivity checks retain their
ordinary defaults. Shared server cooldowns are checked before network work and
production recovery respects Retry-After. An owned failed-acquisition job without
an expired deadline can be continued using `--resume-job`; saved source,
recognizer and chunk identity must match.

An initial attempt accidentally inherited the old harness's explicit 180-second
job window and stopped during acquisition after 181 seconds. The product returned
`job_deadline_exceeded` and retained about 140 MB of partial media. That deadline
was not reset, and its evidence is retained separately. A fresh same-source
attempt uses actual defaults. A regression asserts that default mode introduces
none of those overrides. This earlier failure is not silently discarded from the
qualification history or presented as a product duration limit.
