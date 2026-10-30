// Public-signature adapter retained from vendor/rustls-rustcrypto/src/verify/rsa.rs.
// Primitive verification is unchanged. Original MIT/Apache notices remain under
// vendor/rustls-rustcrypto/; this module is compiled only in the isolated prototype.
use pastey::paste;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::signature::Verifier;
use rsa::{RsaPublicKey, pkcs1v15, pss};
use rustls::pki_types::alg_id;
use rustls::pki_types::{AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm};
use sha2::{Sha256, Sha384, Sha512};

macro_rules! impl_generic_rsa_verifer {
    (
        $name:ident,
        $public_key_algo:expr,
        $signature_alg_id:expr,
        $verifying_key:ty,
        $signature:ty
    ) => {
        paste! {
            #[allow(non_camel_case_types)]
            #[derive(Debug)]
            struct [<RsaVerifier_ $name>];

            impl SignatureVerificationAlgorithm for [<RsaVerifier_ $name>] {
                fn public_key_alg_id(&self) -> AlgorithmIdentifier {
                    $public_key_algo
                }

                fn signature_alg_id(&self) -> AlgorithmIdentifier {
                    $signature_alg_id
                }

                fn verify_signature(
                    &self,
                    public_key: &[u8],
                    message: &[u8],
                    signature: &[u8],
                ) -> Result<(), InvalidSignature> {
                        let public_key = RsaPublicKey::from_pkcs1_der(public_key).map_err(|_| InvalidSignature)?;
                        let signature = <$signature>::try_from(signature).map_err(|_| InvalidSignature)?;
                        <$verifying_key>::new(public_key)
                            .verify(message, &signature)
                            .map_err(|_| InvalidSignature)
                }
            }

            pub const $name: &dyn SignatureVerificationAlgorithm = &[<RsaVerifier_ $name>];
        }
    };
}

impl_generic_rsa_verifer!(
    RSA_PKCS1_SHA256,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PKCS1_SHA256,
    pkcs1v15::VerifyingKey<Sha256>,
    pkcs1v15::Signature
);
impl_generic_rsa_verifer!(
    RSA_PKCS1_SHA384,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PKCS1_SHA384,
    pkcs1v15::VerifyingKey<Sha384>,
    pkcs1v15::Signature
);
impl_generic_rsa_verifer!(
    RSA_PKCS1_SHA512,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PKCS1_SHA512,
    pkcs1v15::VerifyingKey<Sha512>,
    pkcs1v15::Signature
);

impl_generic_rsa_verifer!(
    RSA_PSS_SHA256,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PSS_SHA256,
    pss::VerifyingKey<Sha256>,
    pss::Signature
);
impl_generic_rsa_verifer!(
    RSA_PSS_SHA384,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PSS_SHA384,
    pss::VerifyingKey<Sha384>,
    pss::Signature
);
impl_generic_rsa_verifer!(
    RSA_PSS_SHA512,
    alg_id::RSA_ENCRYPTION,
    alg_id::RSA_PSS_SHA512,
    pss::VerifyingKey<Sha512>,
    pss::Signature
);

pub const ALGORITHMS: &[&dyn SignatureVerificationAlgorithm] = &[
    RSA_PKCS1_SHA256,
    RSA_PKCS1_SHA384,
    RSA_PKCS1_SHA512,
    RSA_PSS_SHA256,
    RSA_PSS_SHA384,
    RSA_PSS_SHA512,
];
pub const MAPPING: &[(
    rustls::SignatureScheme,
    &[&dyn SignatureVerificationAlgorithm],
)] = &[
    (
        rustls::SignatureScheme::RSA_PKCS1_SHA256,
        &[RSA_PKCS1_SHA256],
    ),
    (
        rustls::SignatureScheme::RSA_PKCS1_SHA384,
        &[RSA_PKCS1_SHA384],
    ),
    (
        rustls::SignatureScheme::RSA_PKCS1_SHA512,
        &[RSA_PKCS1_SHA512],
    ),
    (rustls::SignatureScheme::RSA_PSS_SHA256, &[RSA_PSS_SHA256]),
    (rustls::SignatureScheme::RSA_PSS_SHA384, &[RSA_PSS_SHA384]),
    (rustls::SignatureScheme::RSA_PSS_SHA512, &[RSA_PSS_SHA512]),
];
