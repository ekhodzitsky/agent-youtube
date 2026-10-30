use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn sources(path: &Path, bytes: &mut Vec<u8>) -> std::io::Result<()> {
    if path.is_file() {
        bytes.extend_from_slice(path.to_string_lossy().as_bytes());
        bytes.push(0);
        let content = fs::read(path)?;
        bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&content);
    } else if path.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            sources(&entry.path(), bytes)?;
        }
    }
    Ok(())
}
fn git(args: &[&str]) -> Option<String> {
    // A nested research snapshot may see its parent's .git directory. That
    // parent's clean status and revision do not describe this source tree.
    let top = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !top.status.success()
        || Path::new(String::from_utf8_lossy(&top.stdout).trim())
            .canonicalize()
            .ok()?
            != std::env::current_dir().ok()?.canonicalize().ok()?
    {
        return None;
    }
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths = [
        "src",
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "vendor/syom",
        "vendor/bandlimit",
        "vendor/botguard",
        "vendor/rustls-rustcrypto",
        "vendor/rustls-rustcrypto-current",
        "experiments/botguard/vendor/boa_ast",
        "experiments/botguard/vendor/boa_engine",
    ];
    let mut bytes = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed={path}");
        sources(Path::new(path), &mut bytes)?;
    }
    for path in [".git/HEAD", ".git/refs/heads/main", ".git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let fingerprint = (|| -> Option<String> {
        let mut child = Command::new("git")
            .args(["hash-object", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(&bytes).ok()?;
        let out = child.wait_with_output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    })()
    .unwrap_or_else(|| "unavailable".into());
    println!("cargo:rustc-env=AGENT_YOUTUBE_SOURCE_FINGERPRINT={fingerprint}");
    let mut status = vec!["status", "--porcelain", "--untracked-files=normal", "--"];
    status.extend(paths);
    println!(
        "cargo:rustc-env=AGENT_YOUTUBE_SOURCE_DIRTY={}",
        git(&status)
            .map(|s| !s.is_empty())
            .map_or("unknown", |dirty| if dirty { "true" } else { "false" })
    );
    println!(
        "cargo:rustc-env=AGENT_YOUTUBE_REVISION={}",
        git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unavailable".into())
    );
    Ok(())
}
