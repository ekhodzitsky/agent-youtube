//! Shared client-only boundary for isolated provider qualification.
use rustls::{
    crypto::{CryptoProvider, KeyProvider},
    pki_types::PrivateKeyDer,
};
use std::sync::Arc;
#[derive(Debug)]
struct NoPrivateKeys;
impl KeyProvider for NoPrivateKeys {
    fn load_private_key(
        &self,
        _: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
        Err(rustls::Error::General(
            "private keys are disabled in the agent-youtube client provider".into(),
        ))
    }
}
pub(super) fn restrict(provider: &mut CryptoProvider) {
    provider.key_provider = &NoPrivateKeys;
}
