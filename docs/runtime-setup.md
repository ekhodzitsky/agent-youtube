# Managed recognizer runtime — 0.10.0

[Installation](installation.md) · [Current readiness](readiness.md)

## Online setup

agent-youtube setup installs the selected pinned GigaSTT 2.22.0 runtime and models
in tools-local/runtime by default. --runtime-dir chooses another owned directory;
--model-variant chooses e2e_rnnt (default), ml_ctc or ml_ctc_large. An explicit
--model-dir selects local files only. With no model directory, models are stored
inside the runtime root, not in a global home-directory model store.

The installer downloads expected files from fixed GitHub release URLs and immutable
Hugging Face revisions. Exact sizes and SHA-256 values are compiled in
[src/runtime/assets.rs](../src/runtime/assets.rs). HTTPS uses the existing checked
Rust TLS provider; redirects are HTTPS-only on explicit GitHub/Hugging Face CDN
hosts. No credentials or arbitrary download URL is accepted. Signed redirect URLs
remain in memory and never enter JSON diagnostics or transfer receipts.

## Transfer ownership and resume

After the private root is claimed, an OS lock serializes setup. A private
.setup-downloads cache has its own owner marker. Each pinned asset has an
identity-bound partial file and a bounded receipt recording total size, committed
length and SHA-256 of the committed prefix. Network data streams through a 64 KiB
buffer; requests normally use 4 MiB ranges. Files are not loaded into RAM.

Data is synced before publishing a durable cursor. On restart, the committed
prefix is rehashed and verified; only its owned uncommitted tail can be rolled
back. Changed committed bytes, foreign partials, linked artifacts, changed ready
files and malformed range/length/encoding responses are refused. If a server
ignores Range, the complete retained prefix must match before any append.
The final exact size and pinned SHA-256 must match before durable publication.

Transient transfer failures retry with bounded backoff; no-progress attempts are
limited to three. A completed prefix can continue in a new process. Repeat the
same setup command to resume. HTTP429/Retry-After persist a cooldown in this root;
a new invocation cannot shorten it. Unparseable guidance blocks further setup
network requests for inspection. Connect/response/body/per-call timeouts and
redirect limits bound each request. Finite asset sizes bound total retained data.

## Offline and local installation

--offline prohibits requests. Missing/incomplete assets produce setup_offline_missing
with the asset and next action. Complete verified files and fully committed cache
files can be reused/published without network access. For a new offline root,
provide --model-dir and --archive (see installation). Local archives must match
the compiled release hash; explicit local model sets retain actual-byte
fingerprinting. Setup never replaces supplied model files.

The release archive is streamed with a 32 MiB compressed and 128 MiB expanded
limit. Only the executable and four expected notice/license/readme/changelog
files are accepted. Links, traversal, unexpected entries and truncated gzip are
rejected. The executable hash is checked before granting execute permission.
The managed manifest is published last after rechecking models. Existing complete
manifests are validated without rewriting them. Runtime/model changes require a
new root. Current runtime and job identities remain compatible with 0.9.x.

## JSON and readiness

One final JSON result is written to stdout; progress on stderr is JSONL.
setup_plan reports selected model, pinned file bytes and offline policy;
setup_download reports asset name, total bytes and durable received bytes.
Successful output reports state=ready, next_action=transcribe, reused and
 download_bundle_bytes. That size is the sum of pinned model files and compressed
runtime archive, not actual bytes transferred in this invocation: local/reused
assets cost no new download and retries can retransmit data.

Ready means preparation and fingerprint validation. Model loading, inference and
YouTube access remain not_checked; doctor and an actual transcription are separate
checks. Recognition uses native ONNX through GigaSTT. No model or transcript
accuracy, universal access or unrestricted TLS security approval is implied.

## Current qualification — 2026-10-30

A new empty ml_ctc runtime fetched all assets from the real upstream sources.
SIGKILL after 1,061,452 durably committed model bytes was followed by setup in a new
process; model/runtime pins verified, offline reuse and doctor passed. The newly
installed runtime then acquired and transcribed a fresh 19.063625-second video,
validated contiguous sample coverage and cleaned temporary media after publication.
[Detailed evidence and reproduction](verification.md#automatic-setup-qualification--0100-2026-10-30).

## Historical local qualification — 2026-10-29

The pinned archive was actually fetched over HTTPS and installed. An offline
archive installation and repeated setup also passed. Doctor and a fresh full
52.96475-second transcription selected the managed executable with absolute
saved paths and no recognizer flag or PATH modification. Canonical/raw hashes
were verified, and owned media was cleaned only after publication. Setup reuse
after inference still validates the model and executable fingerprints.

A deliberately altered manifest/model fingerprint and an altered executable in
an owned runtime copy were both refused. A completed 0.3.14 job reused its
original transcript, manifest and raw bytes unchanged. CLI regressions verify
unfinished saved-tool selection and explicit overrides even with a broken
default runtime. Local fmt, strict all-target Clippy, 348 Rust tests (14 opt-in
ignored), Rust 1.94 all-target checks and 49 Python qualification tests passed.

The first live transfer failed before manifest publication. Its initial error
lacked structured body-retry guidance; a regression now checks sanitized retryable
body failures separately from disk errors, and the response-body budget is
120 seconds. The later HTTPS installation succeeded. The failed attempt remains
local evidence; it is not relabeled as success. The dependency audit still exits
nonzero for the two existing crypto advisories, with no new advisory from the
seven added packages. Raw logs and generated transcripts remain local-only.
