//! Explicit, versioned local recognizer preparation. Discovery never installs.
use crate::{Result, pipeline::ModelVariant, protocol, storage};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
mod install;

pub(crate) const DEFAULT_ROOT: &str = "tools-local/runtime";
const VERSION: &str = "2.22.0";
const PLATFORM: &str = "x86_64-unknown-linux-gnu";
const BINARY: &str = "gigastt-2.22.0";
const BINARY_SHA: &str = "efb3c09b0ddf7872fe47bccc91f556ae728cf01035a94efe6f8e0c0795591cd1";
const OWNER: &str = "agent-youtube managed runtime v1\n";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    version: String,
    platform: String,
    executable_sha256: String,
    model_dir: PathBuf,
    #[serde(default)]
    model_variant: ModelVariant,
    model_fingerprint: BTreeMap<String, String>,
}

pub(crate) struct Selection {
    pub gigastt: PathBuf,
    pub model_dir: PathBuf,
    pub model_variant: ModelVariant,
}

fn failure(code: &'static str, message: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure_details(
        code,
        Some(false),
        message,
        serde_json::json!({"preserved":true,"next_action":"run_setup_with_valid_models_or_use_explicit_gigastt"}),
    )
}

fn platform() -> Result<()> {
    if !cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        return Err(failure(
            "setup_platform_unsupported",
            "managed installation supports Linux x86_64 GNU; supply --gigastt explicitly on other platforms",
        ));
    }
    Ok(())
}

fn models(dir: &Path, variant: ModelVariant) -> Result<BTreeMap<String, String>> {
    variant
        .files()
        .iter()
        .map(|name| {
            let path = dir.join(name);
            let file = storage::open_regular_input(&path)?;
            if file.metadata()?.len() == 0 {
                return Err("empty model file".into());
            }
            Ok(((*name).to_owned(), storage::digest(&path)?))
        })
        .collect()
}

fn owner(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("runtime root must be a real directory".into());
    }
    let marker = root.join(".runtime-owner");
    storage::reject_nonregular(&marker)?;
    if storage::read_text(&marker, 128)? != OWNER {
        return Err("unrecognized runtime ownership marker".into());
    }
    Ok(())
}

fn load(root: &Path) -> Result<Selection> {
    platform()?;
    owner(root)?;
    let path = root.join("runtime.json");
    storage::reject_nonregular(&path)?;
    let manifest: Manifest = serde_json::from_str(&storage::read_text(&path, 16 << 10)?)?;
    if manifest.schema_version != 1
        || manifest.version != VERSION
        || manifest.platform != PLATFORM
        || manifest.executable_sha256 != BINARY_SHA
        || !manifest.model_dir.is_absolute()
    {
        return Err("unsupported managed runtime identity".into());
    }
    let root = fs::canonicalize(root)?;
    let binary = root.join(BINARY);
    storage::reject_nonregular(&binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&binary)?.permissions().mode() & 0o111 == 0 {
            return Err("managed executable is not executable".into());
        }
    }
    if storage::digest(&binary)? != BINARY_SHA
        || models(&manifest.model_dir, manifest.model_variant)? != manifest.model_fingerprint
    {
        return Err("managed executable or models changed".into());
    }
    Ok(Selection {
        gigastt: binary,
        model_dir: manifest.model_dir,
        model_variant: manifest.model_variant,
    })
}

pub(crate) fn select(root: &Path, required: bool) -> Result<Option<Selection>> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Ok(None),
        Err(_) => Err(failure(
            "runtime_invalid",
            "prepared runtime is unavailable; run setup or supply --gigastt explicitly",
        )),
        Ok(_) => load(root).map(Some).map_err(|_| {
            failure(
                "runtime_invalid",
                "prepared runtime is incomplete or changed; no PATH fallback was attempted",
            )
        }),
    }
}

