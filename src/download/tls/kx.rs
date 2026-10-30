//! Fallible glue for the same locked RustCrypto primitives used by the provider.
use getrandom::rand_core::TryCryptoRng;
use p256::elliptic_curve::Generate;
use rustls::{
    NamedGroup,
    crypto::{ActiveKeyExchange, SharedSecret},
};
use zeroize::Zeroizing;

type ExchangeResult = Result<Box<dyn ActiveKeyExchange>, rustls::Error>;

pub(super) fn start(group: NamedGroup, rng: &mut impl TryCryptoRng) -> ExchangeResult {
    match group {
        NamedGroup::X25519 => {
            let mut bytes = Zeroizing::new([0u8; 32]);
            rng.try_fill_bytes(bytes.as_mut())
                .map_err(|_| rustls::Error::FailedToGetRandomBytes)?;
            let secret = x25519_dalek::StaticSecret::from(*bytes);
            let public = x25519_dalek::PublicKey::from(&secret);
            // StaticSecret provides a fallible construction path via owned bytes.
            // The exchange stays private and consumes itself on completion; keys
            // are never reused. Both this secret and the seed buffer zeroize on drop.
            Ok(Box::new(X25519Exchange { secret, public }))
        }
        NamedGroup::secp256r1 => {
            let secret = p256::ecdh::EphemeralSecret::try_generate_from_rng(rng)
                .map_err(|_| rustls::Error::FailedToGetRandomBytes)?;
            let public = p256::PublicKey::from(&secret).to_sec1_bytes();
            Ok(Box::new(P256Exchange { secret, public }))
        }
        NamedGroup::secp384r1 => {
            let secret = p384::ecdh::EphemeralSecret::try_generate_from_rng(rng)
                .map_err(|_| rustls::Error::FailedToGetRandomBytes)?;
            let public = p384::PublicKey::from(&secret).to_sec1_bytes();
            Ok(Box::new(P384Exchange { secret, public }))
        }
        _ => Err(rustls::PeerMisbehaved::InvalidKeyShare.into()),
    }
}

struct X25519Exchange {
    secret: x25519_dalek::StaticSecret,
    public: x25519_dalek::PublicKey,
}
impl ActiveKeyExchange for X25519Exchange {
    fn pub_key(&self) -> &[u8] {
        self.public.as_bytes()
    }
    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        let peer: [u8; 32] = peer
            .try_into()
            .map_err(|_| rustls::PeerMisbehaved::InvalidKeyShare)?;
        Ok(self
            .secret
            .diffie_hellman(&peer.into())
            .as_bytes()
            .as_slice()
            .into())
    }
}

macro_rules! nist_exchange {
    ($exchange:ident, $curve:ident, $group:ident) => {
        struct $exchange {
            secret: $curve::ecdh::EphemeralSecret,
            public: Box<[u8]>,
        }
        impl ActiveKeyExchange for $exchange {
            fn pub_key(&self) -> &[u8] {
                &self.public
            }
            fn group(&self) -> NamedGroup {
                NamedGroup::$group
            }
            fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
                let public = $curve::PublicKey::from_sec1_bytes(peer)
                    .map_err(|_| rustls::PeerMisbehaved::InvalidKeyShare)?;
                Ok(self
                    .secret
                    .diffie_hellman(&public)
                    .raw_secret_bytes()
                    .as_slice()
                    .into())
            }
        }
    };
}
nist_exchange!(P256Exchange, p256, secp256r1);
nist_exchange!(P384Exchange, p384, secp384r1);

#[cfg(test)]
mod tests {
    use super::*;
    use getrandom::rand_core::{TryCryptoRng, TryRng};
    struct Failed;
    impl TryRng for Failed {
        type Error = std::io::Error;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Err(std::io::Error::other("owned RNG fault"))
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            Err(std::io::Error::other("owned RNG fault"))
        }
        fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            // Exercise cleanup of a partially filled seed as well as failure propagation.
            output.fill(0x5a);
            Err(std::io::Error::other("owned RNG fault"))
        }
    }
    impl TryCryptoRng for Failed {}
    #[test]
    fn every_group_returns_typed_error_on_partial_entropy_failure() {
        for group in [
            NamedGroup::X25519,
            NamedGroup::secp256r1,
            NamedGroup::secp384r1,
        ] {
            assert!(matches!(
                start(group, &mut Failed),
                Err(rustls::Error::FailedToGetRandomBytes)
            ));
        }
    }
}
