use super::*;
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    process::Command,
};
fn local(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url.parse()?;
    if uri.scheme_str() == Some("http") && uri.host() == Some("127.0.0.1") {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn asset(url: String, data: &[u8]) -> Asset {
    Asset {
        name: "fixture.bin",
        url: Box::leak(url.into_boxed_str()),
        bytes: data.len() as u64,
        sha: Box::leak(format!("{:x}", Sha256::digest(data)).into_boxed_str()),
    }
}
fn request(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut all = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        all.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    all
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent()
}
fn seed(root: &Path, asset: Asset, data: &[u8]) -> std::path::PathBuf {
    let dir = cache(root).unwrap();
    let partial = dir.join(format!("{}.part", asset.sha));
    fs::write(&partial, data).unwrap();
    let state = State {
        sha: asset.sha.into(),
        bytes: asset.bytes,
        committed: data.len() as u64,
        prefix_sha: format!("{:x}", Sha256::digest(data)),
    };
    storage::atomic_write(
        &dir.join(format!("{}.json", asset.sha)),
        &serde_json::to_vec(&state).unwrap(),
        false,
    )
    .unwrap();
    partial
}
#[test]
fn transfer_child_fixture() {
    let Ok(root) = std::env::var("AGENT_YOUTUBE_SETUP_TRANSFER_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let url = std::env::var("AGENT_YOUTUBE_SETUP_TRANSFER_URL").unwrap();
    fetch_with(
        &root,
        &root.join("fixture.bin"),
        asset(url, b"abcdefgh"),
        false,
        &AtomicBool::new(false),
        &agent(),
        local,
    )
    .unwrap();
}
#[test]
fn truncated_download_resumes_in_a_new_process_without_repeating_committed_prefix() {
    let root = super::super::tests::temp();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/asset", listener.local_addr().unwrap());
    let selected = asset(url.clone(), b"abcdefgh");
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for response in [
            "HTTP/1.1 206 Partial\r\nContent-Range: bytes 0-7/8\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabcd",
            "HTTP/1.1 403 Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-7/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\nefgh",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            seen.push(request(&mut stream).to_ascii_lowercase());
            stream.write_all(response.as_bytes()).unwrap();
        }
        seen
    });
    assert!(
        fetch_with(
            &root,
            &root.join("fixture.bin"),
            selected,
            false,
            &AtomicBool::new(false),
            &agent(),
            local
        )
        .is_err()
    );
    let partial = root
        .join(".setup-downloads")
        .join(format!("{}.part", selected.sha));
    assert_eq!(fs::read(&partial).unwrap(), b"abcd");
    assert!(!root.join("fixture.bin").exists());
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::transfer::tests::transfer_child_fixture",
        ])
        .env("AGENT_YOUTUBE_SETUP_TRANSFER_ROOT", &root)
        .env("AGENT_YOUTUBE_SETUP_TRANSFER_URL", url)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stdout)
    );
    assert_eq!(fs::read(root.join("fixture.bin")).unwrap(), b"abcdefgh");
    assert!(!partial.exists());
    let requests = server.join().unwrap();
    assert!(requests[0].contains("range: bytes=0-7"));
    assert!(requests[2].contains("range: bytes=4-7"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn changed_committed_prefix_and_foreign_partial_are_refused_without_network() {
    for changed in [false, true] {
        let root = super::super::tests::temp();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let selected = asset(
            format!("http://{}/asset", listener.local_addr().unwrap()),
            b"abcdefgh",
        );
        let partial = seed(&root, selected, b"abcd");
        if changed {
            fs::write(&partial, b"xxxx").unwrap();
        } else {
            fs::remove_file(partial.with_extension("json")).unwrap();
        }
        let before = fs::read(&partial).unwrap();
        assert!(
            fetch_with(
                &root,
                &root.join("fixture.bin"),
                selected,
                false,
                &AtomicBool::new(false),
                &agent(),
                local
            )
            .is_err()
        );
        assert_eq!(fs::read(&partial).unwrap(), before);
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn ignored_range_proves_retained_prefix_and_bad_ranges_never_publish() {
    for kind in ["full", "changed", "range"] {
        let root = super::super::tests::temp();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let selected = asset(
            format!("http://{}/asset", listener.local_addr().unwrap()),
            b"abcdefgh",
        );
        let partial = seed(&root, selected, b"abcd");
        let response = match kind {
            "full" => "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabcdefgh",
            "changed" => {
                "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nxxxxefgh"
            }
            _ => {
                "HTTP/1.1 206 Partial\r\nContent-Range: bytes 0-3/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\nefgh"
            }
        };
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            request(&mut s);
            s.write_all(response.as_bytes()).unwrap();
        });
        let result = fetch_with(
            &root,
            &root.join("fixture.bin"),
            selected,
            false,
            &AtomicBool::new(false),
            &agent(),
            local,
        );
        assert_eq!(result.is_ok(), kind == "full");
        if kind != "full" {
            assert_eq!(fs::read(partial).unwrap(), b"abcd");
            assert!(!root.join("fixture.bin").exists());
        }
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn cooldown_survives_a_second_invocation_without_another_request() {
    for header in ["120", "unparsed"] {
        let root = super::super::tests::temp();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let selected = asset(
            format!("http://{}/asset", listener.local_addr().unwrap()),
            b"abcdefgh",
        );
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            request(&mut s);
            write!(s,"HTTP/1.1 429 Limited\r\nRetry-After: {header}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener
        });
        let result = fetch_with(
            &root,
            &root.join("fixture.bin"),
            selected,
            false,
            &AtomicBool::new(false),
            &agent(),
            local,
        );
        let error = result.unwrap_err();
        let failure = error.downcast_ref::<protocol::Failure>().unwrap();
        assert_eq!(
            failure.code,
            if header == "120" {
                "setup_download_failed"
            } else {
                "setup_retry_after_invalid"
            }
        );
        assert_eq!(
            failure.details()["next_action"],
            if header == "120" {
                "retry_setup_after_delay"
            } else {
                "inspect_retry_guidance"
            }
        );
        let listener = server.join().unwrap();
        assert!(
            fetch_with(
                &root,
                &root.join("fixture.bin"),
                selected,
                false,
                &AtomicBool::new(false),
                &agent(),
                local
            )
            .is_err()
        );
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn final_hash_mismatch_and_changed_ready_files_are_not_overwritten() {
    let root = super::super::tests::temp();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let selected = asset(
        format!("http://{}/asset", listener.local_addr().unwrap()),
        b"abcdefgh",
    );
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        request(&mut s);
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nxxxxxxxx")
            .unwrap();
    });
    assert!(
        fetch_with(
            &root,
            &root.join("fixture.bin"),
            selected,
            false,
            &AtomicBool::new(false),
            &agent(),
            local
        )
        .is_err()
    );
    assert!(!root.join("fixture.bin").exists());
    server.join().unwrap();
    fs::write(root.join("fixture.bin"), b"foreign").unwrap();
    assert!(
        fetch_with(
            &root,
            &root.join("fixture.bin"),
            selected,
            false,
            &AtomicBool::new(false),
            &agent(),
            local
        )
        .is_err()
    );
    assert_eq!(fs::read(root.join("fixture.bin")).unwrap(), b"foreign");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn linked_partial_cannot_modify_an_external_file() {
    use std::os::unix::fs::symlink;
    for hard in [false, true] {
        let root = super::super::tests::temp();
        let selected = asset("http://127.0.0.1:1/asset".into(), b"abcdefgh");
        let partial = seed(&root, selected, b"abcd");
        let outside = root.join("keep");
        fs::write(&outside, b"abcdefgh").unwrap();
        fs::remove_file(&partial).unwrap();
        if hard {
            fs::hard_link(&outside, &partial).unwrap();
        } else {
            symlink(&outside, &partial).unwrap();
        }
        assert!(
            fetch_with(
                &root,
                &root.join("fixture.bin"),
                selected,
                false,
                &AtomicBool::new(false),
                &agent(),
                local
            )
            .is_err()
        );
        assert_eq!(fs::read(outside).unwrap(), b"abcdefgh");
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn uncommitted_owned_tail_is_rolled_back_before_range_resume() {
    let root = super::super::tests::temp();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let selected = asset(
        format!("http://{}/asset", listener.local_addr().unwrap()),
        b"abcdefgh",
    );
    let partial = seed(&root, selected, b"abcd");
    fs::OpenOptions::new()
        .append(true)
        .open(&partial)
        .unwrap()
        .write_all(b"uncommitted")
        .unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        assert!(
            request(&mut s)
                .to_ascii_lowercase()
                .contains("range: bytes=4-7")
        );
        s.write_all(b"HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-7/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\nefgh").unwrap();
    });
    fetch_with(
        &root,
        &root.join("fixture.bin"),
        selected,
        false,
        &AtomicBool::new(false),
        &agent(),
        local,
    )
    .unwrap();
    assert_eq!(fs::read(root.join("fixture.bin")).unwrap(), b"abcdefgh");
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_setup_publishes_a_fully_committed_verified_download_without_network() {
    let root = super::super::tests::temp();
    let selected = asset("http://127.0.0.1:1/asset".into(), b"abcdefgh");
    seed(&root, selected, b"abcdefgh");
    fetch_with(
        &root,
        &root.join("fixture.bin"),
        selected,
        true,
        &AtomicBool::new(false),
        &agent(),
        local,
    )
    .unwrap();
    assert_eq!(fs::read(root.join("fixture.bin")).unwrap(), b"abcdefgh");
    fs::remove_dir_all(root).unwrap();
}
