# Sitta live handoff and integrated review, 2026-11-23

This bounded, explicitly authorized handoff used agent-youtube `0.18.0` and Sitta
`0.1.0-dev.41`. At most four acquisition processes were declared: one direct case,
one intentional timeout, one restart and one explicit MWEB-PO case. Unexpected
failure, cooldown, restriction or a missing committed prefix stopped the series.

Only the first two acquisition processes ran. No resumed or MWEB-PO request was made,
so this is not a completed direct/SABR/token-bearing handoff.

## Direct adapter success

The real Rust adapter completed `of4B3aw7MXo` with anonymous VisionOS direct:

- 858,608 bytes;
- SHA-256 `283550d73f033f6ef17becc4c9301fad5e100237333e4f3d66d5d10318fdccb5`;
- native Sitta validation and independent full FFprobe/FFmpeg decode passed;
- `botguard_used:false`, so no attestation or token-enforcement claim applies.

The output remained private under ignored `runs/sitta-live-handoff-20261123/direct`.

## Intentional restart stop

The next process used a predeclared two-second operation timeout on `dQw4w9WgXcQ`.
It returned `deadline_exceeded` before creating `.resume.json`. There was therefore
no committed prefix and no legitimate resumed invocation. The series stopped: the
third and fourth processes did not run. No MWEB-PO request, SABR handoff or
token-bearing result was obtained. Private interrupted evidence is retained; no
partial was adopted by native acquisition.

This is an honest incomplete restart result, not evidence that resume is broken or
works generally. It does not justify retrying with a longer deadline merely to obtain
a preferred result.

## Integrated production review

Offline evidence already establishes schema negotiation, one bounded child per
acquisition, cancellation/failure/restart behavior, private evidence handling and
independent output digest verification. The full Rust and Sitta test suites and
Clippy passed without changing ASR/model semantics. Native acquisition remains the
default and can be selected by leaving `AGENT_YOUTUBE_SITTA` unset.

Production recommendation: **not ready for production default or release**. The
standalone direct adapter is usable for the selected direct case, but measured live
restart and token-bearing handoff are incomplete. Provider-backed Web/SABR and token
enforcement remain unproven. The final decision needs new owner-authorized evidence;
it must not be inferred from these direct successes.

Process/evidence accounting:

- Direct and interrupted acquisition processes: 2.
- Resumed and MWEB-PO processes: 0.
- Direct validated bytes: 858,608.
- Resumed bytes: unavailable, because no checkpoint was created.
- Live requests after the stop: 0.
- Release, publication and hosted CI: not performed.

## Corrected checkpoint-triggered handoff

A fresh run used a new checkpoint-triggered harness after the first harness read
`PREFIX.resume.json` instead of the actual `PREFIX.m4a.resume.json`. The corrected
observer waits for an intermediate checkpoint with `0 < bytes < total`, cancels,
and starts a fresh adapter process for the same output. This is a harness correction,
not a longer retry of the previous request.

The first process committed 1,048,576 bytes. The second resumed exactly those bytes
and completed 3,449,447 bytes with SHA-256
`51aa762a3254bcbad8813d2badff2f301ed0127a61643a0ab16f387c883e6394`.
A separate explicit MWEB-PO process then selected SABR, attached video-bound PO and
completed the same bytes/hash. Both outputs passed independent full FFprobe/FFmpeg
decode. Three acquisition processes ran; no native fallback or unproved partial
adoption occurred.

## Final recommendation

The adapter is **conditionally ready for opt-in use on Linux amd64**. Direct and
MWEB-SABR handoff, checkpoint-triggered interruption/restart, independent output
verification and full decode are demonstrated. Production default enablement,
release publication and hosted CI remain separate owner decisions.

This recommendation does not imply availability, superiority or mandatory PO.
The corresponding SABR controls returned 200/200/200 with and without PO, so token
enforcement was not observed in that selected case. No further live request is needed
for this review.
