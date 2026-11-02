# Retained local codec forks

Runtime source is pinned from the owner's sibling projects. syom was refreshed on
2026-11-02; bandlimit retains its earlier qualified snapshot.
No changes were made to those projects. Upstream Rust sources and license files
are retained verbatim except for the local patches and checkpoint extensions below.
Manifests omit development/benchmark dependencies and disable upstream test targets;
test source files and corpora are not included. Both have zero runtime crates.
The source repository is public; crate publication remains disabled.

- `syom` 0.9.0 unreleased source candidate: clean upstream revision `60199081659c9fe8ee9bdf7bc07f304d5bf3c8b6` plus the required local checkpoint/completion patch. Bundled snapshot SHA-256 `8375f85d182b3bbe0269a12a92c12908b9d05b138534754c66db97b57145fe12`. `syom/upstream.json` records original/bundled file hashes and the patch digest. This is not a claimed 0.9.0 registry release.
- `bandlimit` 0.1.7: local HEAD `f9bed39b532ab23e5030ded23c7d60e45d5afa09` (clean working tree at import); bundled snapshot SHA-256 `0bb9f1caa31ec46697d08e94d9ee00f761359e646b9c91fb72dbf2fe453d36f1`.

`ryf` is a normal crates.io dependency since agent-youtube 0.3.12, pinned to `=0.7.3`
with its archive checksum in Cargo.lock. All 40 copied library source files were
byte-identical to that release. `default-features = false, features = ["simd"]`
preserves the previous feature set: ADPCM and the optional C benchmark build are
disabled, and no runtime/build dependency is added. The published build script
only emits rebuild hints with these features. Its source is no longer duplicated
here; the application tests exercise the registry package directly.

Hashes cover bundled file paths and contents (including minimized manifests and the
patches below), concatenated in lexical path order as path + NUL + contents.
Paths are relative to each crate directory, with forward slashes. The syom hash
was reverified for this refresh on 2026-11-02; bandlimit retains its verified
snapshot. Historical syom/bandlimit records initially omitted checkpoint extensions
introduced by private commit `5e31ab2`; those records were corrected earlier.

Local syom correctness patch:

- `stream/m4a.rs`: reject an unfinished priming skip or bounded presentation
  window at decoder EOF. The 2026-10-28 qualification adds a shared
  `isomp4.rs` rounding predicate and `stream/m4a_pos.rs` completion check:
  after all indexed AUs, an edit-list residual must be smaller than both one
  movie tick and one millisecond. Actual PCM is never padded. Existing accepted
  PCM and checkpoint formats are unchanged. Open-ended fragmented edits remain supported. The
  earlier pipeline backend identity was bumped to v2 so unfinished v1 runs could not reuse
  potentially incomplete decoded audio or an incorrectly selected source track.
  This patch changes only the vendored snapshot, not the sibling project.

Local bandlimit portability patches:

- `cascade_down.rs`: remove a discarded tuple that moved a mutable slice before its
  later use on non-aarch64 targets (Linux x86_64 failed to compile).
- `oneshot.rs`: import `UnsafeCell` only on macOS, matching its uses.

Three existing Linux dead-code warnings in bandlimit (`prefer_p_cores`, `phase_f64`,
`FULL_48`) are retained; they do not fail agent-youtube's clippy checks.

Local exact-state checkpoint extensions (introduced by private commit `5e31ab2`,
ported onto the current syom revision):

- `syom/src/engine/{fb_pool,filterbank,pns}.rs` and
  `syom/src/stream/m4a_pos.rs`: serialize/restore qualified AAC-LC mono/stereo
  overlap, window, noise-generator and seek-presentation state. Stateful AAC
  extensions and unqualified presentations retain the exact replay fallback.
- `bandlimit/src/stream.rs`: serialize/restore write-mode filter history,
  position and pending output; rebuild deterministic coefficient tables and
  validate restored rates, lengths and finite samples.
- These are local API extensions, not features of unmodified upstream. The new
  syom exact seek position is a replay-based logical position, not our serialized
  codec checkpoint. Our extension refuses exact-mode/failed state; it must not
  bypass upstream replay requirements. Exact PCM and resume corruption tests live
  in agent-youtube's test suite. The refreshed backend has a distinct v3 identity.

Avoid other codec implementation edits here. Refresh from the local projects when needed,
update this provenance and the media backend fingerprint, and rerun agent-youtube checks.


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

Codec qualification uses the revisions above. The default syom overlay runner
now archives the exact 0.9.0 candidate revision, overlays the bundled runtime and
uses its matching library tests. The earlier 0.7.1 snapshot used test revision
`997bf56efda8700792dbc4b0136a36ad90eca328`; that is historical evidence. The
0.8.1 prepared patch remains a historical handoff artifact. The current delta is
[syom-0.9.0.patch](../tools/upstream-codecs/syom-0.9.0.patch), and application
migration evidence is in [the upgrade qualification](../docs/syom-upgrade.md).

Retain these forks only while their required changes are unavailable in the
selected upstream package. In particular, the current syom 0.9.0 source
candidate still lacks our serialized seek-decoder checkpoint API. bandlimit's pinned upstream
source lacks the resampler checkpoint API. Boa carries the functional changes described above. The historical crypto
snapshot is research provenance rather than a selected runtime fork. Public availability alone does not make
these packages equivalent replacements. Move each back to a registry release
after matching its required behavior and verifying recovery; do not retain
unmodified copies solely for version pinning.

Prepared upstream patches, regression evidence and migration blockers are in
[the codec handoff](../tools/upstream-codecs/README.md). They are review artifacts;
production does not compile against the historical prepared artifacts; the current
0.9.0 delta documents the selected vendored extension.

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
