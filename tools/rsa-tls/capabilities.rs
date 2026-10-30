//! Capability probe for the isolated prototype, not a production binary.
#[path = "provider.rs"]
mod composed;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let configure = |version| {
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(composed::provider()))
            .with_protocol_versions(&[version])
            .is_ok()
    };
    let tls13 = configure(&rustls::version::TLS13);
    let tls12 = configure(&rustls::version::TLS12);
    println!(
        "TLS_CAPABILITIES {}",
        serde_json::json!({"tls13":tls13,"tls12":tls12})
    );
    if !tls13 {
        return Err("TLS1.3 configuration unavailable".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [address, certificate] = args.as_slice() {
        use std::sync::Arc;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(std::fs::read(
            certificate,
        )?))?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(composed::provider()))
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut connection =
            rustls::ClientConnection::new(Arc::new(config), "fixture.invalid".try_into()?)?;
        let mut socket = std::net::TcpStream::connect(address)?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        while connection.is_handshaking() {
            connection.complete_io(&mut socket)?;
        }
        println!(
            "TLS_HANDSHAKE {}",
            serde_json::json!({"protocol":format!("{:?}",connection.protocol_version())})
        );
    } else if !args.is_empty() {
        return Err("expected address and test trust certificate".into());
    }
    Ok(())
}