pub(crate) fn setup(
    root: &Path,
    model_dir: Option<PathBuf>,
    archive: Option<PathBuf>,
    model_variant: ModelVariant,
    cancelled: &AtomicBool,
) -> Result<serde_json::Value> {
    platform()?;
    if cancelled.load(Ordering::SeqCst) {
        return Err(protocol::failure(
            "interrupted",
            Some(true),
            "setup interrupted",
        ));
    }
    // An existing manifest is immutable: repeated setup validates it without updates.
    if root.join("runtime.json").try_exists()? {
        let selected = select(root, true)?.ok_or("missing runtime")?;
        if selected.model_variant != model_variant {
            return Err(failure(
                "setup_config_mismatch",
                "existing runtime selects a different model variant; use a new runtime directory",
            ));
        }
        if let Some(path) = model_dir
            && fs::canonicalize(path)? != selected.model_dir
        {
            return Err(failure(
                "setup_config_mismatch",
                "existing runtime selects different models; use a new runtime directory",
            ));
        }
        return Ok(result(root, &selected, true));
    }
    let model_dir = model_dir
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".gigastt/models")))
        .ok_or_else(|| {
            failure(
                "setup_models_invalid",
                "specify --model-dir with the selected INT8 model files",
            )
        })?;
    let model_fingerprint = models(&model_dir, model_variant).map_err(|_| failure("setup_models_invalid", "required selected model files must be readable nonempty regular files; provision models explicitly"))?;
    let model_dir = fs::canonicalize(model_dir)?;
    if root.try_exists()? {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(failure(
                "setup_directory_not_owned",
                "refusing non-directory or linked runtime root",
            ));
        }
        if fs::read_dir(root)?.next().transpose()?.is_some() {
            owner(root).map_err(|_| {
                failure(
                    "setup_directory_not_owned",
                    "refusing a nonempty directory without a valid runtime ownership marker",
                )
            })?;
        }
    }
    storage::private_directory(root, true)?;
    let _lock = storage::Lock::acquire(root)?;
    let marker = root.join(".runtime-owner");
    if !marker.try_exists()? {
        storage::atomic_write(&marker, OWNER.as_bytes(), false)?;
    }
    owner(root)?;
    // Recheck after locking: another setup may have finished in the meantime.
    if root.join("runtime.json").try_exists()? {
        let selected = select(root, true)?.ok_or("missing runtime")?;
        if selected.model_dir != model_dir || selected.model_variant != model_variant {
            return Err(failure(
                "setup_config_mismatch",
                "existing runtime selects different models; use a new runtime directory",
            ));
        }
        return Ok(result(root, &selected, true));
    }
    install::install(root, archive.as_deref(), cancelled)?;
    if models(&model_dir, model_variant)? != model_fingerprint {
        return Err(failure(
            "setup_models_invalid",
            "model content changed during installation; runtime manifest was not published",
        ));
    }
    let manifest = Manifest {
        schema_version: 1,
        version: VERSION.into(),
        platform: PLATFORM.into(),
        executable_sha256: BINARY_SHA.into(),
        model_dir,
        model_fingerprint,
        model_variant,
    };
    if cancelled.load(Ordering::SeqCst) {
        return Err(protocol::failure(
            "interrupted",
            Some(true),
            "setup interrupted; prepared files retained",
        ));
    }
    storage::atomic_write(
        &root.join("runtime.json"),
        &serde_json::to_vec_pretty(&manifest)?,
        false,
    )?;
    let selected = select(root, true)?.ok_or("missing prepared runtime")?;
    Ok(result(root, &selected, false))
}

fn result(root: &Path, selected: &Selection, reused: bool) -> serde_json::Value {
    serde_json::json!({"runtime_dir":root,"gigastt":selected.gigastt,"gigastt_version":VERSION,"model_dir":selected.model_dir,"model_variant":selected.model_variant,
        "reused":reused,"checks":{"executable_sha256":true,"model_files_sha256":true,"model_load":"not_checked","inference":"not_checked","youtube_access":"not_checked"}})
}

#[cfg(test)]
mod tests;
