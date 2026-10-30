//! Isolated upstream RustCrypto adapter, not selected by the production manifest.
mod keyless;
pub(super) fn provider() -> rustls::crypto::CryptoProvider {
    let mut provider = rustls_rustcrypto::provider();
    keyless::restrict(&mut provider);
    provider
}
