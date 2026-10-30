//! Experimental client-only composition; not selected by the production manifest.
use rustls::{
    SignatureScheme,
    crypto::{CryptoProvider, WebPkiSupportedAlgorithms},
    pki_types::SignatureVerificationAlgorithm,
};
use std::{sync::OnceLock, vec::Vec};
mod keyless;
mod rsa;

struct Catalog {
    all: Vec<&'static dyn SignatureVerificationAlgorithm>,
    mapping: Vec<(
        SignatureScheme,
        &'static [&'static dyn SignatureVerificationAlgorithm],
    )>,
}
pub(super) fn provider() -> CryptoProvider {
    let mut provider = rustls_pure::provider();
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        let mut all = provider.signature_verification_algorithms.all.to_vec();
        all.extend_from_slice(rsa::ALGORITHMS);
        let mut mapping = provider.signature_verification_algorithms.mapping.to_vec();
        mapping.extend_from_slice(rsa::MAPPING);
        Catalog { all, mapping }
    });
    provider.signature_verification_algorithms = WebPkiSupportedAlgorithms {
        all: &catalog.all,
        mapping: &catalog.mapping,
    };
    keyless::restrict(&mut provider);
    provider
}
