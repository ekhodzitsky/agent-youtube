# Managed recognizer runtime — 0.4.0

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

`agent-youtube setup` prepares a local pinned GigaSTT installation and returns JSON.
`doctor` and new transcription jobs select it automatically from
`tools-local/runtime` relative to the working directory. Explicit `--runtime-dir`
selects another prepared root; use an absolute path when invoking elsewhere.
An explicit `--gigastt` bypasses managed discovery. `resume` retains saved options.

## Trust and publication

The installer currently supports Linux x86_64 GNU only. It pins the upstream
[GigaSTT 2.22.0 release](https://github.com/ekhodzitsky/gigastt/releases/tag/v2.22.0)
archive SHA-256
`cd790191a600df48aa5c56e124953322af89df4bbd60d2c980436b0597c46194`
and executable SHA-256
`efb3c09b0ddf7872fe47bccc91f556ae728cf01035a94efe6f8e0c0795591cd1`.
These identify the previously qualified artifact; no minisign verification or
security approval is implied. The manifest is not an executable update channel.

HTTPS uses the application's existing checked pure-Rust TLS provider and trust
validation. Redirects must remain HTTPS on explicit GitHub release hosts, with
no URL credentials. Requests have timeouts and a redirect bound. Server HTTP
refusal reports status and parsed Retry-After seconds without response bodies
or signed URLs. Setup does not automatically retry. An interrupted download is
not published as the cached archive; rerun setup to transfer it again.

Archive handling is streamed with a 32 MiB compressed and 128 MiB expanded bound.
Only the regular executable and four expected license/notice/readme/changelog
files are accepted; no unpack API, links, device entries, subdirectories or
arbitrary archive paths are used. The gzip trailer is consumed for CRC checking.
The binary pin is verified before making it executable. These bounds apply to
the fixed installer release, not recording length or media downloads.

New runtime roots use private permissions, an ownership marker and an OS lock.
Unknown nonempty directories and linked roots are refused. Individual artifacts
are published durably without overwriting existing content. Setup can validate
and reuse matching files left by an interrupted installation; changed files are
preserved and refused. The manifest is published last, after model fingerprints
are rechecked. Existing complete manifests are validated and never rewritten.
Prepare a new root to select another model configuration.

## Models and readiness

Setup requires the four readable nonempty e2e_rnnt files listed in
[installation](installation.md). It records their actual SHA-256 fingerprints;
it does not fetch models, change files or assume all identically named releases
are equivalent. The qualified local encoder differs from the encoder in the
GitHub model-release checksum list, so silently replacing it would change saved
job identity. Subsequent managed selection verifies the recorded bytes.

Successful setup proves file integrity and preparation, not ONNX compatibility,
model loading, speech accuracy or YouTube access. Those checks are explicitly
`not_checked` in its result. Doctor checks runnable CLI capabilities and model
readability; a real transcription is needed to exercise inference. Full ASR JSON
and canonical transcripts remain under the existing retention/publication rules.

## Compatibility and tests

A nonexistent optional default runtime root retains legacy PATH behavior.
A present incomplete/corrupt root fails with `runtime_invalid` before creating a
job, rather than silently invoking another binary. Explicit recognizer overrides
remain usable. `--gigastt` and `--runtime-dir` cannot be combined. Saved-job resume
never performs discovery, including when the default runtime is broken.

Tests cover absent/incomplete/linked roots, foreign-directory preservation,
archive integrity, linked/duplicate/missing/traversal entries, truncated gzip,
matching extraction reuse, changed artifact refusal, redirect trust boundaries,
TLS/URL diagnostic redaction and explicit/saved recognizer selection. The
[sanitized qualification record](runtime-setup-results.json) captures live setup,
reuse, doctor, full transcription, old-job reuse and local gates. Raw operational
files stay in ignored local storage. No repository publication or hosted CI is
part of preparation.

The two new direct dependencies, flate2 1.1.10 and tar 0.4.46, add seven locked
package versions. flate2 uses its Rust miniz_oxide backend; tar's default features
are disabled. The installation path requires no native decompressor. This does
not resolve the existing TLS security advisories or make external ASR runtimes
pure Rust.

## Local qualification — 2026-10-29

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
