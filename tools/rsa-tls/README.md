# Client-only pure-Rust TLS comparison

These files are isolated qualification adapters, not production modules. Since
0.5.0 the normal manifest selects pinned upstream RustCrypto; the application
itself enforces private-key refusal and checked X25519/P-256/P-384 exchange with a patched crypto-bigint floor. The earlier
vendored snapshot remains research source/license provenance.

`provider.rs` composes rustls-pure 0.15.4 with public RSA PKCS#1/PSS verification.
`rsa.rs` retains primitive calls from the earlier vendor verifier; only import
aliases and catalogue registration differ. Original MIT/Apache notices remain
under `vendor/rustls-rustcrypto/`. `upstream.rs` delegates to the exact upstream
revision pinned in the runner. Both adapters use `keyless.rs` to refuse all
private keys; the application wrapper still restricts exchange to checked X25519.
Rustls/WebPKI retain hostname, validity, trust-anchor and signature checks.

```sh
python3 tools/qualify_rsa_tls.py --provider composition
python3 tools/qualify_rsa_tls.py --provider upstream
python3 tools/qualify_tls_protocols.py --run runs/rsa-tls-<owned-run>
```

Requires the normal Rust/Python qualification environment, provisioned e2e_rnnt
model files and network access. The first runner accepts `--model-dir`. All
source copies, targets, logs, private test keys and media stay in owned ignored
runs. Normal debug/release hashes are checked around builds/trials; do not rebuild
normal binaries concurrently. Historical shared-target scripts must not be replayed.

The application runner executes trust/signature fixtures, all release tests,
Clippy, Rust 1.94 checks, real GitHub installation and doctor, then two fresh full
acquisitions and one interrupted/resumed acquisition. Shared YouTube cooldowns
stop subsequent requests. It runs no real ASR and never changes production
dependencies. Inputs, selected worker identity and executable hashes are recorded.
The protocol runner uses OpenSSL only as an independent local test oracle. It
checks actual TLS 1.2-only and TLS 1.3-only handshakes with an explicitly provisioned
test CA; production trust is never modified. Failed probe attempts remain local.

Both candidates passed their expected scope on 0.4.0. The composition negotiates
only TLS 1.3 and resolves 286 package versions. Upstream negotiates both versions
and resolves 266; both use crypto-bigint 0.7.10 and retain one RSA advisory. The
upstream manifest's minimum RSA prerelease resolves to 0.10.0-rc.19 in the qualified
lockfile. The [comparison and scoped update](../../docs/tls-provider-scope.md)
explain selection and residual risks. Passing this harness is not independent
security review, public-distribution approval or a universal access guarantee.

The exact application-factory probe exposed a TLS1.2 ECDSA-server refusal under
X25519-only policy. 0.5.0 requires patched crypto-bigint >=0.7.6 and restores
checked P-256/P-384 exchange; strict uncompressed wire-point validation and
private-certificate-key refusal remain enforced. Forced-group protocol oracles
verify each group under TLS1.2 and TLS1.3.
