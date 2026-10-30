# GigaSTT 2.22.0 hard-death and ASR restart qualification

This local Linux x86_64 qualification uses a real 1,053.8376875-second recording
(17.56 minutes), the project-local GigaSTT 2.22.0 executable, and existing GigaAM
v3 e2e_rnnt INT8 models. It exercises recognition recovery after complete audio
acquisition. It does not claim multi-hour recognition qualification or measured
speech accuracy. The precise identities and observed resources are retained in
[the sanitized result record](gigastt-restart-results.json).

## Observed result — 2026-10-29

| Check | Result |
| --- | --- |
| Decoded source | 16,861,403 frames at 16 kHz; 1,053.8376875 seconds |
| Forced termination | Parent and real recognizer received SIGKILL after three complete ASR files |
| Resume | Three saved raw files adopted unchanged; only 33 remaining chunks staged |
| Publication | All 36 raw hashes and canonical transcript verified; full contiguous frame coverage |
| Cleanup/reuse | Media cleaned after publication; completed-job reuse leaves raw and canonical hashes unchanged |
| Sampled RSS | 379.22 MiB before kill; 412.84 MiB on resume |
| Sampled owned file blocks | Peak 50.73 MiB across both phases, hard links deduplicated |

No production defect was demonstrated in this scenario. The code changes are
qualification tooling and its regression tests. This is one completed acceptance
scenario, not a random sample or an estimate of general success probability.
The record preserves the exact producer and model identities; a later binary
rebuild is a different artifact.

## Failure and recovery boundary

The CLI decodes the entire recording into 36 fixed 30-second chunks, with a
shorter final chunk. Once three complete JSON results exist, the qualification
runner freezes agent-youtube and its actual recognizer child, snapshots file hashes,
then sends SIGKILL to the owned process tree. It checks that workers are gone
before starting a separate `resume` process. The killed invocation cannot return
a success JSON; the successful resume must return a valid JSON result.

Complete batch-output files remain uncommitted at the instant of the kill.
Resume validates and adopts them as raw ASR evidence before preparing another
batch. The observer verifies that only the 33 remaining input chunks are staged,
and none of the three saved chunks is resubmitted. Their final raw bytes must
match the pre-kill hashes exactly. This tests file-level recovery, not continuation
of an interrupted neural inference inside an unfinished chunk.

Final verification checks every raw-file hash, the canonical transcript hash,
and all source-frame spans: ordered, contiguous, non-overlapping and ending at
the total decoded frame count. Owned media/chunks must be absent after successful
validated publication. Retained transcript, manifest and raw JSON remain local.
Frame coverage does not establish that every spoken word was recognized or that
text near chunk boundaries is correct.

## Resource measurement

The runner samples parent-plus-descendant RSS every approximately 100 ms and
owned-job storage every approximately one second. RSS sums may count shared
pages more than once. Storage counts each inode once, so batch hard links do not
artificially double the chunk payload. It reports both apparent file bytes and
allocated file blocks; directory metadata is excluded. These are sampled observed peaks, not OS-enforced limits or
proof of a universal memory bound; provisioning, model caches and other jobs are
outside the owned-job disk measurement. Wall times include each invocation's
work and monitoring overhead and are not a comparative performance benchmark.

## Reproduce

Install the [pinned recognizer and models](installation.md), use the production
release executable, and select a new ignored output directory:

```sh
python3 experiments/qualification/asr_restart.py \
  --output runs/asr-restart-new \
  --binary target/release/agent-youtube \
  --gigastt tools-local/gigastt-2.22.0/gigastt
```

The default source is `bedsdFbhqhw`; each phase has a 1,800-second qualification
timeout. Fresh jobs use a 180-second durable retry window. To continue a matching
owned failed-acquisition job, pass `--resume-job runs/previous/job`; its saved
recognizer, source and chunk configuration must match. Existing-job resume keeps
the saved retry policy, since that command does not accept a replacement window. This is a harness budget, not a product recording-length restriction.
The harness's full-transcript reader has a separate 64 MiB memory bound. The run
checks the existing server cooldown before starting, and production network
recovery respects server delays. Stop and inspect any nonzero result; metadata
or a completed child process alone is not sufficient proof.

Earlier qualification attempts are retained separately. Two completed product
recovery, but their harness reports remain failed: the first wrongly
used a 64 KiB probe-result reader for the full transcript; the second overwrote
submission observations during normal staging cleanup. Regressions reproduce
both errors. A subsequent acquisition stopped with `download_no_progress` and
preserved its partial source; it is recorded separately from ASR outcomes. One
harness invocation also used a flag unsupported by `resume` and was rejected
before work. The final qualification continued the retained acquisition job,
then exercised hard death during ASR and another process restart. Earlier failed
reports are not relabeled as successful. No production algorithm, model, schema or
checkpoint format was changed by this work.

Local verification passed: `cargo fmt --check`, strict all-target Clippy,
334 Rust tests (14 opt-in tests ignored), 49 Python qualification tests and
portable documentation links. No GitHub Actions were run.

[The subsequent 3.82-hour default-settings qualification](multi-hour-defaults.md)
uses 300-second chunks and automatic threads; it is separate from this 30-second
chunk historical result.
