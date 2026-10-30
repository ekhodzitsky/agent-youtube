# Public TLS verification fixtures

Generated with `python3 tools/generate_tls_fixtures.py` using OpenSSL as an
independent signing/certificate oracle. Only public keys, signatures, a message,
certificates and a fixed verification time are retained. Private keys stay in
owned ignored `runs/tls-fixtures-*` scratch and are never production credentials.
OpenSSL is needed only to regenerate fixtures, not to build or run agent-youtube/tests.

Signature files follow the pinned provider's `all` order: P-256/SHA-256,
P-256/SHA-384, P-384/SHA-256, P-384/SHA-384, Ed25519, RSA PKCS#1 v1.5 with
SHA-256/384/512, and RSA-PSS with SHA-256/384/512 (digest-sized salt). EC points
are uncompressed SEC1, Ed25519 is a raw public key, RSA is PKCS#1 DER.

The root certificate uses RSA-2048; the leaf uses P-256 with the reserved
`fixture.invalid` DNS name and serverAuth EKU. Its fixed valid time is recorded
in parameters.json, so normal tests do not depend on certificate wall-clock
expiry. Tests must reject a wrong name, expired time, wrong trust anchor and
corrupted issuer signature. These are synthetic project fixtures with no
recordings, user identities, real server keys or external credentials.
