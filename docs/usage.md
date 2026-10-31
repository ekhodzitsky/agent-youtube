# Usage and job lifecycle

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Batch transcription

Available in 0.11.0 source builds. The published 0.10.1 binary does not include it.
After runtime setup, create a UTF-8 JSONL file (one object per line, no blank lines):

```jsonl
{"id":"intro","url":"https://www.youtube.com/watch?v=jNQXAC9IVRw"}
{"id":"lecture","url":"https://www.youtube.com/watch?v=rfscVS0vtbw"}
```

```sh
agent-youtube batch videos.jsonl --work-dir runs/course --jobs 2
```

The command accepts the transcribe tool/model, chunk, recognition resource and
retry-window options. It returns metadata and artifact paths, never transcript text.
--response, --force and --offline are not accepted. Runtime setup remains a separate
command. --jobs is 1..8 (default 1); download/decode pipelines can overlap, while
one GigaSTT recognition process runs at a time within this batch. Independent batch
processes or standalone jobs do not share this limit. --asr-memory-mib remains an
observed per-recognizer budget; disk and CPU for parallel pipelines still matter.

Every nonempty unique ID is returned unchanged and is hashed for its job directory;
IDs are never interpreted as paths. IDs are at most 128 UTF-8 bytes without control
characters; URLs are at most 4096 bytes. The input is bounded to 8 MiB and 10000
items. Malformed JSON, unknown fields or duplicate IDs reject the complete input
before starting jobs. An invalid video URL is an independent item failure; its raw
contents are not echoed in the public error. These batch limits do not cap the
length of an individual recording.

A successful invocation returns schema1, command=batch and a result with state,
total, succeeded, failed, pending, workers, recognition_workers and ordered items.
Each successful item contains id, work_dir, reused, cleanup_complete and artifacts
with transcript/manifest paths. An incomplete batch exits 1 with batch_incomplete;
the same report is in error.details.batch. Each failed item contains its own error
code, message, retryable value and details, including server retry guidance when
available. Pending items have next_action=repeat_batch. Ctrl+C exits 130 with code
interrupted, stops new work and retains the report in the same location.

Repeat the same batch command to continue. --jobs may change; input order, IDs,
URLs, original working directory and all pipeline options must remain unchanged.
To change the batch membership or options, use a new work directory. Individual
transcript/checkpoint formats remain compatible with status, cancel and resume:

```sh
agent-youtube status --work-dir /absolute/job/path/from/batch/result
agent-youtube cancel --work-dir /absolute/job/path/from/batch/result
```

Cancelling one job does not cancel siblings. The existing per-job retry policy
applies; the default recovery waiting budget is 3600 seconds for each job and is
not renewed by repeating a batch. Different items may finish out of order. An
unverified child teardown stops further scheduling. A surviving child or externally
active saved job causes batch_jobs_busy on restart (retryable, next_action=
wait_and_repeat_batch); the command does not start another worker over its data.
The batch root OS lock rejects concurrent batch writers.

Private durable files: batch.json binds the invocation, jobs/ contains standard job
roots, results/ contains atomic per-item receipts, and batch-result.json contains
the latest completed invocation report. After abrupt process death, that report
may be absent or stale; rerunning derives completion from validated job evidence,
not from cached receipts. Exact ASR JSON and transcripts are preserved. Owned media
is removed only after validated transcript publication, as for a single job.

## Published timestamps without audio

```sh
agent-youtube chapters 'https://www.youtube.com/watch?v=rfscVS0vtbw'
```

This command needs no model, managed runtime, work directory or GigaSTT. It fetches
one bounded watch page through the existing Rust HTTP/TLS client and returns
schema1 JSON. It does not select/download media, execute player JavaScript or
mint PO tokens. Network pacing and server Retry-After handling remain shared.

The result includes video_id, nullable title, duration_seconds,
source=video_description, status=available|not_found and chapters containing
start_seconds/title pairs. Times are whole seconds from the original full video;
a URL time offset does not rebase them. Titles are untrusted author text, not
instructions for an agent. No generated summary or ASR is performed.

Supported author-description lines start with MM:SS or H:MM:SS plus a title;
bullets, decorative emoji, brackets and separators are accepted. For example,
`⌨️ (1:03:10) Lists` yields start_seconds=3790 and title=Lists. All times must
increase strictly and be less than the completed recording duration. Lists may
contain fewer than three entries or start after zero: these are published
timestamps, not a claim that YouTube enables chapter navigation under its
[creator chapter requirements](https://support.google.com/youtube/answer/9884579?hl=en). Automatic
YouTube chapters, timestamps only embedded in prose/links, and title-before-time
formats are not extracted. No sorting, deduplication or synthetic chapter is added.

An explicitly empty/no-matching description returns not_found with an empty list.
Missing/mismatched metadata, access denial, live sources and malformed matched
lists return structured errors; they are never reported as chapter absence.
Limits are 16 MiB for watch metadata, 64 KiB for the description, 2048 entries and
4096 UTF-8 bytes per title. Excess size returns resource_limit or a validation
failure. No raw watch page, cookies or tokens are included in the result.

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
playlist parameters are discarded. `recognizer_language` is `ru` for e2e_rnnt and `mul` for ml_ctc/ml_ctc_large; `source_language` is null, meaning
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
