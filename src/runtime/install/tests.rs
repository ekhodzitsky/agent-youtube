use super::*;
fn archive(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
    let zipped = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(zipped);
    for (name, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o700);
        header.set_entry_type(*kind);
        if kind.is_symlink() {
            header.set_link_name("/outside").unwrap();
        }
        header.set_cksum();
        archive
            .append_data(&mut header, name, &b"fixture"[..])
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}
#[test]
fn extraction_rejects_links_duplicate_files_missing_notices_and_truncated_gzip() {
    let regular = tar::EntryType::Regular;
    for entries in [
        vec![("gigastt", tar::EntryType::Symlink)],
        vec![("gigastt", regular), ("gigastt", regular)],
        vec![("gigastt", regular)],
        vec![("outside/file", regular)],
    ] {
        let root = super::super::tests::temp();
        assert!(
            extract(
                &archive(&entries)[..],
                &root,
                "wrong",
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(!root.join("runtime.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
    let entries: Vec<_> = FILES.iter().map(|name| (*name, regular)).collect();
    let mut bytes = archive(&entries);
    bytes.truncate(bytes.len() - 6);
    let root = super::super::tests::temp();
    assert!(
        extract(
            &bytes[..],
            &root,
            &format!("{:x}", Sha256::digest(b"fixture")),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn complete_extraction_is_idempotent_but_refuses_changed_files() {
    let root = super::super::tests::temp();
    let entries: Vec<_> = FILES
        .iter()
        .map(|name| (*name, tar::EntryType::Regular))
        .collect();
    let data = archive(&entries);
    let hash = format!("{:x}", Sha256::digest(b"fixture"));
    let cancelled = AtomicBool::new(false);
    extract(&data[..], &root, &hash, &cancelled).unwrap();
    extract(&data[..], &root, &hash, &cancelled).unwrap();
    fs::write(root.join("NOTICE"), b"unrelated").unwrap();
    assert!(extract(&data[..], &root, &hash, &cancelled).is_err());
    assert_eq!(fs::read(root.join("NOTICE")).unwrap(), b"unrelated");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn redirect_policy_rejects_downgrades_credentials_and_non_release_hosts() {
    for url in [
        "http://github.com/file",
        "https://evil.invalid/file?secret=private",
        "https://user:password@github.com/file",
        "https://github.com:8443/file",
    ] {
        assert!(validate_url(url).is_err());
    }
    assert!(
        validate_url("https://release-assets.githubusercontent.com/file?opaque=private").is_ok()
    );
}

#[test]
fn path_traversal_entry_cannot_write_outside_install_directory() {
    let base = super::super::tests::temp();
    let root = base.join("install");
    fs::create_dir(&root).unwrap();
    let zipped = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(zipped);
    let mut header = tar::Header::new_gnu();
    header.set_size(7);
    header.set_mode(0o600);
    header.as_mut_bytes()[..10].copy_from_slice(b"../escaped");
    header.set_cksum();
    builder.append(&header, &b"fixture"[..]).unwrap();
    let data = builder.into_inner().unwrap().finish().unwrap();
    assert!(extract(&data[..], &root, "wrong", &AtomicBool::new(false)).is_err());
    assert!(!base.join("escaped").exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn transfer_errors_do_not_expose_signed_urls_or_private_tls_context() {
    let tls = rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForNameContext {
        expected: rustls::pki_types::ServerName::try_from("private.invalid").unwrap(),
        presented: vec!["private.invalid".into()],
    });
    let error = transport_error(ureq::Error::Rustls(tls));
    let failure = error.downcast_ref::<protocol::Failure>().unwrap();
    assert_eq!(failure.code, "setup_tls_invalid");
    assert_eq!(failure.retryable, Some(false));
    assert!(!error.to_string().contains("private.invalid"));
    let error = transport_error(ureq::Error::Io(std::io::Error::other(
        "https://release-assets.githubusercontent.com/file?secret=private",
    )));
    assert!(!error.to_string().contains("secret"));
}

#[test]
fn extraction_write_errors_remain_local_io_errors() {
    struct FailedWriter;
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "local disk refused write",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let error = copy(
        &mut &b"fixture"[..],
        &mut FailedWriter,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[test]
fn setup_tls_entropy_failure_is_sanitized_retryable_json() {
    for wrapped in [false, true] {
        let tls = rustls::Error::FailedToGetRandomBytes;
        let error = if wrapped {
            ureq::Error::Io(std::io::Error::other(tls))
        } else {
            ureq::Error::Rustls(tls)
        };
        let error = transport_error(error);
        let failure = error.downcast_ref::<protocol::Failure>().unwrap();
        assert_eq!(failure.code, "setup_tls_random_unavailable");
        assert_eq!(failure.retryable, Some(true));
    }
}
