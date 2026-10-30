# TLS reachability review — 2026-10-29

This is the historical pre-0.5.0 review. The [current provider comparison and
scoped update](tls-provider-scope.md) supersede its dependency identities and
vendored-provider description; the original evidence below is retained.

This review retains the strict pure-Rust production constraint. It does not
approve the current provider for production: its upstream README explicitly
warns against production use. Passing application tests and excluding known
advisory paths do not replace a cryptographic security audit.

## Current configured path

`download::http::Http::new` installs `download::tls::provider`; attestation also
uses that constructor. Certificate/hostname verification and trusted roots remain
Rustls/WebPKI defaults. The configured provider offers only checked X25519 key
exchange. Its private-key loader always refuses keys; NIST secret key exchange
and signing implementations are absent from the retained provider snapshot.

Certificate verification remains reachable on attacker-supplied public data:

| Operation | Reachable data / boundary |
| --- | --- |
| RSA PKCS#1/PSS verification | PKCS#1 public key, peer message and signature; `RsaPublicKey` and `VerifyingKey::verify`; no private key or decrypt/sign entry point |
| P-256/P-384 ECDSA verification | SEC1 public point, DER signature and public message hash; `VerifyingKey::verify_prehash`; secret ECDH excluded |
| Ed25519 verification | Public verification key and peer signature; no signing key |
| X25519 exchange | Ephemeral secret in x25519-dalek; malformed/low-order peers are refused; all-zero shared secrets are rejected |

[RSA RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
concerns private-key timing and lists no patched version.
[crypto-bigint RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html)
concerns secret integer operations and is patched in >=0.7.6. Current verification
uses public operands, but the affected dependency versions remain in Cargo.lock.
This is a reachability disposition, not an audit waiver or proof of security.

The [numeric/provenance record](tls-security-results.json) pins the inspected
upstream revision, candidate archive and public fixture hashes.

## Executable evidence

Existing tests check private-key refusal, the offered group, malformed/low-order
shares and successful X25519 agreement. New tests independently verify all eleven
configured signature algorithms against OpenSSL-generated public fixtures, then
reject modified messages/signatures, empty signatures and malformed keys.

A separate certificate-chain test accepts a valid RSA-issued P-256 server
certificate and rejects an incorrect DNS name, expired validity, an unrelated
trust anchor and a corrupted issuer signature. Verification uses the fixture's
fixed clock. The live constrained client also fetched YouTube robots.txt over
verified HTTPS. These are regression checks, not comprehensive primitive tests.

Only public certificates, keys and signatures are stored in
[the fixture set](../tests/fixtures/tls/README.md). OpenSSL is used solely as an
independent fixture-generation oracle; it is not needed to build/test/run agent-youtube.
No production dependency or TLS implementation changed in this review.

## Pure-Rust alternative actually exercised

The inspected [rustls-rustcrypto upstream](https://github.com/RustCrypto/rustls-rustcrypto)
HEAD still declares 0.0.2-alpha, uses rsa 0.10.0-rc.18 and carries the production
warning. A newer manifest is not a demonstrated production-safe replacement;
the RSA advisory also covers that release candidate.

[rustls-pure 0.15.4](https://docs.rs/rustls-pure/0.15.4/rustls_pure/) was downloaded
with its crates.io archive checksum verified and exercised in an isolated local
project. With only X25519 offered, it obtained HTTP 200 from verified YouTube
HTTPS. Its resolved research graph uses crypto-bigint 0.7.10, contains no rsa,
and has zero reported vulnerabilities in the fetched RustSec database.

However, the same valid RSA-issued fixture chain accepted by the current client
is refused with `UnsupportedSignatureAlgorithmContext`. This matches its declared
lack of RSA certificate verification. It also supports TLS 1.3 only. Its README
states that it has not had an outside security audit and recommends Rustls's
default provider when audited TLS is needed. An isolated full agent-youtube build with this provider then failed a complete short-video
trial with `download_transport` on SABR requests. A diagnostic repeat also failed
and reported `Io(InvalidData)`. A separate request to the SABR source's same CDN
origin exposed `InvalidCertificate(UnsupportedSignatureAlgorithmContext)` for
RSA PKCS#1/SHA-256. The signed media URL was not published or placed in command
arguments. Thus the earlier robots.txt success did not imply CDN compatibility.

Therefore it is not substituted into production as an equivalent provider.
The minimal probe resolves 110 packages; the isolated application candidate
resolves 261 versus 258 in the production lockfile. No new direct/transitive
runtime dependency was added to the production project. Standard native crypto
providers remain outside the owner's confirmed pure-Rust constraint.

The open production issue is a sufficiently reviewed pure-Rust provider that
preserves the required certificate/protocol compatibility, or an explicitly
qualified narrower compatibility scope. Neither this review nor a clean audit
of a different test graph establishes that result. Publication and production
security approval remain separate decisions.
