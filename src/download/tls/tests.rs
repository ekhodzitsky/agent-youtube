use super::*;
use rustls::{NamedGroup, client::danger::ServerCertVerifier};
#[test]
fn aes_gcm_suites_enforce_finite_record_limits() {
    use rustls::{CipherSuite, SupportedCipherSuite};
    for suite in provider().unwrap().cipher_suites {
        let limit = match suite {
            SupportedCipherSuite::Tls12(s) => s.common.confidentiality_limit,
            SupportedCipherSuite::Tls13(s) => s.common.confidentiality_limit,
        };
        let chacha = matches!(
            suite.suite(),
            CipherSuite::TLS13_CHACHA20_POLY1305_SHA256
                | CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
                | CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
        );
        assert_eq!(
            limit,
            if chacha { u64::MAX } else { 1 << 24 },
            "{:?}",
            suite.suite()
        );
    }
}

#[test]
fn client_provider_never_loads_private_keys() {
    let provider = provider().unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(vec![0u8; 32].into());
    let error = provider.key_provider.load_private_key(key).unwrap_err();
    assert!(error.to_string().contains("private keys are disabled"));
}

#[test]
#[ignore = "requires explicitly authorized live HTTPS access"]
fn live_youtube_tls_client_qualification() {
    let http = crate::download::http::Http::new("https://www.youtube.com").unwrap();
    let mut response = http
        .request(
            "https://www.youtube.com/robots.txt",
            None,
            &[],
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let bytes = crate::download::http::read(
        &mut response,
        1024 * 1024,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn offered_groups_are_limited_to_qualified_implementations() {
    let provider = provider().unwrap();
    assert_eq!(
        provider
            .kx_groups
            .iter()
            .map(|g| g.name())
            .collect::<Vec<_>>(),
        vec![
            NamedGroup::X25519,
            NamedGroup::secp256r1,
            NamedGroup::secp384r1
        ]
    );
}
#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn key_exchange_entropy_failure_returns_error_in_child() {
    const FLAG: &str = "AGENT_YOUTUBE_TEST_ENTROPY_GROUP";
    if let Ok(index) = std::env::var(FLAG) {
        let provider = provider().unwrap();
        let filter = [
            libc::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 0,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 0,
                jf: 1,
                k: libc::SYS_getrandom as u32,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x00050000 | libc::EIO as u32,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x7fff0000,
            },
        ];
        let program = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_ptr() as *mut _,
        };
        // Test-only filter applies solely to this disposable child thread.
        // Both program and filter remain valid for the synchronous prctl call.
        unsafe {
            assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
            assert_eq!(libc::prctl(libc::PR_SET_SECCOMP, 2, &program), 0);
        }
        let result = provider.kx_groups[index.parse::<usize>().unwrap()].start();
        assert!(matches!(result, Err(rustls::Error::FailedToGetRandomBytes)));
        return;
    }
    for index in 0..3 {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "download::tls::tests::key_exchange_entropy_failure_returns_error_in_child",
                "--nocapture",
            ])
            .env(FLAG, index.to_string())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "group {index}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn low_order_and_malformed_shares_are_rejected() {
    let provider = provider().unwrap();
    let group = provider
        .kx_groups
        .iter()
        .find(|g| g.name() == NamedGroup::X25519)
        .unwrap();
    let mut one = vec![0; 32];
    one[0] = 1;
    for peer in [vec![], vec![0; 31], vec![0; 33], vec![0; 32], one] {
        assert!(group.start().unwrap().complete(&peer).is_err());
    }
}
#[test]
fn valid_x25519_agrees_without_disabling_signature_algorithms() {
    let provider = provider().unwrap();
    let left = provider.kx_groups[0].start().unwrap();
    let right = provider.kx_groups[0].start().unwrap();
    let lp = left.pub_key().to_vec();
    let rp = right.pub_key().to_vec();
    let ls = left.complete(&rp).unwrap();
    let rs = right.complete(&lp).unwrap();
    assert!(ls.secret_bytes() == rs.secret_bytes());
    assert!(!provider.signature_verification_algorithms.all.is_empty());
}
#[test]
fn public_signature_verifiers_accept_oracle_and_reject_tampering() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls");
    let params: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("parameters.json")).unwrap()).unwrap();
    let message = params["message"].as_str().unwrap().as_bytes();
    let provider = provider().unwrap();
    let algorithms = provider.signature_verification_algorithms.all;
    let keys = [
        "p256", "p256", "p384", "p384", "ed25519", "rsa", "rsa", "rsa", "rsa", "rsa", "rsa",
    ];
    assert_eq!(algorithms.len(), keys.len());
    for (index, (algorithm, key)) in algorithms.iter().zip(keys).enumerate() {
        let public = std::fs::read(root.join(format!("{key}.pub"))).unwrap();
        let mut signature = std::fs::read(root.join(format!("{index}.sig"))).unwrap();
        assert!(
            algorithm
                .verify_signature(&public, message, &signature)
                .is_ok(),
            "algorithm {index}"
        );
        assert!(
            algorithm
                .verify_signature(&public, b"changed message", &signature)
                .is_err()
        );
        assert!(
            algorithm
                .verify_signature(&[], message, &signature)
                .is_err()
        );
        assert!(algorithm.verify_signature(&public, message, &[]).is_err());
        signature[0] ^= 0x80;
        assert!(
            algorithm
                .verify_signature(&public, message, &signature)
                .is_err()
        );
    }
}

