//! Owned streaming setup transfers with durable prefixes and pinned final hashes.
use super::{assets::Asset, failure};
use crate::{Result, protocol, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const CHUNK: u64 = 4 * 1024 * 1024;
const OWNER: &str = "agent-youtube setup downloads v1\n";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    sha: String,
    bytes: u64,
    committed: u64,
    prefix_sha: String,
}
fn interrupted(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(protocol::failure(
            "interrupted",
            Some(true),
            "setup interrupted; download state retained",
        ));
    }
    Ok(())
}
fn invalid() -> Box<dyn std::error::Error + Send + Sync> {
    failure(
        "setup_download_invalid",
        "download identity, range or content verification failed; owned data retained",
    )
}
fn network() -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure(
        "setup_download_failed",
        Some(true),
        "setup transfer failed; rerun setup to continue",
    )
}
pub(super) fn validate_url(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| invalid())?;
    if uri.scheme_str() != Some("https")
        || uri.port_u16().is_some_and(|p| p != 443)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || url.contains('#')
        || !matches!(
            uri.host(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "huggingface.co"
                    | "cdn-lfs.huggingface.co"
                    | "cdn-lfs.hf.co"
                    | "cdn-lfs-us-1.hf.co"
                    | "us.aws.cdn.hf.co"
                    | "cas-bridge.xethub.hf.co"
            )
        )
    {
        return Err(failure(
            "setup_download_failed",
            "refusing an unexpected setup asset origin",
        ));
    }
    Ok(())
}
fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
fn cooldown(cache: &Path) -> Result<()> {
    let path = cache.join("retry-at.json");
    storage::reject_nonregular(&path)?;
    if path.try_exists()? {
        let until: Option<u64> = serde_json::from_str(&storage::read_text(&path, 128)?)?;
        let until = until.ok_or_else(|| {
            failure(
                "setup_retry_after_invalid",
                "server retry guidance is not understood; setup network requests remain blocked",
            )
        })?;
        let seconds = until.saturating_sub(now()?);
        if seconds > 0 {
            return Err(protocol::failure_details(
                "setup_download_failed",
                Some(true),
                "setup server cooldown active",
                serde_json::json!({"retry_after_seconds":seconds,"preserved":true,"next_action":"retry_setup_after_delay"}),
            ));
        }
    }
    Ok(())
}
fn cache(root: &Path) -> Result<std::path::PathBuf> {
    let dir = root.join(".setup-downloads");
    let marker = dir.join("owner");
    if dir.try_exists()? {
        let m = fs::symlink_metadata(&dir)?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(invalid());
        }
        if !marker.try_exists()? && fs::read_dir(&dir)?.next().transpose()?.is_some() {
            return Err(invalid());
        }
    }
    storage::private_directory(&dir, true)?;
    storage::reject_nonregular(&marker)?;
    if !marker.try_exists()? {
        storage::atomic_write(&marker, OWNER.as_bytes(), false)?;
    }
    if storage::read_text(&marker, 128)? != OWNER {
        return Err(invalid());
    }
    Ok(dir)
}
fn save(file: &fs::File, path: &Path, state: &mut State, hash: &Sha256) -> Result<()> {
    file.sync_all()?;
    state.prefix_sha = format!("{:x}", hash.clone().finalize());
    storage::atomic_write(path, &serde_json::to_vec(state)?, true)
}
fn valid_final(path: &Path, asset: Asset) -> Result<bool> {
    storage::reject_nonregular(path)?;
    if !path.try_exists()? {
        return Ok(false);
    }
    if fs::metadata(path)?.len() != asset.bytes || storage::digest(path)? != asset.sha {
        return Err(failure(
            "setup_artifact_changed",
            "existing asset differs from pinned content; refusing to overwrite it",
        ));
    }
    Ok(true)
}
pub(super) fn fetch(
    root: &Path,
    destination: &Path,
    asset: Asset,
    offline: bool,
    cancelled: &AtomicBool,
) -> Result<()> {
    let tls = ureq::tls::TlsConfig::builder()
        .unversioned_rustls_crypto_provider(Arc::new(crate::download::tls::provider()?))
        .build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_resolve(Some(Duration::from_secs(15)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .timeout_per_call(Some(Duration::from_secs(180)))
        .build()
        .new_agent();
    fetch_with(
        root,
        destination,
        asset,
        offline,
        cancelled,
        &agent,
        validate_url,
    )
}
fn fetch_with(
    root: &Path,
    destination: &Path,
    asset: Asset,
    offline: bool,
    cancelled: &AtomicBool,
    agent: &ureq::Agent,
    validate: fn(&str) -> Result<()>,
) -> Result<()> {
    interrupted(cancelled)?;
    if valid_final(destination, asset)? {
        return Ok(());
    }
    if offline
        && !root
            .join(".setup-downloads")
            .join(format!("{}.json", asset.sha))
            .try_exists()?
    {
        return Err(protocol::failure_details(
            "setup_offline_missing",
            Some(false),
            "required setup asset is missing in offline mode",
            serde_json::json!({"asset":asset.name,"bytes":asset.bytes,"next_action":"supply_local_assets_or_run_setup_online","preserved":true}),
        ));
    }
    let cache = cache(root)?;
    let partial = cache.join(format!("{}.part", asset.sha));
    let receipt = cache.join(format!("{}.json", asset.sha));
    storage::reject_nonregular(&partial)?;
    storage::reject_nonregular(&receipt)?;
    let mut state: State = if receipt.try_exists()? {
        serde_json::from_str(&storage::read_text(&receipt, 4096)?)?
    } else {
        if partial.try_exists()? {
            return Err(invalid());
        }
        let s = State {
            sha: asset.sha.into(),
            bytes: asset.bytes,
            committed: 0,
            prefix_sha: format!("{:x}", Sha256::new().finalize()),
        };
        storage::atomic_write(&receipt, &serde_json::to_vec(&s)?, false)?;
        s
    };
    if state.sha != asset.sha || state.bytes != asset.bytes || state.committed > asset.bytes {
        return Err(invalid());
    }
    if !partial.try_exists()? {
        if state.committed != 0 {
            return Err(invalid());
        }
        storage::private_file().open(&partial)?.sync_all()?;
        storage::sync_parent(&partial)?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(&partial)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata()?.nlink() != 1 {
            return Err(invalid());
        }
    }
    if file.metadata()?.len() < state.committed {
        return Err(invalid());
    }
    let mut hash = Sha256::new();
    let mut left = state.committed;
    let mut buf = [0u8; 64 * 1024];
    while left > 0 {
        interrupted(cancelled)?;
        let n = usize::try_from(left.min(buf.len() as u64))?;
        file.read_exact(&mut buf[..n])?;
        hash.update(&buf[..n]);
        left -= n as u64;
    }
    if format!("{:x}", hash.clone().finalize()) != state.prefix_sha {
        return Err(invalid());
    }
    // Only uncommitted bytes of this identity-bound owned transfer may be rolled back.
    file.set_len(state.committed)?;
    file.sync_all()?;
    storage::disk_space_at(&cache, "setup")?.require_at(
        &cache,
        asset.bytes - state.committed,
        "setup",
    )?;
    let mut failures = 0;
    while state.committed < asset.bytes {
        if offline {
            return Err(protocol::failure_details(
                "setup_offline_missing",
                Some(false),
                "setup asset download is incomplete in offline mode",
                serde_json::json!({"asset":asset.name,"downloaded_bytes":state.committed,"total_bytes":asset.bytes,"preserved":true,"next_action":"run_setup_online_to_continue"}),
            ));
        }
        interrupted(cancelled)?;
        cooldown(&cache)?;
        let before = state.committed;
        protocol::progress(
            "setup_download",
            "transferring pinned setup asset",
            serde_json::json!({"asset":asset.name,"downloaded_bytes":before,"total_bytes":asset.bytes,"durable":true}),
        );
        let result = attempt(
            agent, validate, &cache, &mut file, &receipt, &mut state, &mut hash, asset, cancelled,
        );
        if let Err(error) = result {
            interrupted(cancelled)?;
            let retry = error
                .downcast_ref::<protocol::Failure>()
                .is_some_and(|f| f.retryable == Some(true) && f.retry_after_seconds().is_none());
            if state.committed > before {
                failures = 0;
            } else {
                failures += 1;
            }
            if !retry || failures >= 3 {
                return Err(error);
            }
            crate::download::http::pause(1_000u64 << failures.min(3), cancelled)?;
        }
    }
    if format!("{:x}", hash.finalize()) != asset.sha {
        return Err(invalid());
    }
    if !valid_final(destination, asset)? {
        fs::hard_link(&partial, destination)?;
        storage::sync_parent(destination)?;
    }
    fs::remove_file(&partial)?;
    fs::remove_file(&receipt)?;
    storage::sync_parent(&partial)?;
    protocol::progress(
        "setup_download",
        "verified setup asset",
        serde_json::json!({"asset":asset.name,"downloaded_bytes":asset.bytes,"total_bytes":asset.bytes,"durable":true}),
    );
    Ok(())
}
// Keep the owned file, durable cursor and incremental hash explicit at the I/O boundary.
#[allow(clippy::too_many_arguments)]
fn attempt(
    agent: &ureq::Agent,
    validate: fn(&str) -> Result<()>,
    cache: &Path,
    file: &mut fs::File,
    receipt: &Path,
    state: &mut State,
    hash: &mut Sha256,
    asset: Asset,
    cancelled: &AtomicBool,
) -> Result<()> {
    let start = state.committed;
    let end = start.saturating_add(CHUNK - 1).min(asset.bytes - 1);
    let mut url = asset.url.to_owned();
    for _ in 0..8 {
        interrupted(cancelled)?;
        validate(&url)?;
        let mut response = agent
            .get(&url)
            .header("Accept-Encoding", "identity")
            .header("Range", format!("bytes={start}-{end}"))
            .call()
            .map_err(super::install::transport_error)?;
        let status = response.status().as_u16();
        if [301, 302, 303, 307, 308].contains(&status) {
            let next = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(invalid)?;
            url = if next.starts_with('/') && !next.starts_with("//") {
                let uri: ureq::http::Uri = url.parse().map_err(|_| invalid())?;
                format!(
                    "{}://{}{}",
                    uri.scheme_str().ok_or_else(invalid)?,
                    uri.authority().ok_or_else(invalid)?,
                    next
                )
            } else {
                next.to_owned()
            };
            continue;
        }
        if status == 429 || (status >= 400 && response.headers().contains_key("retry-after")) {
            let header = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok());
            let seconds = if header.is_some() {
                crate::download::http::retry_seconds(header, SystemTime::now())
            } else {
                Some(60)
            };
            let until = seconds
                .map(|seconds| now()?.checked_add(seconds).ok_or_else(invalid))
                .transpose()?;
            storage::atomic_write(
                &cache.join("retry-at.json"),
                &serde_json::to_vec(&until)?,
                true,
            )?;
            return Err(protocol::failure_details(
                if seconds.is_some() {
                    "setup_download_failed"
                } else {
                    "setup_retry_after_invalid"
                },
                Some(seconds.is_some()),
                "setup server requested a cooldown",
                serde_json::json!({"http_status":status,"retry_after_seconds":seconds,"preserved":true,"next_action":if seconds.is_some() { "retry_setup_after_delay" } else { "inspect_retry_guidance" }}),
            ));
        }
        if ![200, 206].contains(&status) {
            return Err(protocol::failure_details(
                "setup_download_failed",
                Some(matches!(status, 408 | 500 | 502 | 503 | 504)),
                "setup server refused the asset transfer",
                serde_json::json!({"http_status":status,"preserved":true}),
            ));
        }
        if response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
        {
            return Err(invalid());
        }
        let expected = if status == 206 {
            let range = format!("bytes {start}-{end}/{}", asset.bytes);
            if response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                != Some(range.as_str())
            {
                return Err(invalid());
            }
            end - start + 1
        } else {
            asset.bytes
        };
        if response
            .headers()
            .get("content-length")
            .is_some_and(|v| v.to_str().ok().and_then(|v| v.parse::<u64>().ok()) != Some(expected))
        {
            return Err(invalid());
        }
        let mut reader = response.body_mut().as_reader();
        // Servers may ignore Range. Prove the complete retained prefix before appending.
        if status == 200 && start > 0 {
            file.seek(SeekFrom::Start(0))?;
            let mut left = start;
            let mut a = [0u8; 64 * 1024];
            let mut b = [0u8; 64 * 1024];
            while left > 0 {
                interrupted(cancelled)?;
                let n = usize::try_from(left.min(a.len() as u64))?;
                reader.read_exact(&mut a[..n]).map_err(|_| network())?;
                file.read_exact(&mut b[..n])?;
                if a[..n] != b[..n] {
                    return Err(invalid());
                }
                left -= n as u64;
            }
        }
        file.seek(SeekFrom::Start(state.committed))?;
        let target = if status == 206 { end + 1 } else { asset.bytes };
        let mut buffer = [0u8; 64 * 1024];
        let mut checkpoint = state.committed;
        let outcome = (|| -> Result<()> {
            while state.committed < target {
                interrupted(cancelled)?;
                let n = usize::try_from((target - state.committed).min(buffer.len() as u64))?;
                let got = reader.read(&mut buffer[..n]).map_err(|_| network())?;
                if got == 0 {
                    return Err(network());
                }
                file.write_all(&buffer[..got])?;
                hash.update(&buffer[..got]);
                state.committed += got as u64;
                if state.committed - checkpoint >= 1024 * 1024 {
                    save(file, receipt, state, hash)?;
                    checkpoint = state.committed;
                }
            }
            let mut extra = [0];
            if reader.read(&mut extra).map_err(|_| network())? != 0 {
                return Err(invalid());
            }
            Ok(())
        })();
        save(file, receipt, state, hash)?;
        return outcome;
    }
    Err(failure(
        "setup_download_failed",
        "setup asset redirect limit exceeded",
    ))
}
#[cfg(test)]
mod tests;
