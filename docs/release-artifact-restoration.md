# Release artifact restoration and isolated TLS qualification

Qualification date: 2026-10-29. Scope: this Linux x86_64 host and its current
network. This is private operational evidence, not an availability guarantee or
public distribution approval.

## Artifact incident and correction

An earlier research build shared the normal Cargo target directory and replaced
`target/release/agent-youtube` with an RSA-free experimental executable. Its SHA-256 was
`155cce11c5978bdb113bcdc1f5cad03b724a4b905608cf9d032b069894e0d8f8`.
The executable contained a research diagnostic marker and failed a real CDN TLS
handshake. The source manifest had not selected that provider. This was an
artifact isolation error in our qualification work, not evidence that the
production provider had lost RSA support. The bad executable is retained locally
as non-executable incident evidence.

Qualification builds now use owned target directories under ignored `runs/`.
The RSA runner additionally compares normal debug/release executable hashes before
and after builds and trials. Do not concurrently rebuild normal binaries during
that sentinel check. Historical one-off scripts are frozen evidence and must not
be replayed. Codec and vendor qualification runners also use isolated targets.

A nested source snapshot previously inherited the parent repository's revision
and clean status. The build script now accepts Git identity only when the Git
root is the actual source root. A standalone before/after probe demonstrated
`source_dirty: null` and unavailable revision for the nested snapshot. Its source
fingerprint still identifies build inputs; unavailable Git metadata is not a
clean-tree assertion.

## Working release 0.3.14

The production provider and certificate/hostname verification remain enabled.
The local qualified executable SHA-256 is
`60d2d363913f94ace08c2fc6c58ac09bb4e984a6e0337826750519a9818baf85`.
It was built before commit: revision `8fcc861`, `source_dirty: true`, source
fingerprint `3d1c61594748291d34588bf98f6b35d147a089fa`. A later rebuild may have a
different binary identity; do not transfer these results to an arbitrary binary.

A fresh full transcription of the affected 253-second video completed:
252.842125 seconds of decoded audio, one contiguous processing chunk, validated
canonical transcript and retained raw ASR hashes. Owned media/chunks were empty
after durable publication. The restored 0.3.13 production build also completed
this source. The qualified 0.3.14 executable reused that completed job without
changing canonical transcript bytes. Media backend and checkpoint schemas are
unchanged. These are completeness/publication checks, not a speech accuracy
assessment. Raw transcripts and request logs remain local.

Typed Rustls certificate and peer compatibility failures now return
`tls_certificate_invalid` or `tls_peer_incompatible`, `retryable: false`, and a
static sanitized reason. Regression tests cover direct and wrapped Rustls errors,
private peer-context redaction and one attempt with no retry delay. Ordinary
transient network failures retain their retry policy. No trust bypass or
exception for the affected host was added.

Local gates passed: fmt, strict all-target Clippy, 334 debug tests, 334 release
tests, Rust 1.94 locked all-target check, and 42 Python qualification tests.
Fourteen opt-in Rust tests remain ignored. A test-only ASR watcher correction
accounts for its existing polling fallback when OS notifications are unavailable;
production recognition behavior was not changed.

## Modern pure-Rust composition: qualified prototype only

[The reproducible runner and adapter](../tools/rsa-tls/README.md) combine
rustls-pure 0.15.4 with the retained public RSA PKCS#1/PSS verification calls.
The key loader unconditionally refuses private keys. Rustls/WebPKI continue to
validate hostname, validity, chain and signatures; the application keeps checked
X25519 exchange. No new cryptographic primitive is implemented here.

The candidate passes six TLS trust/signature tests, the 334-test release suite,
Clippy and Rust 1.94 checks. Two fresh real full acquisitions (about 53 and 253
seconds) and one interrupted/resumed 1,054-second acquisition completed with
full decode validation. The resume probe checks the committed prefix rather than
merely obtaining a new URL. Normal executable hashes remained unchanged.
These acquisition probes do not perform ASR. Early failed runs (snapshot module
placement and the notification-dependent test assumption) remain local evidence;
they are not counted as successful trials.

The capability probe accepts TLS 1.3 configuration but rejects TLS 1.2-only
configuration. This loss of compatibility is a reason to retain the production
provider. The prototype is not selected by the production manifest.

The inspected candidate graph resolves 279 package versions versus 258 for the
baseline. Linux normal/build selection uses 251 versus 232. No native crypto
package is active in that selection; optional inactive `ring` and `cc` entries
still exist in resolution. crypto-bigint moves from 0.5.5 to 0.7.10. Raw RustSec
audit still exits nonzero for rsa 0.9.10 (RUSTSEC-2023-0071); it does not become
clean because this adapter only verifies public signatures. Neither the glue nor
the upstream provider has received an external security approval here. This
qualification does not remove the existing production security/distribution
prerequisites in [the security review](tls-security-review.md).

Sanitized machine-readable identities and outcomes are in
[the result record](release-artifact-results.json). Raw reports, lockfiles,
command logs, failed runs and exact output hashes are retained locally. No
GitHub Actions, repository publication or other-project changes were required.