#[test]
fn rsa_policy_checks_actual_modulus_bits_for_every_algorithm() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls/rsa-policy");
    let provider = provider().unwrap();
    let names = [
        "pkcs1-sha256",
        "pkcs1-sha384",
        "pkcs1-sha512",
        "pss-sha256",
        "pss-sha384",
        "pss-sha512",
    ];
    for bits in [1024, 2047, 2048, 3072] {
        let key = std::fs::read(root.join(format!("{bits}.pub"))).unwrap();
        for (algorithm, name) in provider.signature_verification_algorithms.all[5..]
            .iter()
            .zip(names)
        {
            // Digest-sized SHA512 PSS salt cannot fit in a 1024-bit RSA key.
            if bits == 1024 && name == "pss-sha512" {
                continue;
            }
            let signature = std::fs::read(root.join(format!("{bits}-{name}.sig"))).unwrap();
            assert_eq!(
                algorithm
                    .verify_signature(&key, b"agent-youtube RSA policy fixture\n", &signature)
                    .is_ok(),
                bits >= 2048,
                "{bits} {name}"
            );
            for (_, mapped) in provider.signature_verification_algorithms.mapping {
                for verifier in *mapped {
                    if verifier.public_key_alg_id() == algorithm.public_key_alg_id()
                        && verifier.signature_alg_id() == algorithm.signature_alg_id()
                    {
                        assert_eq!(
                            verifier
                                .verify_signature(
                                    &key,
                                    b"agent-youtube RSA policy fixture\n",
                                    &signature
                                )
                                .is_ok(),
                            bits >= 2048,
                            "handshake {bits} {name}"
                        );
                    }
                }
            }
            assert!(
                algorithm
                    .verify_signature(&vec![0; 1101], b"", &signature)
                    .is_err()
            );
        }
    }
}

#[test]
fn rsa_policy_applies_to_complete_certificate_chains() {
    use rustls::{
        RootCertStore,
        client::WebPkiServerVerifier,
        pki_types::{CertificateDer, ServerName, UnixTime},
    };
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls/rsa-policy");
    let params: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("parameters.json")).unwrap()).unwrap();
    let now = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
        params["valid_at"].as_u64().unwrap(),
    ));
    for bits in [1024, 2047, 2048, 3072] {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(
                std::fs::read(root.join(format!("ca-{bits}.der"))).unwrap(),
            ))
            .unwrap();
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(provider().unwrap()),
        )
        .build()
        .unwrap();
        let leaf =
            CertificateDer::from(std::fs::read(root.join(format!("leaf-{bits}.der"))).unwrap());
        assert_eq!(
            verifier
                .verify_server_cert(
                    &leaf,
                    &[],
                    &ServerName::try_from("fixture.invalid").unwrap(),
                    &[],
                    now
                )
                .is_ok(),
            bits >= 2048,
            "issuer {bits}"
        );
    }
}

#[test]
fn certificate_chain_verification_preserves_hostname_time_and_trust_checks() {
    use rustls::{
        RootCertStore,
        client::{WebPkiServerVerifier, danger::ServerCertVerifier},
        pki_types::{CertificateDer, ServerName, UnixTime},
    };
    use std::{sync::Arc, time::Duration};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls");
    let params: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("parameters.json")).unwrap()).unwrap();
    let when = params["valid_at"].as_u64().unwrap();
    let ca = CertificateDer::from(std::fs::read(root.join("ca.der")).unwrap());
    let leaf = CertificateDer::from(std::fs::read(root.join("leaf.der")).unwrap());
    let mut roots = RootCertStore::empty();
    roots.add(ca).unwrap();
    let verifier =
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::new(provider().unwrap()))
            .build()
            .unwrap();
    let name = ServerName::try_from("fixture.invalid").unwrap();
    let now = UnixTime::since_unix_epoch(Duration::from_secs(when));
    assert!(
        verifier
            .verify_server_cert(&leaf, &[], &name, &[], now)
            .is_ok()
    );
    let wrong = ServerName::try_from("other.invalid").unwrap();
    assert!(
        verifier
            .verify_server_cert(&leaf, &[], &wrong, &[], now)
            .is_err()
    );
    let expired = UnixTime::since_unix_epoch(Duration::from_secs(when + 500 * 86400));
    assert!(
        verifier
            .verify_server_cert(&leaf, &[], &name, &[], expired)
            .is_err()
    );
    // An unrelated leaf must not become a trusted root for the original issuer.
    let mut unrelated = RootCertStore::empty();
    unrelated.add(leaf.clone()).unwrap();
    let untrusted = WebPkiServerVerifier::builder_with_provider(
        Arc::new(unrelated),
        Arc::new(provider().unwrap()),
    )
    .build()
    .unwrap();
    assert!(
        untrusted
            .verify_server_cert(&leaf, &[], &name, &[], now)
            .is_err()
    );
    let mut corrupted = leaf.to_vec();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 1;
    assert!(
        verifier
            .verify_server_cert(&CertificateDer::from(corrupted), &[], &name, &[], now)
            .is_err()
    );
}
#[test]
fn nist_exchange_agrees_and_rejects_invalid_or_compressed_points() {
    let provider = provider().unwrap();
    for (index, size) in [(1, 65), (2, 97)] {
        let group = provider.kx_groups[index];
        let left = group.start().unwrap();
        let right = group.start().unwrap();
        assert_eq!(left.group(), group.name());
        let lp = left.pub_key().to_vec();
        let rp = right.pub_key().to_vec();
        assert_eq!(lp.len(), size);
        assert_eq!(lp[0], 4);
        let ls = left.complete(&rp).unwrap();
        let rs = right.complete(&lp).unwrap();
        assert_eq!(ls.secret_bytes(), rs.secret_bytes());
        let mut compressed = lp[..size.div_ceil(2)].to_vec();
        compressed[0] = 2;
        let mut off_curve = vec![0; size];
        off_curve[0] = 4;
        for peer in [Vec::new(), vec![4; size - 1], compressed, off_curve] {
            assert!(group.start().unwrap().complete(&peer).is_err());
        }
    }
}

