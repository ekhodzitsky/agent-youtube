# Pure-Rust TLS scope and 0.5.0 update

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

Follow-up: the [focused TLS risk review](tls-risk-assessment.md) confirms missing
RSA minimum-key policy, entropy-failure panics and ineffective AES-GCM record
limits. These were unfixed in 0.5.0 and are addressed by [0.5.1 hardening](tls-hardening.md);
the functional results below do not establish safety for those conditions.

Qualification date: 2026-10-29. Scope: controlled private operation on the current
Linux x86_64 host/network. This update is not unrestricted production-security
approval or a guarantee of future YouTube access. Repository privacy and
manual-only workflows remain unchanged.

## Decision and comparison

Select RustCrypto upstream revision
[`70f76c039e587192688af18a80d5d6435dedaf22`](https://github.com/RustCrypto/rustls-rustcrypto/tree/70f76c039e587192688af18a80d5d6435dedaf22)
with its qualified lockfile. The application blocks certificate private-key
loading and limits key exchange to checked X25519, P-256 and P-384. This avoids
maintaining the separate composition catalogue and preserves required protocol
compatibility with modern primitives.

| Isolated candidate on 0.4.0 | Composition | Upstream |
| --- | --- | --- |
| Primitive source | rustls-pure 0.15.4 plus retained public RSA verifier | pinned RustCrypto Git source |
| Provider-level TLS 1.2 oracle | Refused, as expected | Negotiated |
| Provider-level TLS 1.3 oracle | Negotiated | Negotiated |
| GitHub setup and doctor | Passed | Passed |
| Full decoded audio | 2 fresh + 1 interrupted/resumed acquisition | 2 fresh + 1 interrupted/resumed acquisition |
| Application gates | 348 release tests, Clippy, Rust 1.94 passed | Same |
| Resolved package versions | 286 | 266 |
| RSA | 0.9.10 | 0.10.0-rc.19 |
| crypto-bigint | 0.7.10 | 0.7.10 |
| Raw RustSec audit | One RSA finding; exit 1 | One RSA finding; exit 1 |

The isolated application snapshots retained their former X25519-only policy;
their provider-level protocol probe did not exercise that wrapper. Both completed
GitHub installation and all three full audio trials. The TLS1.3-only candidate's
success establishes that TLS1.3 was sufficient for those observed service routes,
not that every future route or proxy supports it. Normal debug/release executable
hashes remained unchanged throughout both isolated qualifications.

## Defect exposed by the exact application factory

A separate opt-in probe using the actual application provider refused a local
TLS1.2 ECDSA server under X25519-only policy, while raw upstream succeeded. This
failed result is retained, not counted as protocol compatibility. Restricting
advertised groups affects ECDSA-certificate compatibility in older TLS; see
[RFC 8422](https://www.rfc-editor.org/rfc/rfc8422.html#section-5.1.1).

X25519-only was a mitigation for the old crypto-bigint timing issue. The selected
graph uses 0.7.10; the direct dependency constraint requires the
[patched >=0.7.6 family](https://rustsec.org/advisories/RUSTSEC-2026-0387.html)
before NIST ECDH is enabled. Version 0.5.0 deliberately changes that wire/security
policy. It offers X25519 first, then P-256/P-384. NIST shares must have the exact
uncompressed TLS encoding; the underlying provider validates the curve point.
Compressed, truncated and off-curve points are rejected. The fixed-length
all-zero X25519 check remains. No cryptographic primitive is implemented here.

The final application probe checks actual TLS1.2 and TLS1.3 handshakes against
owned OpenSSL test oracles, including forced X25519, P-256 and P-384 exchanges.
It records the negotiated protocol and group, rather than merely accepting a
configuration builder. The wire ClientHello regression asserts exactly those
three groups in preference order; its former X25519-only expectation was updated
as part of the documented policy change, rather than removed or made permissive. Private fixture keys stay in ignored runs. Only the
probe explicitly trusts its generated CA; production trust roots and hostname,
chain, time and signature validation are unchanged.

The dependency transition also made the existing private-key-refusal regression
fail: unpatched upstream permits loading certificate private keys. Moving the
unconditional refusal into the application restores the contract independently
of dependency internals. All eleven public signature fixtures and negative
certificate checks remain active. A group-identity regression additionally caught
an X25519 constant left in the generalized exchange wrapper; it now delegates
the actual group identity. The before/after failures are retained locally.

## Security disposition and support boundary

The old crypto-bigint 0.5.5 package is absent from the selected lockfile. The
historical vendor directory is not a runtime dependency; it retains research
source/license provenance. Native crypto implementations are not selected.
Registry versions and the Git revision are locked; upgrades require qualification.
The upstream manifest's RSA minimum is rc.18, but the qualified resolution is
**rc.19**. Do not confuse a manifest range with the executable's exact graph.

The [RSA advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) remains
unpatched and raw audit still exits 1. RSA public certificate verification is
reachable; RSA signing/decryption and certificate private-key loading are not
exposed through this client provider. NIST ECDH uses secret arithmetic in the
patched family. These boundaries are not an advisory waiver or a security audit.
No advisory exclusion, trust bypass, native fallback or RSA-certificate removal
was used to obtain a cleaner report.

Upstream still labels the provider experimental and
[warns against production use](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/README.md).
Neither this update nor successful tests erase that warning. Independent review
of the configured cryptographic path and disposition of the unpatched RSA
package remain blockers to claiming unrestricted production-security readiness.
The strict pure-Rust requirement remains in force; this is a scoped improvement
of the private candidate, not a claim to have found an audited replacement.

The exercised endpoints are GitHub release setup, YouTube/player scripts and
actual media routes reached by the finite trials. Local protocol oracles are
not enterprise proxy, custom trust-store or all-network qualification. Servers
requiring other groups, client-certificate authentication or protocols outside
TLS1.2/1.3 remain outside support. Video deletion, upstream refusal, rate limits,
account requirements and route changes remain external access limitations.

## Reproduction and evidence

Use [the isolated comparison tools](../tools/rsa-tls/README.md). Each candidate
uses an owned target and records source/probe/worker identities. The final
[sanitized result record](tls-provider-scope-results.json) separates candidate
qualification, exact application-factory handshakes and deployed-artifact trials.
Media, transcripts, request logs, keys and unsanitized metadata remain local.
The initial driver was stopped before network trials when a combined-log envelope
parser issue was identified. A protocol helper also ran before report metadata
was saved; it now reads completed capability output. Those incomplete attempts
are retained separately and are not successful trials.

Only TLS dependencies, policy and validation boundaries change. CLI response,
ASR/checkpoint schemas, media backend, models and transcript bytes do not migrate.
Saved-job reuse is checked separately from new-job recognition. Future rebuilds
have their own binary identity; pre-commit qualification does not apply to an
arbitrary executable.

## Final artifact qualification

The final 0.5.0 application artifact negotiated ten loopback cases: default and
forced-group ECDSA cases under both protocol versions, plus RSA-certificate cases
under both versions. The exact application factory, including point checks and
private-key refusal, was exercised. Its fresh GitHub setup and doctor passed.
A separate process resumed a 163,880-byte committed prefix through full verified
decoding of 1,053.8376875 seconds. A fresh 52.96475-second transcription validated
canonical and exact raw ASR hashes, then cleaned owned media. A completed 0.4.0
job retained identical transcript, manifest and raw bytes when reused.

Final local gates passed: fmt, strict all-target Clippy, 349 debug and 349 release
tests, Rust 1.94 all-target check, and 52 Python qualification tests. Fifteen
opt-in Rust tests remain ignored by ordinary suites; the protocol probe is
explicitly exercised above. The selected graph resolves 266 packages and no
active native crypto package. The exact pre-commit identities and raw-audit
failure are in the result record; no GitHub Actions were run.
