//! Bounded release transfer and whitelist-only extraction; no archive unpack API.
use super::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    sync::Arc,
    time::Duration,
};

const ARCHIVE_SHA: &str = "cd790191a600df48aa5c56e124953322af89df4bbd60d2c980436b0597c46194";
const URL: &str = "https://github.com/ekhodzitsky/gigastt/releases/download/v2.22.0/gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz";
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

#[derive(Debug)]
struct BodyFailure(std::io::ErrorKind);
impl std::fmt::Display for BodyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("release response read failed")
    }
}
impl std::error::Error for BodyFailure {}
struct ReleaseReader<R>(R);
impl<R: Read> Read for ReleaseReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .read(buffer)
            .map_err(|error| std::io::Error::new(error.kind(), BodyFailure(error.kind())))
    }
}
fn body_error(
    error: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    if let Some(body) = error
        .downcast_ref::<std::io::Error>()
        .and_then(|e| e.get_ref())
        .and_then(|e| e.downcast_ref::<BodyFailure>())
    {
        let reason = match body.0 {
            std::io::ErrorKind::TimedOut => "timeout",
            std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
            std::io::ErrorKind::ConnectionReset => "connection_reset",
            _ => "io",
        };
        protocol::failure_details(
            "setup_download_failed",
            Some(true),
            "release response transfer failed; rerun setup to retry",
            serde_json::json!({"phase":"response_body","reason":reason,"preserved":true}),
        )
    } else {
        error
    }
}

fn validate_url(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url.parse()?;
    if uri.scheme_str() != Some("https")
        || !matches!(
            uri.host(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
        || uri.port_u16().is_some_and(|p| p != 443)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || url.contains('#')
    {
        return Err(failure(
            "setup_download_failed",
            "refusing an unexpected release redirect",
        ));
    }
    Ok(())
}

fn transport_error(error: ureq::Error) -> Box<dyn std::error::Error + Send + Sync> {
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

fn download(path: &Path, cancelled: &AtomicBool) -> Result<()> {
    let tls = ureq::tls::TlsConfig::builder()
        .unversioned_rustls_crypto_provider(Arc::new(crate::download::tls::provider()?))
        .build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .new_agent();
    let mut url = URL.to_owned();
    for _ in 0..5 {
        validate_url(&url)?;
        if cancelled.load(Ordering::SeqCst) {
            return Err(protocol::failure(
                "interrupted",
                Some(true),
                "setup interrupted",
            ));
        }
        let mut response = agent.get(&url).call().map_err(transport_error)?;
        let status = response.status().as_u16();
        if [301, 302, 303, 307, 308].contains(&status) {
            url = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    failure(
                        "setup_download_failed",
                        "release redirect has no valid location",
                    )
                })?
                .to_owned();
            continue;
        }
        if status != 200 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.parse::<u64>().ok().or_else(|| {
                        httpdate::parse_http_date(v).ok().map(|at| {
                            at.duration_since(std::time::SystemTime::now())
                                .map_or(0, |d| d.as_secs().saturating_add(1))
                        })
                    })
                });
            return Err(protocol::failure_details(
                "setup_download_failed",
                Some(status == 429 || status >= 500),
                "release server refused transfer; respect retry guidance before another setup attempt",
                serde_json::json!({"http_status":status,"retry_after_seconds":retry_after,"preserved":true}),
            ));
        }
        storage::atomic_write_with(path, false, |file| {
            let mut body = response.body_mut().as_reader().take(ARCHIVE_LIMIT + 1);
            let sha = copy(&mut ReleaseReader(&mut body), file, cancelled).map_err(body_error)?;
            if body.limit() == 0 || sha != ARCHIVE_SHA {
                return Err(failure(
                    "setup_archive_invalid",
                    "release archive size or SHA-256 mismatch",
                ));
            }
            Ok(())
        })?;
        return Ok(());
    }
    Err(failure(
        "setup_download_failed",
        "release redirect limit exceeded",
    ))
}

pub(super) fn install(root: &Path, supplied: Option<&Path>, cancelled: &AtomicBool) -> Result<()> {
    let cached = root.join("gigastt-2.22.0.tar.gz");
    let archive = supplied.unwrap_or(&cached);
    if supplied.is_none() && !archive.try_exists()? {
        download(archive, cancelled)?;
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