#[test]
#[ignore = "requires an owned loopback TLS oracle and explicit test trust certificate"]
fn research_tls_protocol_handshake() -> crate::Result<()> {
    let address = std::env::var("AGENT_YOUTUBE_TLS_PROBE_ADDRESS")?;
    let trust = std::env::var("AGENT_YOUTUBE_TLS_PROBE_CA")?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(std::fs::read(
        trust,
    )?))?;
    let record_probe = std::env::var_os("AGENT_YOUTUBE_TLS_PROBE_RECORD_LIMIT").is_some();
    let mut provider = provider()?;
    if record_probe {
        provider.cipher_suites = provider
            .cipher_suites
            .iter()
            .copied()
            .filter(|suite| {
                matches!(
                    suite.suite(),
                    rustls::CipherSuite::TLS13_AES_128_GCM_SHA256
                        | rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
                        | rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
                )
            })
            .map(|suite| {
                // Test-only process-lifetime ownership for rustls's static suite API.
                let owned = Box::leak(Box::new(limits::copy_with_limit(suite, 4)));
                owned.supported()
            })
            .collect();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
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
    if record_probe {
        use std::io::{Read, Write};
        let mut written = 0;
        let mut application_records = 0;
        let mut close_alert = false;
        let tls13 = connection.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3);
        if tls13 {
            for _ in 0..12 {
                let mut stream = rustls::Stream::new(&mut connection, &mut socket);
                stream.write_all(b"probe\n")?;
                stream.flush()?;
                let mut response = [0u8; 6];
                stream.read_exact(&mut response)?;
                assert_eq!(&response, b"eborp\n");
                written += 6;
            }
            connection.send_close_notify();
            while connection.wants_write() {
                connection.write_tls(&mut socket)?;
            }
        } else {
            for byte in b"GET / HTTP/1.0\r\n\r\n" {
                let count = connection.writer().write(std::slice::from_ref(byte))?;
                while connection.wants_write() {
                    let mut wire = Vec::new();
                    connection.write_tls(&mut wire)?;
                    let mut offset = 0;
                    while offset < wire.len() {
                        assert!(wire.len() - offset >= 5);
                        let size =
                            u16::from_be_bytes([wire[offset + 3], wire[offset + 4]]) as usize;
                        assert!(wire.len() - offset >= 5 + size);
                        match wire[offset] {
                            23 => {
                                assert!(!close_alert);
                                application_records += 1;
                            }
                            21 => close_alert = true,
                            _ => {}
                        }
                        offset += size + 5;
                    }
                    socket.write_all(&wire)?;
                }
                if count == 0 {
                    break;
                }
                written += count;
            }
            assert!(
                application_records > 0 && application_records <= 4 && close_alert,
                "TLS1.2 must send a close alert before exceeding the record budget"
            );
        }
        println!(
            "TLS_RECORD_LIMIT {}",
            serde_json::json!({"accepted_plaintext_bytes":written,"tls12_application_records":application_records,"close_alert":close_alert,"tls13":tls13,"test_record_limit":4})
        );
    }
    println!(
        "TLS_HANDSHAKE {}",
        serde_json::json!({"protocol":format!("{:?}",connection.protocol_version()), "group":format!("{:?}",connection.negotiated_key_exchange_group().map(|g|g.name())), "cipher_suite":format!("{:?}",connection.negotiated_cipher_suite().map(|s|s.suite()))})
    );
    Ok(())
}
