# RSA policy fixtures

Independent OpenSSL public-key, signature and certificate fixtures for the TLS
2048–8192-bit RSA policy. No private keys are stored here. Regenerate with
`python3 tools/generate_tls_fixtures.py --rsa-policy`; owned private keys remain
only in ignored `runs/tls-fixtures-*` scratch. OpenSSL is not needed for tests.

Keys have exactly 1024, 2047, 2048 and 3072 bits. The 2047-bit case detects checks
that count DER octets instead of actual modulus bits. Each key signs the message
in `parameters.json` with SHA256/384/512 and PKCS1 or PSS (digest-sized salt).
RSA-1024/PSS/SHA512 is excluded because that salt and digest cannot fit.

Each RSA CA also signs an ECDSA server certificate for `fixture.invalid`;
fixed verification time comes from `parameters.json`. Tests exercise complete
certificate-chain validation, all six RSA verifiers, their TLS handshake
mappings, strong-key acceptance and weak-key refusal. The ordinary TLS fixtures
retain malformed-input, tampered-signature, name, time and trust regressions.
