# Automatic recovery and 0.6.0 migration

New `transcribe` jobs automatically retry typed temporary acquisition failures.
The default is **3,600 seconds of cumulative waiting time**. An agent can start
one command and let it retain media, respect retry guidance and resume internally:

```sh
agent-youtube transcribe 'https://www.youtube.com/watch?v=of4B3aw7MXo' --work-dir runs/example
```

Use `--retry-window-seconds N` to choose the waiting budget. Explicit `0` disables
outer retries for a new job; the downloader's own bounded recovery still applies.
Permanent content/identity/integrity errors, ASR failures and unknown errors are
returned instead of being retried automatically. No additional download service,
proxy, cookie source or cryptographic provider is introduced.

## What the budget measures

Only time waiting between attempts spends the new budget. Active HTTP attempts,
source acquisition, decoding and recognition do not spend it. The budget does not
cancel a healthy long recording or ASR batch. Existing network timeouts, bounded
no-progress recovery and ASR inactivity/resource limits remain independent.
Consequently, total wall time may exceed an hour; the setting is not an overall
execution deadline. A caller can use the existing cancellation command when needed.

For a retryable failure the job persists its cause, next-attempt time, wait start
and wait deadline before waiting. Server Retry-After is rounded upward and never
shortened; an unparseable header is returned to the caller. Existing exponential
backoff applies without explicit guidance. If the server requires longer than
the remaining budget, the job expires without issuing an early request.

When a wait becomes eligible, elapsed wait time is deducted and persisted before
active work begins. Its pending deadline is cleared. A later independent retry
can use only the remainder. Cancellation or process downtime during a pending
wait still counts; restarting cannot refill the budget. Whole-second scheduling
uses the existing upward-rounding safety margin. At most the final eligible
attempt can start with zero remaining budget; healthy work may finish, while
another retryable failure cannot dispatch a further attempt.

`job_deadline_exceeded` remains the exhaustion code, with `retryable: false` and
preserved-artifact guidance. Its structured `retry_policy` distinguishes a new
waiting budget from a legacy execution deadline. An expired budget requires an
explicit separate job; deleting or changing its ledger is not a supported reset.
Validated publication is reconciled before a valid but exhausted retry policy,
so completed transcripts can still be reused and owned-media cleanup finished.

## Durable policy and status

The private `job-retry.json` ledger has a new version:

| Policy | Private schema | Timing |
| --- | --- | --- |
| New enabled job | 2 | Remaining waiting seconds; deadline exists only for a pending wait |
| Existing enabled job | 1 | Original absolute deadline, including active-work cancellation |
| Disabled job | No ledger | Bounded internal recovery returns errors to the caller |

New ledgers retain `window_seconds`, `remaining_seconds`, `waiting_since`,
`next_retry_at`, pending `deadline`, attempt count, cause and completion state.
Malformed combinations are rejected before use. Active work has null pending
time fields. Budget mutation is protected by the existing job lock and durable
atomic writes; the ledger is not ASR evidence and never contains session secrets.

For enabled policies, `status` and waiting events add:

- `retry_policy`: `waiting_budget` or `legacy_job_deadline`.
- `retry_budget_remaining_seconds`: projected remaining wait budget, or null for
  a legacy absolute-deadline policy.
- Existing `next_retry_at` and `deadline`: Unix seconds; new-policy values are
  null during active processing and after completion.

These are additive JSON fields. Compact/full response, transcript, raw ASR and
media checkpoint schemas are unchanged. Stdout remains one result JSON document;
waiting events use the existing stderr JSONL channel. There is no detached
scheduler: after process death, the caller launches the saved job with `resume`.

## Migration

0.6.0 is a minor-version change because a new command can now wait and retry
instead of immediately returning the first exhausted transient acquisition error.

- New jobs without a retry flag use the waiting budget.
- `resume` and repeated `transcribe` without a retry flag inherit saved policy,
  including an old explicit zero or missing option. They do not adopt new defaults.
- Existing schema1 ledgers remain schema1 with their original absolute deadline.
  This includes their existing active-work timeout behavior; no silent conversion
  extends a historical deadline.
- A saved enabled job whose ledger is missing fails `job_retry_invalid`; the
  remaining budget/deadline cannot be reconstructed safely. Media and transcripts
  are retained. Direct export of an already valid transcript remains available.
- An explicit changed window/video cannot override an existing ledger. Old disabled
  jobs can explicitly opt into enabled recovery, creating a new schema2 ledger.
- Older binaries do not support schema2 ledgers and must not be used to resume
  new 0.6.0 jobs. Rust callers still set `Options.retry_window_seconds` explicitly;
  deserializing a legacy invocation with that field absent continues to mean zero.

## Verification

Failing-before regressions reproduced both missing default recovery and active
ASR being cancelled by the old deadline. The new tests exercise one CLI invocation
through a durable ranged prefix, HTTP429 timing, replay-safe continuation and
validated publication. A separate SABR test stalls after its initialization
prefix until the outer retry is actually scheduled, then completes the same
source and transcribes it once. These use an owned loopback oracle, not a claim
about natural YouTube fault frequency.

Additional checks cover healthy ASR exceeding the configured budget before/after
a retry, remaining-budget accounting across restart, malformed/expired state,
legacy absolute deadlines, disabled old jobs, saved-policy inheritance, missing
ledgers, cancellation and durable publication reconciliation. Explicit opt-in
qualification also uses retained real audio and real local GigaSTT; evidence and
limits are in [the result record](automatic-recovery-results.json).

The real-audio fault qualification used GigaSTT 2.22.0, processed all
52.96475 seconds into six raw ASR chunks, retried one SABR stall and cleaned media
after validated publication. A separate fresh YouTube invocation downloaded the
same 858,695-byte source, decoded all 847,436 frames at 16 kHz and published a
transcript in 22.29 seconds without an outer retry. Its source digest matched the
retained reference. This is one short external sample, not a new multi-hour corpus.
The new release also reused an actual completed 0.5.1 job with byte-identical
transcript, manifest and raw ASR, preserving its disabled retry policy.

Local gates passed: formatting, strict all-target Clippy, 366 Rust tests (17
separately qualified/opt-in tests ignored), 56 Python tests, a locked Rust 1.94
all-target check and an optimized build. Existing dependency dead-code warnings
remain. Hosted CI was not invoked.

The earlier [full-source acceptance](acquisition-051-qualification.md) remains
0.5.1 evidence: two real no-progress outcomes required caller-driven resume in
that experiment. The new default automates that outer scheduling mechanism; it
does not guarantee that YouTube grants access. The separate
[TLS security disposition](tls-hardening.md) remains unchanged.
