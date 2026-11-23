# Sitta acquisition adapter

The downloader has an opt-in external acquisition backend for the stable Sitta
schema-v1 CLI on Linux amd64. The native Rust downloader remains the default and
its behavior is unchanged. This adapter does not stabilize Sitta's Go library API
or either program's private checkpoint formats.

Live handoff and production acceptance are separate evidence gates. Offline adapter
tests do not establish YouTube acceptance, availability or token enforcement.

## Enable and negotiate

Set `AGENT_YOUTUBE_SITTA` to the Sitta executable before constructing the downloader:

```sh
export AGENT_YOUTUBE_SITTA=/path/to/sitta
```

Construction runs `sitta --version` and `sitta help` with ten-second bounds. It
requires schema version 1, a nonempty Sitta build identity and the `download`
command. Invalid, missing, multiple-document or incompatible responses are refused.
Batch jobs clone only this configuration; they do not share an acquisition process.

No selection is made when the variable is absent. An empty value or failed
negotiation is an error, never a silent native fallback.

## Full acquisition lifecycle

A full `Downloader::download` invokes one bounded child:

```sh
sitta download --output PREFIX.m4a --timeout 1800000ms VIDEO_ID
```

The existing `process::Runner` supplies a 30-minute-plus-15-second outer deadline,
cancellation polling, bounded private stdout/stderr capture and process-group
reaping. The outer deadline leaves time for Sitta's final error response and worker
teardown. Cancellation terminates the owned process group; it never causes native
acquisition to start.

The adapter accepts additive JSON fields but requires schema version 1 and exactly
one final stdout document. Progress must be schema-v1 JSONL. Valid public stages are
mapped into the existing job progress lifecycle. Raw private error messages are not
forwarded: public errors map to interrupted, timeout, cooldown or acquisition failure
with sanitized cause codes and retry metadata.

Sitta owns `.partial`, `.resume.json`, `.resume.lock` and optional anonymous leases.
The adapter does not edit, import or infer success from those files. An error or
malformed response never adopts a partial and never falls back to native media.
Existing output and recovery evidence are preserved.

Before returning `PREFIX.m4a`, the adapter independently requires:

- `media.verified:true` and the exact requested output path;
- a nonempty regular file, not a symlink;
- exact byte count in the receipt and on disk;
- a lowercase SHA-256 receipt that matches the file digest.

A late Sitta error can still leave a complete visible output. The caller must inspect
that output rather than assuming error implies absence, matching the CLI contract.

## Scope boundaries

`download_range` and the native metadata/ASR paths remain native. The adapter does
not consume or publish Sitta partial prefixes, and it does not mix native and Sitta
media. It does not change model selection, decoding, transcript formats or batch
manifest behavior.

The captured process files are private implementation diagnostics, not a public
reporting API. Their names are unique and non-clobbering. Runner currently caps each
captured stream at 1 MiB; larger diagnostics fail instead of becoming an accepted
result.

## Offline verification

Build the real Sitta `cli.Run` contract process and run the adapter tests:

```sh
go -C /path/to/sitta test -c ./internal/cli -o /tmp/sitta-contract.test
AGENT_YOUTUBE_SITTA_CONTRACT_BINARY=/tmp/sitta-contract.test \
  cargo test download::sitta -- --nocapture
```

The local contract process covers full success, sanitized failure, cancellation,
timeout and same-output restart. Fake-CLI tests cover schema evolution and invalid
output validation. The complete required checks are:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

No YouTube request is made by these tests. Real live handoff requires a separate
owner-authorized bounded qualification.
