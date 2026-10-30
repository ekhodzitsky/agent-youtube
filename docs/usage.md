# Usage and job lifecycle

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Use

```sh
agent-youtube doctor --gigastt ./tools-local/gigastt-2.22.0/gigastt \
  --model-dir ./models/e2e_rnnt

agent-youtube transcribe 'https://www.youtube.com/watch?v=of4B3aw7MXo' \
  --work-dir runs/example \
  --gigastt ./tools-local/gigastt-2.22.0/gigastt --model-dir ./models/e2e_rnnt

agent-youtube export runs/example/transcript.json --format json --output example.json
agent-youtube export runs/example/transcript.json --format md  --output example.md
agent-youtube export runs/example/transcript.json --format txt --output example.txt
agent-youtube export runs/example/transcript.json --format srt --output example.srt
agent-youtube export runs/example/transcript.json --format vtt --output example.vtt
```

Exports need only the JSON: no media, model, network or external tool is required.
Existing output files cause an error; `export --force` explicitly replaces an export.
It cannot overwrite its own input JSON. A literal option value such as `--output --help`
is a filename; use `--` before positional paths beginning with a dash. The ASR executable
can be supplied with `--gigastt`. The former `--yt-dlp`, `--ffmpeg` and `--ffprobe`
options are no longer accepted.
UTF-8 work paths containing spaces, percent signs, dollars or leading dashes are handled
as literal paths. They are canonicalized before invoking tools; returned run paths are absolute.
Arguments never pass through a shell.

Accepted URL forms are HTTPS `youtube.com/watch?v=ID`, `/shorts/ID`, `/live/ID`
and `youtu.be/ID`. Playlist/search/arbitrary-host inputs are rejected. Tracking and
playlist parameters are discarded. `recognizer_language` is configured as `ru`; `source_language` is null, meaning
undetected; this version does not detect whether the spoken language is Russian.


## Job lifecycle

```sh
agent-youtube status --work-dir runs/JOB
agent-youtube cancel --work-dir runs/JOB
agent-youtube resume --work-dir runs/JOB --response metadata
```

All commands return one JSON document. `status` takes an existing directory and
performs no network/model work or checkpoint mutation. The OS lock identifies a
running job. When idle, it verifies the committed checkpoint, published transcript
and retained evidence hashes before reporting completion. States are `not_started`,
`running`, `waiting`, `resumable`, `deadline_exceeded`, `cleanup_pending`, and `completed`; `next_action` describes
the next operation. Running status does not read a concurrently changing journal.
Validation of a completed job reads its retained evidence and can take time.

`cancel` acknowledges a cooperative request for the current run, not completed
termination. Poll status until idle. The request uses a durable generation counter;
a later run ignores stale requests. Initialization and cancel requests share a short
control lock, so a request cannot acknowledge the preceding generation while a new
run starts. Retry `work_directory_busy` contention. Cancellation latency includes the downloader's
bounded network operations and child teardown. A legacy binary without job control
cannot acknowledge this request; use its existing signal handling.

`resume` restores the saved invocation from private `job.json` and returns the same
transcript response shape as `transcribe`, with command `resume`. Completed runs
reuse the transcript without models or network. Legacy runs without saved options
report `repeat_transcribe`; repeat their original command once. Relative model/tool
paths require resuming from the original working directory; bare tool names still
use PATH. Changed binaries/models/settings remain subject to checkpoint compatibility
checks. To change resource options, repeat `transcribe` with explicit options.
`job-control.json` and `job-cancel.json` are private local control records, not ASR
evidence. No detached scheduler or MCP server is included; an agent can already call
these JSON commands, and a future thin MCP adapter can reuse this lifecycle.

New CLI jobs automatically continue transient acquisition failures with a
3,600-second cumulative **waiting budget**. Override it with
`--retry-window-seconds N`; explicit `0` disables outer retries. Active network
attempts, decoding and recognition do not spend this budget and are not cancelled
when the budget would have elapsed on a wall clock. Their own transport and ASR
inactivity limits still apply. Total job wall time can exceed the waiting budget.

A pending retry has a durable deadline and next-attempt time. Time spent waiting,
including downtime/cancellation while waiting, is charged before the next attempt.
Active work clears that pending deadline. Repeated `transcribe` without a policy
flag and `resume` inherit the saved policy; they do not refill its budget. Saved
version-1 schedules keep their original absolute job deadline, including active
cancellation. Old disabled or missing-option invocations remain disabled. See
[the versioned migration contract](automatic-recovery.md).

Only typed transient source/download failures are automatically retried. Server
Retry-After is rounded upward and never shortened; without a supplied delay,
outer retries wait60/120/240/480/960 seconds, then remain at960. Permanent content,
identity/integrity, ASR and unknown errors are not retried. Unparseable Retry-After
is returned to the caller rather than guessed. Waiting produces JSON progress on
stderr; stdout still contains one final JSON result. `status` exposes Unix-second
`next_retry_at` and `deadline`, plus `retry_policy` and
`retry_budget_remaining_seconds`. A new-policy `deadline` is null during active
processing and is set only during a pending wait. `waiting` requires an active
process owning the run lock. `cancel` interrupts waiting without clearing the schedule.

Private `job-retry.json` version2 retains the video binding, configured waiting
budget, remaining seconds, wait start/deadline, next attempt and cause code.
Legacy version1 retains its original absolute deadline. After process termination, launch `resume` to continue
the saved schedule; no daemon restarts a dead process or survives host shutdown by
itself. Expiry returns `job_deadline_exceeded` with preserved data and exit1;
status directs the caller to inspect preserved artifacts. Cancellation remains
exit130. A changed window/video, malformed schedule or missing previously enabled ledger
fails as `job_retry_invalid`;
an expired budget requires a separate explicit job rather than resetting the file.
Rust embedding callers add `retry_window_seconds: 0` (or their chosen window) to
`Options` literals; legacy saved invocation JSON defaults the missing field to0.

## Troubleshooting

| Code or symptom | Action |
| --- | --- |
| `runtime_invalid` | Check the prepared directory and its fingerprints; use a new directory for changed runtime/models, or select an explicit recognizer. Do not delete task evidence. |
| `setup_models_invalid` | Provision and verify the four files; pass their directory to setup. Setup never fetches models. |
| `process_spawn_failed` | Inspect structured `reason`; fix the executable, interpreter/loader or permissions. |
| `youtube_rate_limited` / `youtube_bot_check` | Let the saved bounded schedule honor timing; after process death, use status/resume. Do not clear the shared cooldown. |
| `job_deadline_exceeded` | Inspect preserved data. Existing exhausted schedules are not reset; a separate job is an explicit new attempt. |
| `insufficient_disk_space` | Free space outside retained task media and resume the same job. |
| `checkpoint_settings_mismatch` | Retain the old binary/configuration for unfinished work or choose a new work directory. |
| `tls_certificate_invalid` / `tls_peer_incompatible` | Repair trust/endpoint compatibility; retries cannot bypass certificate validation. |
| `process_failed` / `process_timeout` | Inspect the owned private stderr log and resource/inactivity settings; resume after correcting the cause. Raw tool output is not copied into JSON errors. |
| `cleanup_pending` | Transcript is already published. Resolve the reported owned-file conflict and repeat the command to finish cleanup. |

The [agent contract](agent-contract.md) describes stable error fields, unknown
retryability, cancellation and status races. [Current readiness](readiness.md)
defines the qualified platform and security boundary.
