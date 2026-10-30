# TLS hardening in 0.5.1

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

The three confirmed defects in the [0.5.0 review](tls-risk-assessment.md) are
fixed. The provider remains pinned to upstream revision
`70f76c039e587192688af18a80d5d6435dedaf22`; all dependency versions remain
unchanged. This is targeted remediation, not an independent cryptographic audit.

## RSA strength policy

A borrowed PKCS1 DER parser checks the RSA modulus before allocating big integers
or invoking the original verifier. Public DER is bounded to 1100 bytes; moduli
must have 2048–8192 bits. The parser removes DER sign octets, and the 2048-bit
lower boundary checks the highest actual bit rather than just byte length.
The upstream verifier still checks exponent bounds, padding, signature encoding
and signature validity. Non-RSA algorithms retain their original verification.

Both certificate-chain and TLS handshake signature tables reference the checked
verifiers. Independent public fixtures test all six RSA algorithms and handshake
mappings: reject 1024/2047-bit keys, accept 2048/3072-bit keys. Digest-sized
SHA512 PSS cannot fit in RSA-1024, so that impossible positive fixture is excluded.
Complete chains signed by the same four issuer sizes exercise policy through
WebPkiServerVerifier. Existing wrong-name, wrong-time, wrong-trust, malformed-key
and tampered-signature tests remain enabled. No certificate-validation bypass,
RSA certificate removal or certificate private-key loading was introduced.

## Fallible randomness and recovery

The application now owns only the key-exchange glue; arithmetic remains in the
same locked X25519/P-256/P-384 primitives. NIST generation uses the fallible
`Generate` API. X25519 fills a zeroizing seed through the fallible OS RNG,
constructs an owned zeroizing secret and consumes the private exchange at
completion. The library type is named `StaticSecret` because it supports
construction from bytes; the application never exposes or reuses it. Strict
wire lengths, uncompressed NIST points, on-curve checks and X25519 low-order
refusal remain in force. There is no panic-catching or substitute randomness.

Every enabled group passes deterministic partial-entropy failure tests. A Linux
x86_64 child-process seccomp test denies `getrandom` with EIO and verifies that
each **actual configured group** returns `FailedToGetRandomBytes` rather than
panicking. No host policy is changed. Both HTTP factories classify direct and
I/O-wrapped errors into sanitized retryable JSON:

- Download: `tls_random_unavailable`, preserving media for resume.
- Setup: `setup_tls_random_unavailable`, preserving normal setup ownership rules.

Download entropy failures use the existing durable retry window and backoff;
no client/source switching is needed for a local RNG outage. A regression checks
persisted scheduling and the unchanged deadline, including disabled default
retry-window behavior. Setup still requires an explicit retry. Certificate and
compatibility failures remain permanent.

## AES-GCM record budgets

Owned process-lifetime suite tables retain upstream algorithms, suite order and
protocol/authentication parameters. AES-GCM sets `confidentiality_limit` to `2^24`, triggering outgoing key
rotation or closure as in standard rustls; any stricter upstream limit is retained.
ChaCha20-Poly1305 retains its standard different `u64::MAX` policy. This controls
outgoing TLS keys and imposes no audio-duration or file-size restriction.

The independent local OpenSSL oracle exercises both protocols with RSA and
ECDSA certificates and a **test-only** four-record AES-128-GCM budget. TLS1.3
completes twelve authenticated echo round trips across automatic key updates;
the oracle observes six KeyUpdate messages, counting both directions. TLS1.2
emits three application records followed by a close alert, with no application
records after that alert. The test counts actual wire records: a successful
plaintext-buffer write alone does not establish delivery. Ordinary tests check
all six configured AES-GCM suites' production budgets; the reduced-budget wire
probe exercises AES-128-GCM, not every cipher implementation.

## Verification and retained scope

Local required gates, optimized TLS tests, MSRV 1.94 and Python qualification
tests pass. The final protocol matrix covers ten ordinary handshakes (default,
forced X25519/P-256/P-384 and RSA certificate, each under both TLS versions) plus
four reduced-budget recovery cases. Fresh YouTube audio is downloaded, fully
decoded and transcribed; fresh GitHub runtime setup verifies the other HTTP
factory. Sanitized measurements are in [the result record](tls-hardening-results.json).
Raw proofs, failed-before regressions, oracle logs and private test keys remain
local under `runs/tls-fixes-20261029/`.

The locked graph still has 266 package versions including the application.
Six existing dependencies become direct dependencies; only the X25519
`static_secrets` feature is newly enabled. No ring, aws-lc, OpenSSL, native TLS,
C compiler or CMake package is selected in the Linux normal/build graph.
OpenSSL is solely a fixture generator and independent test server.

Raw cargo audit still exits 1 for
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html).
The public-only RSA reachability assessment is unchanged: there is no local
certificate private RSA key to recover through the private-key timing attack.
No advisory waiver or claim that the RSA package is patched is made. The
[upstream experimental warning](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/README.md)
and unreviewed cryptographic risks remain disclosed. These fixes do not prove
constant-time behavior on every CPU or unconditional production security.

Weak-RSA endpoints now fail validation intentionally. Package version becomes
0.5.1; agent response/checkpoint/transcript schemas and exact ASR records are
unchanged. Existing jobs require no migration.
