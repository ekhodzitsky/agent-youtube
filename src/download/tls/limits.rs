//! Retain upstream cipher implementations while enforcing AES-GCM record budgets.
use rustls::{
    CipherSuite, CipherSuiteCommon, SupportedCipherSuite, Tls12CipherSuite, Tls13CipherSuite,
};
use std::sync::OnceLock;

pub(super) const AES_GCM_RECORD_LIMIT: u64 = 1 << 24;

pub(super) enum OwnedSuite {
    Tls12(Tls12CipherSuite),
    Tls13(Tls13CipherSuite),
}
impl OwnedSuite {
    pub(super) fn supported(&'static self) -> SupportedCipherSuite {
        match self {
            Self::Tls12(s) => SupportedCipherSuite::Tls12(s),
            Self::Tls13(s) => SupportedCipherSuite::Tls13(s),
        }
    }
}

fn aes_gcm(suite: CipherSuite) -> bool {
    matches!(
        suite,
        CipherSuite::TLS13_AES_128_GCM_SHA256
            | CipherSuite::TLS13_AES_256_GCM_SHA384
            | CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
            | CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
            | CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
            | CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
    )
}

pub(super) fn copy_with_limit(source: SupportedCipherSuite, limit: u64) -> OwnedSuite {
    let common = |source: &CipherSuiteCommon| CipherSuiteCommon {
        suite: source.suite,
        hash_provider: source.hash_provider,
        confidentiality_limit: if aes_gcm(source.suite) {
            limit.min(source.confidentiality_limit)
        } else {
            source.confidentiality_limit
        },
    };
    match source {
        SupportedCipherSuite::Tls12(s) => OwnedSuite::Tls12(Tls12CipherSuite {
            common: common(&s.common),
            prf_provider: s.prf_provider,
            kx: s.kx,
            sign: s.sign,
            aead_alg: s.aead_alg,
        }),
        SupportedCipherSuite::Tls13(s) => OwnedSuite::Tls13(Tls13CipherSuite {
            common: common(&s.common),
            hkdf_provider: s.hkdf_provider,
            aead_alg: s.aead_alg,
            quic: s.quic,
        }),
    }
}

pub(super) fn checked(source: &[SupportedCipherSuite]) -> Vec<SupportedCipherSuite> {
    static SUITES: OnceLock<Vec<OwnedSuite>> = OnceLock::new();
    SUITES
        .get_or_init(|| {
            source
                .iter()
                .map(|s| copy_with_limit(*s, AES_GCM_RECORD_LIMIT))
                .collect()
        })
        .iter()
        .map(OwnedSuite::supported)
        .collect()
}
