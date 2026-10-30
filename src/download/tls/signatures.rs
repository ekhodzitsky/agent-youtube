//! Apply the same RSA strength policy to certificate chains and handshake signatures.
use pkcs1::der::Decode;
use rustls::{
    SignatureScheme,
    crypto::WebPkiSupportedAlgorithms,
    pki_types::{AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id},
};
use std::sync::OnceLock;

type Algorithm = &'static dyn SignatureVerificationAlgorithm;
type Mapping = (SignatureScheme, &'static [Algorithm]);

#[derive(Debug)]
struct CheckedSignature(Algorithm);
impl SignatureVerificationAlgorithm for CheckedSignature {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.0.public_key_alg_id()
    }
    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.0.signature_alg_id()
    }
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        if self.public_key_alg_id() == alg_id::RSA_ENCRYPTION {
            // Bound DER parsing before any allocation or big-integer arithmetic.
            if public_key.len() > 1100 {
                return Err(InvalidSignature);
            }
            let key = pkcs1::RsaPublicKeyRef::from_der(public_key).map_err(|_| InvalidSignature)?;
            let modulus = key.modulus.as_bytes();
            // UintRef strips the sign octet. A 256-byte integer can still be 2047 bits.
            if !(256..=1024).contains(&modulus.len())
                || (modulus.len() == 256 && modulus.first().is_none_or(|first| *first < 128))
            {
                return Err(InvalidSignature);
            }
        }
        // The original verifier still enforces exponent, encoding, padding and signature validity.
        self.0.verify_signature(public_key, message, signature)
    }
}

pub(super) fn checked(
    source: WebPkiSupportedAlgorithms,
) -> crate::Result<WebPkiSupportedAlgorithms> {
    // Rustls requires static references. These bounded tables live for the process,
    // retain their owners and are initialized once from the pinned upstream table.
    static CHECKED: OnceLock<Vec<CheckedSignature>> = OnceLock::new();
    let checked = CHECKED.get_or_init(|| source.all.iter().map(|a| CheckedSignature(*a)).collect());
    static ALL: OnceLock<Vec<Algorithm>> = OnceLock::new();
    let all = ALL.get_or_init(|| checked.iter().map(|a| a as Algorithm).collect());
    static GROUPS: OnceLock<Vec<(SignatureScheme, Vec<Algorithm>)>> = OnceLock::new();
    let groups = source
        .mapping
        .iter()
        .map(|(scheme, algorithms)| {
            let algorithms = algorithms
                .iter()
                .map(|original| {
                    checked
                        .iter()
                        .find(|a| std::ptr::eq(a.0, *original))
                        .map(|a| a as Algorithm)
                        .ok_or_else(|| {
                            crate::protocol::failure(
                                "tls_provider_invalid",
                                Some(false),
                                "unmapped TLS signature verifier",
                            )
                        })
                })
                .collect::<crate::Result<Vec<_>>>()?;
            Ok((*scheme, algorithms))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let groups = GROUPS.get_or_init(|| groups);
    static MAPPING: OnceLock<Vec<Mapping>> = OnceLock::new();
    let mapping = MAPPING.get_or_init(|| {
        groups
            .iter()
            .map(|(scheme, algorithms)| (*scheme, algorithms.as_slice()))
            .collect()
    });
    Ok(WebPkiSupportedAlgorithms { all, mapping })
}
