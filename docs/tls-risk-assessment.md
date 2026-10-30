# Reachable TLS risks in 0.5.0

Reviewed on 2026-10-29 against application commit `88ebbe7` and locked
RustCrypto revision `70f76c039e587192688af18a80d5d6435dedaf22`.
The three confirmed findings are remediated in [0.5.1](tls-hardening.md).
The baseline findings below describe 0.5.0.

This is a source review with focused fault injection, not an independent
cryptographic audit. It supplements [functional qualification](tls-provider-scope.md).
No production dependency, crypto implementation or release artifact changed.

## Confirmed findings

| Finding | Evidence | Impact and priority |
| --- | --- | --- |
| No TLS-specific RSA minimum modulus size | An independent OpenSSL RSA-1024/SHA256 signature is accepted by the exact application provider's RSA PKCS1 verifier | Fix first: missing certificate-key strength policy. This is a primitive-level result, not evidence of bypassing trust, hostname or chain validation. |
| Key exchange panics on OS entropy failure | Denying `getrandom` with `EIO` in disposable Linux processes makes X25519, P-256 and P-384 `start()` exit 101 through `UnwrapErr` | Fix next: operational failure can escape the typed JSON error contract. This requires an OS RNG failure; no remote trigger was established. |
| AES-GCM record limits are effectively disabled | All nine selected suites expose `u64::MAX`; six are AES-GCM. Standard rustls AES-GCM suites use `1 << 24` | Fix the provider policy: outbound key rotation/closure occurs far later than the standard provider's threshold. Practical exposure is limited by the client's small outbound requests. |

These findings remain **unfixed** in 0.5.0. Focused remediation is recorded in
the local work queue. Successful handshakes and full downloads did not exercise
these failure conditions.

### RSA verification policy

The selected [RSA adapter](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/src/verify/rsa.rs)
parses `RsaPublicKey::from_pkcs1_der`, constructs a verifying key and verifies
the signature. The locked `rsa 0.10.0-rc.19` source's `encoding.rs` delegates
public parsing to `RsaPublicKey::new`; `key.rs` enforces an 8192-bit maximum
and a bounded odd public exponent, but no 2048-bit minimum modulus.
The rustls-webpki standard RSA algorithms explicitly use 2048–8192-bit keys.

The local probe generates an owned RSA-1024 key and signs the literal message
`tls-risk-probe`. The exact production TLS module accepts the resulting PKCS1
SHA256 signature. The shared adapter structure also lacks the minimum for the
other five RSA algorithms; those five weak-key cases were not dynamically
tested in this review. An attacker would still need a chain accepted by the
trust store and the correct name, or a separate trust compromise. No such
attack was demonstrated. Reject weak keys before verification; retain normal
RSA certificate compatibility and test complete chains as well as primitives.

### Entropy failure

The selected [key-exchange implementation](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/src/kx.rs)
wraps `getrandom::SysRng` in `rand_core::UnwrapErr` for all three groups.
The locked `rand_core 0.10.1` wrapper panics on an underlying error.
Our `CheckedGroup::start` delegates to that implementation. In contrast,
the provider's separate `SecureRandom::fill` returns `GetRandomFailed`.

A process-local seccomp filter denies only the Linux x86_64 `getrandom` syscall
with `EIO`, after constructing the provider. In each of three child processes,
`SecureRandom::fill` returns an error and the subsequent group `start()` panics.
The parent process and host RNG policy are unchanged. This proves the group
failure path; it does not reproduce a naturally occurring RNG outage or establish
an end-to-end CLI panic after its earlier random draws succeed.
Use fallible RNG APIs throughout key generation and propagate an operational
error. A preflight RNG check cannot prevent a later failure; catching panics or
substituting deterministic bytes is not an acceptable production remedy.

### Record limits

The selected [suite definitions](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/src/lib.rs)
set every `confidentiality_limit` to `u64::MAX`. In locked rustls 0.23.46,
`common_state.rs::start_encryption_tls12` and `tls13/key_schedule.rs` pass this
limit to the record encrypter. `record_layer.rs` applies its sequence ceiling,
which is still vastly above the standard AES-GCM threshold. The standard
provider's `crypto/ring/tls12.rs` and `tls13.rs` use `1 << 24` for AES-GCM;
ChaCha20-Poly1305 intentionally has a different policy.

This field controls **our outgoing records**, not a maximum audio-file size.
Most bytes during audio acquisition arrive from the server, whose encryption
key rotation is its own responsibility. The finding does not imply that a
multi-hour audio download crosses this threshold. Remediation should retain
unbounded recording duration, restore finite AES-GCM policy and exercise real
TLS1.3 key updates / TLS1.2 closure at reduced test thresholds.

## Advisory scope and existing protections

Raw `cargo audit --json` still reports only
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html),
for `rsa 0.10.0-rc.19`. No advisory suppression was added. The advisory concerns
private-key leakage through timing; there is no patched-version designation.
This client loads no certificate private keys, performs no RSA signing or
decryption, and negotiates ECDHE rather than RSA key transport. The reviewed
certificate verification path uses public RSA operations. Consequently, a
private RSA key to recover through that advisory is absent from this path.
That reachability assessment neither patches the package nor establishes the
safety of every public parsing/verification operation. The weak-key policy
finding above is separate from the Marvin advisory.

Both HTTP factories (`download/http.rs`, `runtime/install.rs`) explicitly select
the same application provider. Neither configures a trust bypass or client
certificate. Certificate/name/time checks and malformed-share rejection have
existing regression coverage. ECDH still handles private ephemeral secrets:
disabling certificate-key loading does not remove all secret arithmetic.
Patched `crypto-bigint 0.7.10` remains selected. No native crypto fallback was
introduced. This review did not prove constant-time execution on every CPU,
exhaustively fuzz certificate parsing, or perform a new dependency-wide audit.

Upstream still says
[“DO NOT USE THIS IN PRODUCTION”](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/README.md).
Its warning cites incomplete suite coverage and lack of formal/FIPS assurances.
Those are different from a demonstrated exploit; FIPS certification is not a
project requirement. The current findings show why functional compatibility
alone is insufficient to dismiss the warning. Fix and qualify the concrete
paths first; broader security confidence remains a separate review question.

## Reproduction and evidence boundaries

Local-only evidence is retained under `runs/tls-review-20261029/`: audit output,
the disposable probe's source/lockfile/build log, owned oracle fixtures and
`results.json`. Private fixture keys are ignored and are not shared records.
The probe copies `src/download/tls.rs` unchanged; both copies have SHA-256
`2f64166cc4704e9370a07f1de8f06f78a636eaf505a4ea7096e97a028a8c869f`.
Its application error-construction shim does not affect successful provider
construction, key exchange or signature verification. Dependencies resolve
offline from the production lock into an isolated target directory.

Re-run the normal probe with its RSA public key and signature paths, or with
`entropy 0`, `entropy 1`, `entropy 2` for each checked group. Entropy probes are
Linux x86_64 only and intentionally exit 101 on this unfixed baseline.
OpenSSL is solely the independent signature oracle; it is not a runtime
dependency. No new real-network acquisition was needed to confirm these paths.
The pinned upstream commit was also the repository's current HEAD when checked.
