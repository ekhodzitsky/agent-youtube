# Retained dependency forks and assets

syom is no longer vendored from agent-youtube 0.16.0. Cargo selects unmodified
upstream 0.10.0 source candidate at `1383825e013f95ee565eb094122f3ec6fa2f7193`.
Its public persistent-state and completion APIs replace our former local delta.
This candidate is not a published registry release. See the
[consumer migration](../docs/syom-upstream.md). The former syom patches remain
historical review artifacts under `tools/upstream-codecs/`; they are not applied.

## bandlimit 0.1.7

The retained snapshot is from clean local revision
`f9bed39b532ab23e5030ded23c7d60e45d5afa09`, with bundled SHA-256
`0bb9f1caa31ec46697d08e94d9ee00f761359e646b9c91fb72dbf2fe453d36f1`.
The hash covers paths and contents in lexical order as path + NUL + contents.
Manifests omit development/benchmark dependencies and disable upstream test
targets; test corpora are not bundled. There are zero runtime dependencies.
Both upstream licenses are retained.

Local changes:

- `cascade_down.rs`: remove a discarded tuple that moved a mutable slice before
  later use and prevented Linux x86_64 compilation.
- `oneshot.rs`: import `UnsafeCell` only on macOS, matching its uses.
- `stream.rs`: serialize/restore write-mode filter history, position and pending
  output; reconstruct coefficient tables and validate rates, lengths and samples.

Three existing Linux dead-code warnings (`prefer_p_cores`, `phase_f64`, `FULL_48`)
remain. Do not replace this fork until upstream provides equivalent qualified
resampler checkpoints. Codec updates require exact PCM/restart comparisons and
an explicit backend identity decision. Do not edit sibling source repositories.

ryf is a normal crates.io dependency pinned to `=0.7.3` with its archive checksum
in Cargo.lock. Only SIMD is enabled; optional ADPCM and C benchmark builds stay
disabled. It is not duplicated here.

# Web runtime sources

The application also includes the fixed DOM bundle and all third-party notices in
[botguard/README.md](botguard/README.md). The corrected Boa 0.22 source copies are
shared with the isolated experiment at
[its vendor record](../experiments/botguard/vendor/README.md). They replace the
name-specific diagnostic bypass with per-invocation eval binding state and correct
direct-eval arguments analysis. Main and experiment lockfiles pin their separate
graphs; neither runs a native JS engine or an external JS executable.

# Historical client crypto snapshot

Since 0.5.0 this snapshot is not selected by the production manifest. The application
uses pinned upstream Git source and enforces private-key refusal in its own TLS
adapter. This directory retains source/license provenance for the isolated public
RSA comparison adapter; the record below describes the earlier snapshot. See
[the current comparison](../docs/tls-provider-scope.md).

`rustls-rustcrypto` 0.0.2-alpha was copied from the crates.io source on 2026-10-28.
MIT and Apache-2.0 licenses are retained. Snapshot SHA-256: `36bd7595ba6c9bf8fd933bd4539d6c805922b78cb956f573fc1ba3b30c18e375`,
using the same path/NUL/content algorithm as the codec snapshots.

Local changes: obtain algorithm identifiers from `rustls-pki-types`, removing the
obsolete webpki dependency; remove unused signing modules and NIST ECDH entry
points; reject private-key loading; remove unused direct pkcs8/sec1 and PEM/ECDH
features; replace paste with the API-compatible pastey 0.2.3 proc-macro (already in the lockfile, zero
transitive dependencies). Cipher suites and certificate signature verification
remain upstream implementations. The application still rejects all-zero X25519
results. No cryptographic primitive was rewritten. The source fingerprint includes
this snapshot. See docs/development.md for residual advisory scope and tests.

## Qualification and historical patches

The default `tools/qualify_vendors.py --crate syom` runner archives the exact
selected upstream revision and runs its own tests without an overlay. bandlimit
still uses the qualified runtime overlay. Prepared codec patches are historical
handoff artifacts; `--prepared-patches` exercises their original pinned bases.
The [codec handoff](../tools/upstream-codecs/README.md) preserves their evidence.

Keep vendored code only while required changes are unavailable upstream.
The two Boa copies and bandlimit still carry functional changes. The old TLS
snapshot is research provenance; the selected TLS copy has the manifest patch
below. syom no longer needs a local fork.

# Current TLS provider manifest patch — 0.6.1

Production selects `rustls-rustcrypto-current`, copied from upstream Git revision
`70f76c039e587192688af18a80d5d6435dedaf22`. It retains both upstream licenses and
README including the production warning. `upstream.json` records original file
SHA-256 values. All Rust source bytes are unchanged; only Cargo.toml disables
publication and aliases `paste` to the previously used `pastey =0.2.3` proc macro.
This removes RUSTSEC-2024-0436 from the selected build graph without a new
cryptographic implementation or an RSA advisory waiver. The application still
enforces private-key refusal, checked/fallible key exchanges, RSA key strength
and finite AES-GCM record policy. Source equality, dependency-policy and actual
TLS oracle tests cover the change. Return to upstream when this manifest patch
is available there and matching behavior is qualified. The older crypto directory
above remains research provenance and is not silently upgraded.

The retained research provider adapter also uses the current application name in
its private-key rejection message. This is diagnostic text, not a cryptographic
change; the selected current provider Rust sources remain upstream-identical.
