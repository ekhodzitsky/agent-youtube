//! Bounded release transfer and whitelist-only extraction; no archive unpack API.
use super::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};

const ARCHIVE_SHA: &str = super::assets::ARCHIVE.sha;
const ARCHIVE_LIMIT: u64 = 32 << 20;
const EXPANDED_LIMIT: u64 = 128 << 20;
const FILES: [&str; 5] = ["gigastt", "LICENSE", "NOTICE", "README.md", "CHANGELOG.md"];

fn copy(reader: &mut impl Read, writer: &mut impl Write, cancelled: &AtomicBool) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(protocol::failure(
                "interrupted",
                Some(true),
                "setup interrupted; published installation files retained",
            ));
        }
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
fn validate_url(url: &str) -> Result<()> {
    super::transfer::validate_url(url)
}

pub(super) fn transport_error(error: ureq::Error) -> Box<dyn std::error::Error + Send + Sync> {
    let tls = match &error {
        ureq::Error::Rustls(error) => Some(error),
        ureq::Error::Io(error) => error
            .get_ref()
            .and_then(|source| source.downcast_ref::<rustls::Error>()),
        _ => None,
    };
    if matches!(tls, Some(rustls::Error::FailedToGetRandomBytes)) {
        return protocol::failure(
            "setup_tls_random_unavailable",
            Some(true),
            "system randomness unavailable for release TLS; retry when available",
        );
    }
    if matches!(
        tls,
        Some(rustls::Error::InvalidCertificate(_) | rustls::Error::PeerIncompatible(_))
    ) {
        protocol::failure(
            "setup_tls_invalid",
            Some(false),
            "release TLS validation or compatibility failed; validation remains enabled",
        )
    } else {
        protocol::failure(
            "setup_download_failed",
            Some(true),
            "release HTTPS transfer failed; no URL diagnostics are emitted",
        )
    }
}

pub(super) fn install(
    root: &Path,
    supplied: Option<&Path>,
    offline: bool,
    cancelled: &AtomicBool,
) -> Result<()> {
    let cached = root.join("gigastt-2.22.0.tar.gz");
    let archive = supplied.unwrap_or(&cached);
    if supplied.is_none() {
        super::transfer::fetch(root, archive, super::assets::ARCHIVE, offline, cancelled)?;
    }
    storage::reject_nonregular(archive)?;
    let file = storage::open_regular_input(archive)?;
    if file.metadata()?.len() > ARCHIVE_LIMIT || storage::digest(archive)? != ARCHIVE_SHA {
        return Err(failure(
            "setup_archive_invalid",
            "release archive size or SHA-256 mismatch; existing data preserved",
        ));
    }
    extract(file, root, BINARY_SHA, cancelled)
}

fn extract(input: impl Read, root: &Path, binary_sha: &str, cancelled: &AtomicBool) -> Result<()> {
    let decoded = GzDecoder::new(input).take(EXPANDED_LIMIT + 1);
    let mut archive = tar::Archive::new(decoded);
    let mut seen = BTreeSet::new();
    for (index, entry) in archive.entries()?.enumerate() {
        if index > 6 {
            return Err(failure("setup_archive_invalid", "too many release entries"));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path == Path::new(".") && entry.header().entry_type().is_dir() {
            continue;
        }
        let name = path
            .to_str()
            .ok_or("non-UTF8 release path")?
            .strip_prefix("./")
            .unwrap_or(path.to_str().ok_or("non-UTF8 release path")?);
        if !FILES.contains(&name)
            || !entry.header().entry_type().is_file()
            || !seen.insert(name.to_owned())
            || entry.size()
                > if name == "gigastt" {
                    100 << 20
                } else {
                    4 << 20
                }
        {
            return Err(failure(
                "setup_archive_invalid",
                "release contains unexpected, duplicate, linked or oversized entries",
            ));
        }
        let destination = root.join(if name == "gigastt" { BINARY } else { name });
        storage::reject_nonregular(&destination)?;
        if destination.try_exists()? {
            let sha = copy(&mut entry, &mut std::io::sink(), cancelled)?;
            if sha != storage::digest(&destination)? {
                return Err(failure(
                    "setup_artifact_changed",
                    "refusing to overwrite an existing installation artifact",
                ));
            }
        } else {
            storage::atomic_write_with(&destination, false, |file| {
                copy(&mut entry, file, cancelled)?;
                Ok(())
            })?;
        }
    }
    if seen.len() != FILES.len() {
        return Err(failure(
            "setup_archive_invalid",
            "release is missing required files or notices",
        ));
    }
    // Consume the gzip trailer too; TAR's end marker alone is not CRC validation.
    let mut decoded = archive.into_inner();
    copy(&mut decoded, &mut std::io::sink(), cancelled)?;
    if decoded.limit() == 0 || storage::digest(&root.join(BINARY))? != binary_sha {
        return Err(failure(
            "setup_archive_invalid",
            "decompressed size or executable SHA-256 mismatch",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(BINARY))?;
        executable.set_permissions(fs::Permissions::from_mode(0o700))?;
        executable.sync_all()?;
    }
    storage::sync_parent(&root.join(BINARY))?;
    Ok(())
}

#[cfg(test)]
mod tests;
