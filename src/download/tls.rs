//! Client-only TLS policy over pinned pure-Rust cryptographic primitives.
use crate::Result;
mod kx;
mod limits;
mod signatures;
use rustls::{
    NamedGroup,
    crypto::{ActiveKeyExchange, CryptoProvider, KeyProvider, SharedSecret, SupportedKxGroup},
};
use std::sync::Arc;

pub(crate) fn provider() -> Result<CryptoProvider> {
    let mut provider = rustls_rustcrypto::provider();
    provider.cipher_suites = limits::checked(&provider.cipher_suites);
    // Certificate private-key loading is forbidden; ephemeral ECDH is separate.
    provider.key_provider = &NoPrivateKeys;
    provider.signature_verification_algorithms =
        signatures::checked(provider.signature_verification_algorithms)?;
    // NIST secret arithmetic is allowed only with the patched crypto-bigint family
    // fenced in Cargo.toml; strict wire-point checks remain in the application.
    static GROUPS: [CheckedGroup; 3] = [
        CheckedGroup(NamedGroup::X25519),
        CheckedGroup(NamedGroup::secp256r1),
        CheckedGroup(NamedGroup::secp384r1),
    ];
    provider.kx_groups = GROUPS.iter().map(|g| g as &dyn SupportedKxGroup).collect();
    Ok(provider)
}
#[derive(Debug)]
struct NoPrivateKeys;
impl KeyProvider for NoPrivateKeys {
    fn load_private_key(
        &self,
        _: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> std::result::Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
        Err(rustls::Error::General(
            "private keys are disabled in the agent-youtube client provider".into(),
        ))
    }
}
#[derive(Debug)]
struct CheckedGroup(NamedGroup);
impl SupportedKxGroup for CheckedGroup {
    fn name(&self) -> NamedGroup {
        self.0
    }
    fn start(&self) -> std::result::Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        Ok(Box::new(CheckedExchange(kx::start(
            self.0,
            &mut getrandom::SysRng,
        )?)))
    }
    fn ffdhe_group(&self) -> Option<rustls::ffdhe_groups::FfdheGroup<'static>> {
        None
    }
}
struct CheckedExchange(Box<dyn ActiveKeyExchange>);
impl ActiveKeyExchange for CheckedExchange {
    fn pub_key(&self) -> &[u8] {
        self.0.pub_key()
    }
    fn group(&self) -> NamedGroup {
        self.0.group()
    }
    fn complete(self: Box<Self>, peer: &[u8]) -> std::result::Result<SharedSecret, rustls::Error> {
        let group = self.0.group();
        let valid = match group {
            NamedGroup::X25519 => peer.len() == 32,
            NamedGroup::secp256r1 => peer.len() == 65 && peer.first() == Some(&4),
            NamedGroup::secp384r1 => peer.len() == 97 && peer.first() == Some(&4),
            _ => false,
        };
        if !valid {
            return Err(rustls::PeerMisbehaved::InvalidKeyShare.into());
        }
        let secret = self.0.complete(peer)?;
        // Fixed-length OR reduction: do not short-circuit on secret bytes.
        if group == NamedGroup::X25519
            && secret
                .secret_bytes()
                .iter()
                .fold(0u8, |bits, byte| bits | byte)
                == 0
        {
            return Err(rustls::Error::PeerMisbehaved(
                rustls::PeerMisbehaved::InvalidKeyShare,
            ));
        }
        Ok(secret)
    }
}

#[cfg(test)]
mod tests;
