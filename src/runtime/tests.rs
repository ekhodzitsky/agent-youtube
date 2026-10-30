use super::*;
use std::sync::atomic::AtomicU64;
static NEXT: AtomicU64 = AtomicU64::new(0);
#[path = "../../tests/support/contract.rs"]
mod contract;

#[test]
fn setup_result_serializer_matches_the_machine_contract() {
    let root = temp();
    let selected = Selection {
        gigastt: root.join(BINARY),
        model_dir: root.join("models"),
    };
    let documents = [false, true].map(|reused| {
        serde_json::json!({
            "schema_version":1,"ok":true,"command":"setup",
            "result":result(&root, &selected, reused)
        })
    });
    contract::validate(&documents);
    fs::remove_dir_all(root).unwrap();
}
pub(super) fn temp() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "agent-youtube-runtime-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    root
}
#[test]
fn invalid_runtime_is_never_repaired_or_ignored_by_discovery() {
    let root = temp();
    fs::write(root.join("unrelated"), b"preserve").unwrap();
    assert!(select(&root, false).is_err());
    assert_eq!(fs::read(root.join("unrelated")).unwrap(), b"preserve");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn missing_optional_runtime_preserves_legacy_selection() {
    let root = temp();
    assert!(select(&root.join("absent"), false).unwrap().is_none());
    assert!(select(&root.join("absent"), true).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn setup_rejects_foreign_directories_and_bad_archives_without_manifest() {
    let root = temp();
    let model_dir = root.join("models");
    fs::create_dir(&model_dir).unwrap();
    for name in pipeline::MODEL_FILES {
        fs::write(model_dir.join(name), b"fixture").unwrap();
    }
    let foreign = root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("keep"), b"unchanged").unwrap();
    let cancelled = AtomicBool::new(false);
    let error = setup(&foreign, Some(model_dir.clone()), None, &cancelled).unwrap_err();
    assert_eq!(
        error.downcast_ref::<protocol::Failure>().unwrap().code,
        "setup_directory_not_owned"
    );
    assert_eq!(fs::read(foreign.join("keep")).unwrap(), b"unchanged");
    let bad = root.join("bad.tar.gz");
    fs::write(&bad, b"not a release").unwrap();
    let target = root.join("runtime");
    let error = setup(&target, Some(model_dir), Some(bad), &cancelled).unwrap_err();
    assert_eq!(
        error.downcast_ref::<protocol::Failure>().unwrap().code,
        "setup_archive_invalid"
    );
    assert!(!target.join("runtime.json").exists());
    assert!(select(&target, false).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[cfg(unix)]
#[test]
fn linked_runtime_and_manifest_are_not_followed() {
    use std::os::unix::fs::symlink;
    let root = temp();
    let actual = root.join("real");
    fs::create_dir(&actual).unwrap();
    symlink(&actual, root.join("linked")).unwrap();
    assert!(select(&root.join("linked"), false).is_err());
    fs::write(actual.join(".runtime-owner"), OWNER).unwrap();
    fs::write(root.join("secret"), b"private fixture").unwrap();
    symlink(root.join("secret"), actual.join("runtime.json")).unwrap();
    assert!(select(&actual, true).is_err());
    assert_eq!(fs::read(root.join("secret")).unwrap(), b"private fixture");
    fs::remove_dir_all(root).unwrap();
}
