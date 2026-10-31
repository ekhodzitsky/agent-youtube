use super::*;
use std::{
    io::{BufRead, BufReader},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
const ID: &str = "a2hbJlPxj4M";

fn refresh_page(request: &Request, player: Value) -> Reply {
    let client = if request.headers["user-agent"] == MWEB_USER_AGENT {
        "MWEB"
    } else {
        "WEB"
    };
    let config = serde_json::json!({"STS":1,"INNERTUBE_CONTEXT":{"client":{"clientName":client,"clientVersion":"fixture","visitorData":"fixture-visitor"}},"WEB_PLAYER_CONTEXT_CONFIGS":{"WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER":{"encryptedHostFlags":"fixture-flags"}}});
    Reply::bytes(format!("var ytInitialPlayerResponse={player};ytcfg.set({config});").into_bytes())
}

#[test]
fn exhausted_transient_download_retains_status_and_can_resume() {
    let failing = Arc::new(AtomicBool::new(true));
    let flag = failing.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            return metadata(o, 3);
        }
        observed.fetch_add(1, Ordering::SeqCst);
        if flag.load(Ordering::SeqCst) {
            Reply {
                status: 503,
                headers: vec![],
                body: vec![],
            }
        } else {
            range(&r, b"aac")
        }
    });
    let error = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap_err();
    let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
    assert_eq!(failure.code, "download_http_temporary");
    assert_eq!(failure.retryable, Some(true));
    assert_eq!(failure.http_status(), Some(503));
    assert_eq!(failure.details()["attempts"], 4);
    assert_eq!(failure.details()["retries_exhausted"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let receipt = fs::read(suffix(&server.prefix(), ".json")).unwrap();
    assert!(!receipt.is_empty());
    assert_eq!(
        fs::metadata(suffix(&server.prefix(), ".m4a"))
            .unwrap()
            .len(),
        0
    );
    failing.store(false, Ordering::SeqCst);
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"aac");
}

#[cfg(unix)]
#[test]
fn every_download_outcome_releases_worker_for_long_lived_callers() {
    for outcome in ["metadata failure", "media failure", "cancelled", "success"] {
        let server = Server::new(move |request, origin| {
            if outcome == "metadata failure" {
                Reply::bytes(b"invalid metadata".to_vec())
            } else if request.path.starts_with("/youtubei/") {
                metadata(origin, 3)
            } else if outcome == "media failure" {
                Reply {
                    status: 429,
                    headers: vec![],
                    body: vec![],
                }
            } else {
                range(&request, b"aac")
            }
        });
        let downloader = Downloader::loopback(&server.origin).unwrap();
        let (worker, pid) = crate::web::Worker::idle_fixture();
        *downloader.runtime.lock().unwrap() = Some(worker);
        let result = downloader.download(
            ID,
            &server.root.join("download"),
            &AtomicBool::new(outcome == "cancelled"),
        );
        assert_eq!(result.is_ok(), outcome == "success");
        assert!(
            downloader.runtime.lock().unwrap().is_none(),
            "worker survived {outcome}"
        );
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

struct Request {
    path: String,
    headers: std::collections::BTreeMap<String, String>,
    body: Vec<u8>,
}
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }
}
struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
    root: PathBuf,
}
impl Server {
    fn new(handler: impl Fn(Request, &str) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{addr}");
        let base = origin.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let task = thread::spawn(move || {
            while !signal.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut headers = std::collections::BTreeMap::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    let (k, v) = line.split_once(':').unwrap();
                    headers.insert(k.to_ascii_lowercase(), v.trim().into());
                }
                let length = headers
                    .get("content-length")
                    .and_then(|s: &String| s.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let reply = handler(
                    Request {
                        path,
                        headers,
                        body,
                    },
                    &base,
                );
                let mut header = format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.status);
                if !reply
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                {
                    header.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
                }
                for (k, v) in reply.headers {
                    header.push_str(&format!("{k}: {v}\r\n"));
                }
                header.push_str("\r\n");
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        let root = std::env::temp_dir().join(format!(
            "agent-youtube-download-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self {
            origin,
            stop,
            task: Some(task),
            root,
        }
    }
    fn prefix(&self) -> PathBuf {
        self.root.join("download.1.2.3")
    }
    fn downloader(&self) -> Downloader {
        Downloader::loopback(&self.origin).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.origin.trim_start_matches("http://"));
        if let Some(t) = self.task.take() {
            t.join().unwrap();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn format(length: usize) -> Value {
    serde_json::json!({"itag":140,"lastModified":"12345","mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","bitrate":128000,"contentLength":length.to_string()})
}
fn player(origin: &str, length: usize) -> Value {
    let mut f = format(length);
    f["url"] = serde_json::json!(format!("{origin}/audio"));
    serde_json::json!({"playabilityStatus":{"status":"OK"},"videoDetails":{"videoId":ID,"title":"Original recording","lengthSeconds":"12"},"streamingData":{"adaptiveFormats":[f]}})
}
fn metadata(origin: &str, length: usize) -> Reply {
    Reply::bytes(serde_json::to_vec(&player(origin, length)).unwrap())
}
fn range(request: &Request, data: &[u8]) -> Reply {
    let value = request
        .headers
        .get("range")
        .unwrap()
        .strip_prefix("bytes=")
        .unwrap();
    let (a, b) = value.split_once('-').unwrap();
    let a = a.parse::<usize>().unwrap();
    let b = b.parse::<usize>().unwrap();
    Reply {
        status: 206,
        headers: vec![
            (
                "Content-Range".into(),
                format!("bytes {a}-{b}/{}", data.len()),
            ),
            ("ETag".into(), "\"original\"".into()),
        ],
        body: data[a..=b].to_vec(),
    }
}
#[test]
fn download_capacity_refusal_preserves_validated_media() {
    let data = vec![42; RANGE_BYTES as usize + 23];
    let request_count = Arc::new(AtomicUsize::new(0));
    let seen = request_count.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            metadata(o, data.len())
        } else {
            seen.fetch_add(1, Ordering::SeqCst);
            range(&r, &data)
        }
    });
    // Neither exhausted bytes nor exhausted inodes may open/write the audio source.
    for (bytes, files) in [(0, Some(100)), (u64::MAX, Some(0))] {
        let error = crate::storage::with_space(
            crate::storage::DiskSpace {
                available_bytes: bytes,
                allocation_unit: 4096,
                available_files: files,
            },
            || {
                server
                    .downloader()
                    .download(ID, &server.prefix(), &AtomicBool::new(false))
            },
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "insufficient_disk_space"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 0);
        assert!(!suffix(&server.prefix(), ".m4a").exists());
    }
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    let before = fs::read(&path).unwrap();
    // A metadata refresh still needs space; refusal must preserve the retained source.
    let journal = suffix(&server.prefix(), ".json");
    let receipt = fs::read(&journal).unwrap();
    let error = crate::storage::with_space(
        crate::storage::DiskSpace {
            available_bytes: 0,
            allocation_unit: 4096,
            available_files: Some(0),
        },
        || {
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
        },
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "insufficient_disk_space"
    );
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(fs::read(journal).unwrap(), receipt);
}

#[test]
fn range_download_uses_exact_offsets_and_validates_identity() {
    let data = vec![42; RANGE_BYTES as usize + 23];
    let expected = data.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            metadata(o, data.len())
        } else {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            if n > 0 {
                assert_eq!(r.headers["if-range"], "\"original\"");
            }
            range(&r, &data)
        }
    });
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}
#[test]
fn ignored_or_malformed_ranges_cannot_commit_audio() {
    for fault in ["ignored", "start", "total", "length", "excess", "etag"] {
        let server = Server::new(move |r, o| {
            if r.path.starts_with("/youtubei/") {
                return metadata(o, 100);
            }
            let mut reply = range(&r, &[42; 100]);
            match fault {
                "ignored" => reply.status = 200,
                "start" => reply.headers[0].1 = "bytes 1-99/100".into(),
                "total" => reply.headers[0].1 = "bytes 0-99/101".into(),
                "length" => reply.headers.push(("Content-Length".into(), "99".into())),
                "excess" => reply.body.push(42),
                _ => reply.status = 404,
            }
            reply
        });
        assert!(
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .is_err(),
            "{fault}"
        );
        assert_eq!(
            fs::metadata(suffix(&server.prefix(), ".m4a"))
                .unwrap()
                .len(),
            0,
            "{fault}"
        );
    }
}
#[test]
fn last_range_body_failure_is_retried_without_false_eof() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let players = Arc::new(AtomicUsize::new(0));
    let observed = players.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            observed.fetch_add(1, Ordering::SeqCst);
            return metadata(o, 100);
        }
        let mut reply = range(&r, &[7; 100]);
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            reply.body.truncate(30);
            reply.headers.push(("Content-Length".into(), "100".into()));
        }
        reply
    });
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), [7; 100]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(players.load(Ordering::SeqCst), 1);
}
#[test]
fn durable_partial_resumes_and_corruption_is_preserved() {
    for corrupt in [false, true] {
        let fail = Arc::new(AtomicBool::new(true));
        let failed = fail.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = Server::new(move |r, o| {
            if r.path.starts_with("/youtubei/") {
                return metadata(o, RANGE_BYTES as usize + 23);
            }
            seen.fetch_add(1, Ordering::SeqCst);
            if failed.load(Ordering::SeqCst)
                && r.headers["range"].starts_with(&format!("bytes={RANGE_BYTES}-"))
            {
                return Reply {
                    status: 503,
                    headers: vec![],
                    body: vec![],
                };
            }
            range(&r, &vec![9; RANGE_BYTES as usize + 23])
        });
        assert!(
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .is_err()
        );
        let partial = suffix(&server.prefix(), ".m4a");
        assert_eq!(fs::metadata(&partial).unwrap().len(), RANGE_BYTES);
        fail.store(false, Ordering::SeqCst);
        let before = requests.load(Ordering::SeqCst);
        if corrupt {
            File::options()
                .write(true)
                .open(&partial)
                .unwrap()
                .write_all(b"changed")
                .unwrap();
        } else {
            File::options()
                .append(true)
                .open(&partial)
                .unwrap()
                .write_all(b"uncommitted crash tail")
                .unwrap();
        }
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        if corrupt {
            assert!(result.unwrap_err().to_string().contains("integrity"));
            assert_eq!(requests.load(Ordering::SeqCst), before);
            assert!(fs::read(partial).unwrap().starts_with(b"changed"));
        } else {
            assert_eq!(
                fs::read(result.unwrap()).unwrap(),
                vec![9; RANGE_BYTES as usize + 23]
            );
            assert_eq!(requests.load(Ordering::SeqCst), before + 1);
        }
    }
}
#[test]
fn cancelled_download_preserves_receipts_without_another_request() {
    let server = Server::new(|_, o| metadata(o, 100));
    let error = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("interrupted"));
    let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
    assert_eq!(failure.code, "interrupted");
    assert_eq!(failure.retryable, Some(true));
    assert!(!suffix(&server.prefix(), ".m4a").exists());
}

#[test]
fn source_lease_duration_must_match_the_receipt_when_both_are_known() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let downloader = server.downloader();
    let mut value = player(&server.origin, 4);
    value["videoDetails"]["lengthSeconds"] = "53".into();
    value["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/audio?expire=4102444800", server.origin).into();
    let source = parse_source(serde_json::to_vec(&value).unwrap(), ID, &downloader.http).unwrap();
    let mut receipt = Receipt {
        version: 1,
        identity: source.identity.clone(),
        progress: Progress::default(),
        etag: None,
        web_client: None,
        lease: lease::save(&source, &server.prefix()).unwrap(),
        duration_seconds: Some(54),
    };
    assert!(lease::load(&server.prefix(), &receipt, ID, &downloader.http).is_err());
    receipt.duration_seconds = Some(53);
    assert_eq!(
        lease::load(&server.prefix(), &receipt, ID, &downloader.http)
            .unwrap()
            .unwrap()
            .duration_seconds,
        Some(53)
    );
    receipt.duration_seconds = None;
    assert!(
        lease::load(&server.prefix(), &receipt, ID, &downloader.http)
            .unwrap()
            .is_some()
    );
}

#[test]
fn restart_reuses_unexpired_source_when_player_is_blocked() {
    let blocked = Arc::new(AtomicBool::new(false));
    let fail = blocked.clone();
    let players = Arc::new(AtomicUsize::new(0));
    let seen = players.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            seen.fetch_add(1, Ordering::SeqCst);
            if fail.load(Ordering::SeqCst) {
                return Reply {
                    status: 403,
                    headers: vec![],
                    body: vec![],
                };
            }
            let mut value = player(origin, RANGE_BYTES as usize + 23);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/audio?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        if !fail.load(Ordering::SeqCst)
            && request.headers["range"].starts_with(&format!("bytes={RANGE_BYTES}-"))
        {
            return Reply {
                status: 429,
                headers: vec![("Retry-After".into(), "120".into())],
                body: vec![],
            };
        }
        range(&request, &vec![9; RANGE_BYTES as usize + 23])
    });
    let prefix = server.prefix();
    let run = |expect_failure: bool| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "download::tests::source_lease_process_fixture",
                "--nocapture",
            ])
            .env("AGENT_YOUTUBE_LEASE_CHILD_ORIGIN", &server.origin)
            .env("AGENT_YOUTUBE_LEASE_CHILD_PREFIX", &prefix)
            .env(
                "AGENT_YOUTUBE_LEASE_CHILD_FAIL",
                if expect_failure { "1" } else { "0" },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    run(true);
    assert_eq!(
        fs::metadata(suffix(&prefix, ".m4a")).unwrap().len(),
        RANGE_BYTES
    );
    blocked.store(true, Ordering::SeqCst);
    run(false);
    assert_eq!(
        fs::read(suffix(&prefix, ".m4a")).unwrap(),
        vec![9; RANGE_BYTES as usize + 23]
    );
    assert_eq!(players.load(Ordering::SeqCst), 1);
}

#[test]
fn complete_verified_audio_does_not_need_youtube_or_a_live_source_lease() {
    for fault in ["none", "prefix", "lease"] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = Server::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Reply {
                status: 403,
                headers: vec![],
                body: vec![],
            }
        });
        let mut receipt = seed_source_lease(&server, None);
        receipt.progress.bytes = 4;
        receipt.progress.digest = format!("{:x}", Sha256::digest([9; 4]));
        rewrite_source_lease(&server, &mut receipt, |value| {
            value["expires_at"] = 0.into()
        });
        fs::write(
            suffix(&server.prefix(), ".m4a"),
            if fault == "prefix" {
                vec![8, 9, 9, 9]
            } else {
                vec![9, 9, 9, 9, 10]
            },
        )
        .unwrap();
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        if fault == "lease" {
            fs::write(
                suffix(
                    &server.prefix(),
                    &format!(".lease-{}", receipt.lease.as_ref().unwrap()),
                ),
                b"corrupt lease",
            )
            .unwrap();
        }
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        if fault != "none" {
            assert!(result.unwrap_err().to_string().contains("integrity"));
            let expected = if fault == "prefix" {
                vec![8, 9, 9, 9]
            } else {
                vec![9, 9, 9, 9, 10]
            };
            assert_eq!(
                fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
                expected
            );
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
    }
}

#[test]
fn expired_lease_does_not_hide_a_fresh_matching_metadata_bootstrap() {
    let server = Server::new(|request, _| {
        if request.path.starts_with("/fresh?") {
            return range(&request, &[9; 4]);
        }
        Reply {
            status: 403,
            headers: vec![],
            body: vec![],
        }
    });
    let mut receipt = seed_source_lease(&server, None);
    rewrite_source_lease(&server, &mut receipt, |value| {
        value["expires_at"] = 0.into()
    });
    let downloader = server.downloader();
    let mut root = player(&server.origin, 4);
    root["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/fresh?expire=4102444800", server.origin).into();
    let source = parse_source(serde_json::to_vec(&root).unwrap(), ID, &downloader.http).unwrap();
    *downloader.bootstrap.lock().unwrap() = Some((Instant::now(), source));
    let path = downloader
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), [9; 4]);
}

#[cfg(unix)]
#[test]
fn interrupted_player_script_is_retried_before_mutating_the_n_challenge() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = Server::new(move |request, _| {
        assert!(request.path.starts_with("/s/player/"));
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            return Reply {
                status: 200,
                headers: vec![("Content-Length".into(), "100".into())],
                body: b"truncated".to_vec(),
            };
        }
        Reply::bytes(b"player script fixture".to_vec())
    });
    let downloader = server.downloader();
    let mut root = player(&server.origin, 4);
    root["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/audio?n=challenge", server.origin).into();
    let mut source =
        parse_source(serde_json::to_vec(&root).unwrap(), ID, &downloader.http).unwrap();
    source.web_client = Some(WebClient::Samsung);
    source.player_script = Some("/s/player/abcd1234/player_ias.vflset/en_US/base.js".into());
    *downloader.runtime.lock().unwrap() = Some(crate::web::Worker::solution_fixture());
    downloader
        .prepare_web(&mut source, ID, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(query_value(&source.url, "n"), Some("solved"));
    if let Some(mut worker) = downloader.runtime.lock().unwrap().take() {
        worker.close().unwrap();
    }
}

#[test]
fn source_lease_process_fixture() {
    if std::env::var_os("AGENT_YOUTUBE_LEASE_CHILD_COOLDOWN").is_some() {
        let cancelled = AtomicBool::new(false);
        super::gate::Gate::user()
            .unwrap()
            .defer(120, &cancelled)
            .unwrap();
        let downloader = Downloader::youtube().unwrap();
        let mut source = parse_source(
            serde_json::to_vec(&player("https://r1.googlevideo.com", 4)).unwrap(),
            ID,
            &downloader.http,
        )
        .unwrap();
        source.web_client = Some(WebClient::Mweb);
        let error = downloader
            .refresh_token(&mut source, ID, false, &cancelled)
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "youtube_rate_limited"
        );
        assert!(downloader.runtime.lock().unwrap().is_none());
        return;
    }
    let Ok(origin) = std::env::var("AGENT_YOUTUBE_LEASE_CHILD_ORIGIN") else {
        return;
    };
    let prefix = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_LEASE_CHILD_PREFIX").unwrap());
    let result =
        Downloader::loopback(&origin)
            .unwrap()
            .download(ID, &prefix, &AtomicBool::new(false));
    assert_eq!(
        result.is_err(),
        std::env::var("AGENT_YOUTUBE_LEASE_CHILD_FAIL").unwrap() == "1",
        "{result:?}"
    );
}

#[test]
fn shared_cooldown_prevents_cached_source_token_issuance() {
    let server = Server::new(|_, _| Reply::bytes(vec![]));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "download::tests::source_lease_process_fixture",
            "--nocapture",
        ])
        .env("AGENT_YOUTUBE_LEASE_CHILD_COOLDOWN", "1")
        .env("XDG_CACHE_HOME", &server.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn seed_source_lease(server: &Server, web_client: Option<WebClient>) -> Receipt {
    let downloader = server.downloader();
    let mut player = player(&server.origin, 4);
    player["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/old?expire=4102444800", server.origin).into();
    let mut source =
        parse_source(serde_json::to_vec(&player).unwrap(), ID, &downloader.http).unwrap();
    source.web_client = web_client;
    if let Some(client) = web_client {
        source.client = serde_json::json!({"clientName":if client == WebClient::Web {"WEB"} else {"MWEB"},"clientVersion":"fixture"});
    }
    let receipt = Receipt {
        version: 1,
        identity: source.identity.clone(),
        web_client,
        etag: None,
        lease: lease::save(&source, &server.prefix()).unwrap(),
        duration_seconds: None,
        progress: Progress {
            bytes: 2,
            digest: format!("{:x}", Sha256::digest([9, 9])),
            ..Progress::default()
        },
    };
    fs::write(
        suffix(&server.prefix(), ".json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    fs::write(suffix(&server.prefix(), ".m4a"), [9, 9]).unwrap();
    receipt
}

fn rewrite_source_lease(server: &Server, receipt: &mut Receipt, mutate: impl FnOnce(&mut Value)) {
    let path = suffix(
        &server.prefix(),
        &format!(".lease-{}", receipt.lease.as_ref().unwrap()),
    );
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    fs::write(suffix(&server.prefix(), &format!(".lease-{digest}")), bytes).unwrap();
    receipt.lease = Some(digest);
    fs::write(
        suffix(&server.prefix(), ".json"),
        serde_json::to_vec(receipt).unwrap(),
    )
    .unwrap();
}

#[test]
fn refresh_retains_available_representation_when_a_higher_bitrate_appears() {
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            let mut value = player(origin, 4);
            let formats = value["streamingData"]["adaptiveFormats"]
                .as_array_mut()
                .unwrap();
            let mut higher = formats[0].clone();
            higher["itag"] = 141.into();
            higher["bitrate"] = 256000.into();
            formats.push(higher);
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        range(&request, &[9; 4])
    });
    let mut receipt = seed_source_lease(&server, None);
    rewrite_source_lease(&server, &mut receipt, |value| {
        value["expires_at"] = 0.into()
    });
    for _ in 0..2 {
        let downloader = server.downloader();
        downloader.metadata(ID, &AtomicBool::new(false)).unwrap();
        let path = downloader
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), [9; 4]);
        let saved: Receipt =
            serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
        assert_eq!(saved.identity, receipt.identity);
    }
}

#[test]
fn expired_source_refreshes_once_without_mixing_representations() {
    for changed in [false, true] {
        let players = Arc::new(AtomicUsize::new(0));
        let seen = players.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") || request.path.starts_with("/embed/") {
                return refresh_page(&request, player(origin, if changed { 5 } else { 4 }));
            }
            if request.path.starts_with("/youtubei/") {
                seen.fetch_add(1, Ordering::SeqCst);
                let mut value = player(origin, if changed { 5 } else { 4 });
                value["streamingData"]["adaptiveFormats"][0]["url"] =
                    format!("{origin}/new?expire=4102444800").into();
                return Reply::bytes(serde_json::to_vec(&value).unwrap());
            }
            assert!(request.path.starts_with("/new?"));
            assert_eq!(request.headers["range"], "bytes=2-3");
            range(&request, &[9; 4])
        });
        let mut receipt = seed_source_lease(&server, None);
        rewrite_source_lease(&server, &mut receipt, |value| {
            value["expires_at"] = 0.into()
        });
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        if changed {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("representation changed")
            );
            assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
            assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
        }
        assert_eq!(players.load(Ordering::SeqCst), if changed { 4 } else { 1 });
    }
}

#[test]
fn direct_403_retries_the_same_source_before_refresh() {
    for changed_etag in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let players = Arc::new(AtomicUsize::new(0));
        let observed = players.clone();
        let server = Server::new(move |request, origin| {
            if !request.path.starts_with("/old?") {
                observed.fetch_add(1, Ordering::SeqCst);
                return metadata(origin, 4);
            }
            assert_eq!(request.headers["range"], "bytes=2-3");
            assert_eq!(request.headers["if-range"], "\"fixture\"");
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                return Reply {
                    status: 403,
                    headers: vec![],
                    body: vec![],
                };
            }
            let mut reply = range(&request, &[9; 4]);
            reply.headers[1].1 = if changed_etag {
                "\"changed\""
            } else {
                "\"fixture\""
            }
            .into();
            reply
        });
        let mut receipt = seed_source_lease(&server, None);
        receipt.etag = Some("\"fixture\"".into());
        receipt.duration_seconds = Some(12);
        fs::write(
            suffix(&server.prefix(), ".json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        let start = Instant::now();
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(players.load(Ordering::SeqCst), 0);
        assert!(start.elapsed() >= Duration::from_secs(5));
        if changed_etag {
            assert!(result.unwrap_err().to_string().contains("ETag changed"));
            assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
            assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
        }
    }
}

#[test]
fn rejected_cached_source_refreshes_only_after_the_media_refusal() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/old?") {
            seen.lock().unwrap().push("old");
            return Reply {
                status: 403,
                headers: vec![],
                body: vec![],
            };
        }
        if request.path.starts_with("/youtubei/") {
            seen.lock().unwrap().push("player");
            let mut value = player(origin, 4);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/new?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        seen.lock().unwrap().push("new");
        assert!(request.path.starts_with("/new?"));
        range(&request, &[9; 4])
    });
    seed_source_lease(&server, None);
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), [9; 4]);
    assert_eq!(*order.lock().unwrap(), ["old", "old", "player", "new"]);
}

#[test]
fn direct_403_persistent_refusal_renews_before_excluding_the_client() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            seen.lock().unwrap().push("player");
            let mut value = player(origin, 4);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/new?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        seen.lock()
            .unwrap()
            .push(if request.path.starts_with("/old?") {
                "old"
            } else if request.path.starts_with("/new?") {
                "new"
            } else {
                "alternative"
            });
        Reply {
            status: 403,
            headers: vec![],
            body: vec![],
        }
    });
    seed_source_lease(&server, None);
    assert!(
        server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .is_err()
    );
    let calls = order.lock().unwrap();
    assert_eq!(&calls[..4], ["old", "old", "player", "new"]);
    assert_eq!(&calls[4..], ["alternative"; 5]);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert_eq!(receipt.progress.bytes, 2);
    assert_eq!(
        receipt.progress.digest,
        format!("{:x}", Sha256::digest([9, 9]))
    );
}

#[test]
fn direct_403_does_not_repeat_a_near_expiry_source_or_override_retry_after() {
    for cooldown in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                let mut value = player(origin, 4);
                value["streamingData"]["adaptiveFormats"][0]["url"] =
                    format!("{origin}/new?expire=4102444800").into();
                return Reply::bytes(serde_json::to_vec(&value).unwrap());
            }
            if request.path.starts_with("/old?") {
                seen.fetch_add(1, Ordering::SeqCst);
                return Reply {
                    status: 403,
                    headers: if cooldown {
                        vec![("Retry-After".into(), "120".into())]
                    } else {
                        vec![]
                    },
                    body: vec![],
                };
            }
            range(&request, &[9; 4])
        });
        let mut receipt = seed_source_lease(&server, None);
        if !cooldown {
            rewrite_source_lease(&server, &mut receipt, |value| {
                value["expires_at"] = (lease::now().unwrap() + 64).into();
            });
        }
        let start = Instant::now();
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        if cooldown {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .retry_after_seconds(),
                Some(120)
            );
            assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
        }
    }
}

#[test]
fn direct_403_stable_retry_consumes_the_existing_no_progress_budget() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = Server::new(move |request, _| {
        assert!(request.path.starts_with("/old?"));
        Reply {
            status: if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                403
            } else {
                503
            },
            headers: vec![],
            body: vec![],
        }
    });
    seed_source_lease(&server, None);
    let error = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap_err();
    let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
    assert_eq!(failure.http_status(), Some(503));
    assert_eq!(failure.details()["attempts"], 4);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn direct_403_stable_wait_is_cancellable_and_preserves_the_receipt() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = Server::new(move |request, _| {
        assert!(request.path.starts_with("/old?"));
        seen.fetch_add(1, Ordering::SeqCst);
        let flag = flag.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        Reply {
            status: 403,
            headers: vec![],
            body: vec![],
        }
    });
    let mut receipt = seed_source_lease(&server, None);
    receipt.duration_seconds = Some(12);
    fs::write(
        suffix(&server.prefix(), ".json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
    let start = Instant::now();
    assert!(
        server
            .downloader()
            .download(ID, &server.prefix(), &cancelled)
            .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn changed_pinned_representation_can_resume_from_a_compatible_client() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let receipt = seed_source_lease(&server, Some(WebClient::Samsung));
    let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
    let mut calls = Vec::new();
    let selected = recovery::select(
        &receipt,
        &[],
        |client| {
            calls.push(client);
            let mut source = parse_source(
                serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
                ID,
                &server.downloader().http,
            )?;
            source.web_client = client;
            if client == receipt.web_client {
                source.identity.last_modified = "new revision".into();
            }
            Ok(source)
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(calls, [Some(WebClient::Samsung), Some(WebClient::Visionos)]);
    assert_eq!(selected.identity, receipt.identity);
    assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn representation_change_waits_for_exhaustion_and_never_hides_a_terminal_failure() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let receipt = seed_source_lease(&server, Some(WebClient::Samsung));
    for terminal in [
        None,
        Some("youtube_rate_limited"),
        Some("youtube_source_unavailable"),
        Some("http_transport"),
    ] {
        let mut calls = 0;
        let error = recovery::select(
            &receipt,
            &[],
            |client| {
                calls += 1;
                if calls == 2
                    && let Some(code) = terminal
                {
                    return Err(crate::protocol::failure(
                        code,
                        None,
                        "terminal selection failure",
                    ));
                }
                let mut source = parse_source(
                    serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
                    ID,
                    &server.downloader().http,
                )?;
                source.web_client = client;
                source.identity.last_modified = "new revision".into();
                Ok(source)
            },
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(
            failure.code,
            terminal.unwrap_or("download_representation_changed")
        );
        assert_eq!(calls, if terminal.is_some() { 2 } else { 5 });
        assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
    }
}

#[test]
fn changed_pinned_client_proves_the_prefix_before_compatible_resume() {
    for corrupt in [false, true] {
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") {
                return refresh_page(&request, player(origin, 5));
            }
            if request.path.starts_with("/youtubei/") {
                return metadata(origin, 4);
            }
            seen.lock().unwrap().push(request.headers["range"].clone());
            range(&request, &[if corrupt { 8 } else { 9 }; 4])
        });
        let mut receipt = seed_source_lease(&server, Some(WebClient::Mweb));
        rewrite_source_lease(&server, &mut receipt, |value| {
            value["expires_at"] = 0.into()
        });
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        metrics::reset();
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        if corrupt {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "download_source_mismatch"
            );
            assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
            assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
            assert_eq!(*ranges.lock().unwrap(), ["bytes=0-1"]);
            assert_eq!(metrics::snapshot()["verified_replayed_audio_bytes"], 0);
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
            assert_eq!(*ranges.lock().unwrap(), ["bytes=0-1", "bytes=2-3"]);
            let after: Receipt =
                serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap())
                    .unwrap();
            assert_eq!(after.web_client, Some(WebClient::Samsung));
            assert_eq!(after.identity, receipt.identity);
            assert_eq!(metrics::snapshot()["verified_replayed_audio_bytes"], 2);
        }
    }
}

#[test]
fn cancelled_changed_representation_never_requests_another_client() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let receipt = seed_source_lease(&server, Some(WebClient::Samsung));
    let cancelled = AtomicBool::new(false);
    let mut calls = 0;
    let error = recovery::select(
        &receipt,
        &[],
        |client| {
            calls += 1;
            let mut source = parse_source(
                serde_json::to_vec(&player(&server.origin, 5)).unwrap(),
                ID,
                &server.downloader().http,
            )?;
            source.web_client = client;
            cancelled.store(true, Ordering::SeqCst);
            Ok(source)
        },
        &cancelled,
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("interrupted"));
    assert_eq!(calls, 1);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn refused_source_selection_skips_incompatible_clients_and_preserves_terminal_errors() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let mut receipt = seed_source_lease(&server, Some(WebClient::Mweb));
    let original = fs::read(suffix(&server.prefix(), ".json")).unwrap();
    let mut calls = Vec::new();
    let source = recovery::select(
        &receipt,
        &[Some(WebClient::Mweb)],
        |client| {
            calls.push(client);
            let mut source = parse_source(
                serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
                ID,
                &server.downloader().http,
            )?;
            source.web_client = client;
            if client == Some(WebClient::Samsung) {
                source.identity.last_modified = "another revision".into();
            }
            Ok(source)
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(calls, [Some(WebClient::Samsung), Some(WebClient::Visionos)]);
    assert_eq!(source.identity, receipt.identity);
    assert_eq!(source.web_client, Some(WebClient::Visionos));
    assert_eq!(
        fs::read(suffix(&server.prefix(), ".json")).unwrap(),
        original
    );
    for code in [
        "youtube_rate_limited",
        "youtube_source_unavailable",
        "http_transport",
    ] {
        calls.clear();
        let error = recovery::select(
            &receipt,
            &[Some(WebClient::Mweb)],
            |client| {
                calls.push(client);
                Err(crate::protocol::failure(
                    code,
                    (code != "http_transport").then_some(true),
                    "controlled failure",
                ))
            },
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            code
        );
        assert_eq!(calls.len(), 1);
    }
    calls.clear();
    let error = recovery::select(
        &receipt,
        &[Some(WebClient::Mweb)],
        |client| {
            calls.push(client);
            Err(crate::protocol::failure(
                "youtube_bot_check",
                Some(true),
                "controlled bot check",
            ))
        },
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "download_recovery_exhausted"
    );
    assert_eq!(calls.len(), 4);
    receipt.web_client = Some(WebClient::Samsung);
    let mut called = false;
    assert!(
        recovery::select(
            &receipt,
            &[],
            |_| {
                called = true;
                unreachable!()
            },
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!called);
}

#[test]
fn switched_source_must_reproduce_every_committed_prefix_byte() {
    for fault in ["none", "prefix", "etag", "cooldown", "truncated"] {
        let server = Server::new(move |request, _| {
            assert_eq!(request.headers["range"], "bytes=0-1");
            let mut reply = range(&request, &[if fault == "prefix" { 8 } else { 9 }, 9, 9, 9]);
            if fault == "etag" {
                reply.headers[1].1 = "\"changed\"".into();
            }
            if fault == "cooldown" {
                reply.status = 429;
                reply.headers.push(("Retry-After".into(), "120".into()));
            }
            if fault == "truncated" {
                reply.body.pop();
            }
            reply
        });
        let mut receipt = seed_source_lease(&server, None);
        receipt.etag = Some("\"original\"".into());
        let source = parse_source(
            serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
            ID,
            &server.downloader().http,
        )
        .unwrap();
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        let result =
            server
                .downloader()
                .verify_source_prefix(&source, &receipt, &AtomicBool::new(false));
        assert_eq!(result.is_ok(), fault == "none", "{fault}: {result:?}");
        assert_eq!(fs::read(suffix(&server.prefix(), ".json")).unwrap(), before);
        assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
        if fault == "cooldown" {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .retry_after_seconds(),
                Some(120)
            );
        }
    }
}

#[test]
fn repeated_media_refusal_switches_client_and_restart_uses_the_new_lease() {
    let players = Arc::new(AtomicUsize::new(0));
    let seen = players.clone();
    let allow_tail = Arc::new(AtomicBool::new(false));
    let ready = allow_tail.clone();
    let old_requests = Arc::new(AtomicUsize::new(0));
    let old = old_requests.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/watch") {
            return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"fresh-visitor"}}});"#.to_vec());
        }
        if request.path.starts_with("/youtubei/") {
            seen.fetch_add(1, Ordering::SeqCst);
            let mut value = player(origin, 4);
            let path = if request
                .headers
                .get("x-youtube-client-name")
                .map(String::as_str)
                == Some("7")
            {
                "new"
            } else {
                "old"
            };
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/{path}?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        if request.path.starts_with("/old?") {
            old.fetch_add(1, Ordering::SeqCst);
            return Reply {
                status: 403,
                headers: vec![],
                body: vec![],
            };
        }
        assert!(request.path.starts_with("/new?"));
        assert_eq!(request.headers["user-agent"], samsung::USER_AGENT);
        if request.headers["range"] == "bytes=2-3" && !ready.load(Ordering::SeqCst) {
            return Reply {
                status: 429,
                headers: vec![("Retry-After".into(), "120".into())],
                body: vec![],
            };
        }
        range(&request, &[9; 4])
    });
    seed_source_lease(&server, None);
    let error = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "youtube_rate_limited"
    );
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert_eq!(receipt.web_client, Some(WebClient::Samsung));
    assert!(receipt.lease.is_some());
    assert_eq!(receipt.progress.bytes, 2);
    assert_eq!(old_requests.load(Ordering::SeqCst), 3);
    assert_eq!(players.load(Ordering::SeqCst), 2);
    allow_tail.store(true, Ordering::SeqCst);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "download::tests::source_lease_process_fixture",
            "--nocapture",
        ])
        .env("AGENT_YOUTUBE_LEASE_CHILD_ORIGIN", &server.origin)
        .env("AGENT_YOUTUBE_LEASE_CHILD_PREFIX", server.prefix())
        .env("AGENT_YOUTUBE_LEASE_CHILD_FAIL", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9; 4]);
    assert_eq!(players.load(Ordering::SeqCst), 2);
}

#[test]
fn sabr_prefix_can_move_to_direct_only_after_complete_byte_proof() {
    let server = Server::new(|request, origin| {
        if request.path.starts_with("/watch") {
            return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"fresh-visitor"}}});"#.to_vec());
        }
        if request.path.starts_with("/youtubei/") {
            let mut value = player(origin, 4);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/new?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        assert_eq!(request.headers["range"], "bytes=0-1");
        range(&request, &[9; 4])
    });
    let mut receipt = seed_source_lease(&server, None);
    receipt.identity.direct = false;
    receipt.progress.init = true;
    receipt.progress.sequence = Some(8);
    receipt.progress.end_ms = 1000;
    let source = server
        .downloader()
        .replace_source(
            ID,
            &server.prefix(),
            &mut receipt,
            &[None],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(source.direct);
    let adopted: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert!(adopted.identity.direct);
    assert!(!adopted.progress.init);
    assert_eq!(adopted.progress.sequence, None);
    assert_eq!(adopted.progress.end_ms, 0);
    assert_eq!(adopted.progress.bytes, 2);
    assert_eq!(adopted.etag.as_deref(), Some("\"original\""));
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn refusal_budget_allows_all_qualified_direct_clients_without_cycling() {
    let refused = Arc::new(Mutex::new(Vec::new()));
    let seen = refused.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/watch") {
            return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"watch-visitor"}}}); var ytInitialPlayerResponse = {"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you're not a bot"}};"#.to_vec());
        }
        if request.path.starts_with("/embed/") {
            return Reply::bytes(br#"ytcfg.set({"VISITOR_DATA":"embed-visitor","INNERTUBE_CONTEXT":{"client":{}},"WEB_PLAYER_CONTEXT_CONFIGS":{"WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER":{"encryptedHostFlags":"host-flags"}}});"#.to_vec());
        }
        if request.path.starts_with("/youtubei/") {
            let mut value = player(origin, 4);
            let name = request
                .headers
                .get("x-youtube-client-name")
                .map_or("old", String::as_str);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/{name}?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        if request.headers["range"] == "bytes=0-1" || request.path.starts_with("/56?") {
            return range(&request, &[9; 4]);
        }
        seen.lock()
            .unwrap()
            .push(request.path.split('?').next().unwrap().to_owned());
        Reply {
            status: 403,
            headers: vec![],
            body: vec![],
        }
    });
    seed_source_lease(&server, None);
    let result = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false));
    assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
    assert_eq!(
        *refused.lock().unwrap(),
        ["/old", "/old", "/old", "/7", "/101"]
    );
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert_eq!(receipt.web_client, Some(WebClient::Embedded));
}

#[test]
fn invalid_lease_or_prefix_never_requests_or_modifies_media() {
    for fault in [
        "digest", "missing", "prefix", "video", "host", "track", "client", "script", "expiry",
        "path",
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = Server::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Reply {
                status: 403,
                headers: vec![],
                body: vec![],
            }
        });
        let mut receipt = seed_source_lease(&server, Some(WebClient::Mweb));
        match fault {
            "digest" => fs::write(
                suffix(
                    &server.prefix(),
                    &format!(".lease-{}", receipt.lease.as_ref().unwrap()),
                ),
                b"corrupted",
            )
            .unwrap(),
            "missing" => fs::remove_file(suffix(
                &server.prefix(),
                &format!(".lease-{}", receipt.lease.as_ref().unwrap()),
            ))
            .unwrap(),
            "prefix" => fs::write(suffix(&server.prefix(), ".m4a"), b"bad prefix").unwrap(),
            "path" => {
                receipt.lease = Some("../outside".into());
                fs::write(
                    suffix(&server.prefix(), ".json"),
                    serde_json::to_vec(&receipt).unwrap(),
                )
                .unwrap();
            }
            _ => rewrite_source_lease(&server, &mut receipt, |value| match fault {
                "video" => value["player"]["videoDetails"]["videoId"] = "wrongwrongx".into(),
                "host" => {
                    value["player"]["streamingData"]["adaptiveFormats"][0]["url"] =
                        "https://evil.invalid/audio?expire=4102444800".into()
                }
                "track" => {
                    value["player"]["streamingData"]["adaptiveFormats"][0]["lastModified"] =
                        "different".into()
                }
                "client" => value["client"]["clientName"] = "WEB".into(),
                "script" => value["player_script"] = "/s/player/../outside.js".into(),
                "expiry" => value["expires_at"] = u64::MAX.into(),
                _ => unreachable!(),
            }),
        }
        let media = fs::read(suffix(&server.prefix(), ".m4a")).unwrap();
        let journal = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        assert!(
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .is_err(),
            "{fault}"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0, "{fault}");
        assert_eq!(
            fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
            media,
            "{fault}"
        );
        assert_eq!(
            fs::read(suffix(&server.prefix(), ".json")).unwrap(),
            journal,
            "{fault}"
        );
    }
}

#[test]
fn leased_web_context_is_private_and_has_no_generated_token_or_session() {
    let server = Server::new(|_, _| Reply::bytes(vec![]));
    let mut value = player(&server.origin, 4);
    value["responseContext"] = serde_json::json!({"visitorData":"private-visitor-fixture"});
    value["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/audio?expire=4102444800&n=original", server.origin).into();
    let mut source = parse_source(
        serde_json::to_vec(&value).unwrap(),
        ID,
        &server.downloader().http,
    )
    .unwrap();
    source.web_client = Some(WebClient::Mweb);
    source.client = serde_json::json!({"clientName":"MWEB","clientVersion":"fixture","deviceModel":"iPad","visitorData":"private-visitor-fixture"});
    source.player_script = Some("/s/player/fixture/base.js".into());
    source.session.po_token = b"private-generated-token-fixture".to_vec();
    source.url = replace_query(&source.url, "pot", "private-generated-token-fixture").unwrap();
    let receipt = Receipt {
        version: 1,
        identity: source.identity.clone(),
        progress: Progress::default(),
        etag: None,
        web_client: source.web_client,
        lease: lease::save(&source, &server.prefix()).unwrap(),
        duration_seconds: None,
    };
    let path = suffix(
        &server.prefix(),
        &format!(".lease-{}", receipt.lease.as_ref().unwrap()),
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("private-generated-token-fixture")
            && !text.contains("private-visitor-fixture")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let loaded = lease::load(&server.prefix(), &receipt, ID, &server.downloader().http)
        .unwrap()
        .unwrap();
    assert!(loaded.session.po_token.is_empty() && loaded.token_expires.is_none());
    assert_eq!(loaded.client["deviceModel"], "iPad");
    assert_eq!(loaded.user_agent, MWEB_USER_AGENT);
    assert_eq!(loaded.player_script, source.player_script);
    assert_eq!(query_value(&loaded.url, "n"), Some("original"));
    assert!(query_value(&loaded.url, "pot").is_none());
}

#[cfg(unix)]
#[test]
fn linked_source_leases_are_rejected_without_network_or_media_changes() {
    for hard_link in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = Server::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Reply::bytes(vec![])
        });
        let receipt = seed_source_lease(&server, None);
        let path = suffix(
            &server.prefix(),
            &format!(".lease-{}", receipt.lease.unwrap()),
        );
        let other = server.root.join("unrelated");
        fs::rename(&path, &other).unwrap();
        if hard_link {
            fs::hard_link(&other, &path).unwrap();
        } else {
            std::os::unix::fs::symlink(&other, &path).unwrap();
        }
        let bytes = fs::read(&other).unwrap();
        assert!(
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(&other).unwrap(), bytes);
        assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
    }
}

#[test]
fn legacy_receipt_records_the_client_when_a_new_web_lease_is_attached() {
    let server = Server::new(|_, _| panic!("unexpected source or media request"));
    let mut receipt = seed_source_lease(&server, None);
    receipt.lease = None;
    fs::write(
        suffix(&server.prefix(), ".json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let downloader = server.downloader();
    let mut value = player(&server.origin, 4);
    value["streamingData"]["adaptiveFormats"][0]["url"] =
        format!("{}/audio?expire=4102444800", server.origin).into();
    let mut source =
        parse_source(serde_json::to_vec(&value).unwrap(), ID, &downloader.http).unwrap();
    source.web_client = Some(WebClient::Web);
    source.client = serde_json::json!({"clientName":"WEB","clientVersion":"fixture"});
    *downloader.bootstrap.lock().unwrap() = Some((Instant::now(), source));
    // The unit-test executable has no private CLI worker. Preparation stops there,
    // after durable lease attachment and before any source/media HTTP request.
    assert!(
        downloader
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .is_err()
    );
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert_eq!(receipt.web_client, Some(WebClient::Web));
    assert!(
        lease::load(&server.prefix(), &receipt, ID, &downloader.http)
            .unwrap()
            .is_some()
    );
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
}

#[test]
fn source_expiry_uses_protocol_deadlines_without_an_extra_lifetime_ceiling() {
    let before = lease::now().unwrap();
    let root = serde_json::json!({"streamingData":{"expiresInSeconds":"172800"}});
    let deadline = lease::deadline(&root, "https://r1.googlevideo.com/audio?expire=4102444800")
        .unwrap()
        .unwrap();
    assert!((before + 172800..=lease::now().unwrap() + 172800).contains(&deadline));
    assert_eq!(
        lease::deadline(&root, "https://r1.googlevideo.com/audio?expire=100").unwrap(),
        Some(100)
    );
    assert_eq!(
        lease::deadline(
            &serde_json::json!({}),
            "https://r1.googlevideo.com/audio?expire=4102444800"
        )
        .unwrap(),
        Some(4102444800)
    );
    assert!(
        lease::deadline(&root, "https://r1.googlevideo.com/audio")
            .unwrap()
            .is_none()
    );
}
#[test]
fn extraction_rejects_video_opus_auto_dubs_live_and_untrusted_urls() {
    let http = Http::new("https://www.youtube.com").unwrap();
    for case in ["video", "opus", "dub", "live", "host", "id", "restricted"] {
        let mut p = player("https://r1.googlevideo.com", 100);
        let f = &mut p["streamingData"]["adaptiveFormats"][0];
        match case {
            "video" => f["width"] = serde_json::json!(1920),
            "opus" => f["mimeType"] = serde_json::json!("audio/webm; codecs=\"opus\""),
            "dub" => f["audioTrack"] = serde_json::json!({"isAutoDubbed":true}),
            "host" => f["url"] = serde_json::json!("https://evil.invalid/audio"),
            "live" => p["videoDetails"]["isLive"] = serde_json::json!(true),
            "id" => p["videoDetails"]["videoId"] = serde_json::json!("wrongwrongxx"),
            _ => p["playabilityStatus"]["status"] = serde_json::json!("LOGIN_REQUIRED"),
        }
        assert!(
            parse_source(serde_json::to_vec(&p).unwrap(), ID, &http).is_err(),
            "{case}"
        );
    }
    let source = parse_source(
        serde_json::to_vec(&player("https://r1.googlevideo.com", 100)).unwrap(),
        ID,
        &http,
    )
    .unwrap();
    assert_eq!(source.identity.itag, 140);
}
#[test]
fn protobuf_and_ump_truncation_and_oversized_lengths_fail_closed() {
    for data in [
        &[0x80][..],
        &[0x0a, 0xff, 0xff, 0xff, 0xff, 0x7f],
        &[0],
        &[15],
    ] {
        assert!(wire::fields(data).is_err());
    }
    assert!(wire::part(&mut &[20, 2, 1][..]).is_err());
    assert!(wire::part(&mut &[20, 240, 255, 255, 255, 127][..]).is_err());
}

fn ump(kind: u8, body: &[u8], out: &mut Vec<u8>) {
    out.push(kind);
    let n = body.len() as u32;
    if n < 128 {
        out.push(n as u8);
    } else if n < 16384 {
        out.extend_from_slice(&[(128 | (n & 63)) as u8, (n >> 6) as u8]);
    } else {
        out.push(240);
        out.extend_from_slice(&n.to_le_bytes());
    }
    out.extend_from_slice(body);
}
fn sabr_player(origin: &str, length: usize) -> Reply {
    let mut p = player(origin, length);
    p["streamingData"]["adaptiveFormats"][0]
        .as_object_mut()
        .unwrap()
        .remove("url");
    p["streamingData"]["adaptiveFormats"].as_array_mut().unwrap().push(serde_json::json!({"itag":160,"lastModified":"12345","mimeType":"video/mp4; codecs=\"avc1\"","bitrate":100000}));
    p["streamingData"]["serverAbrStreamingUrl"] =
        serde_json::json!(format!("{origin}/sabr?test=1"));
    p["playerConfig"] = serde_json::json!({"mediaCommonConfig":{"mediaUstreamerRequestConfig":{"videoPlaybackUstreamerConfig":"AQ"}}});
    Reply::bytes(serde_json::to_vec(&p).unwrap())
}
fn sabr_segment(id: u8, init: bool, sequence: u64, offset: u64, payload: &[u8], out: &mut Vec<u8>) {
    let mut h = Vec::new();
    wire::num(&mut h, 1, id.into());
    wire::blob(&mut h, 2, ID.as_bytes());
    wire::num(&mut h, 3, 140);
    wire::num(&mut h, 4, 12345);
    wire::num(&mut h, 6, offset);
    wire::num(&mut h, 8, u64::from(init));
    wire::num(&mut h, 9, sequence);
    wire::num(&mut h, 11, sequence * 1000);
    wire::num(&mut h, 12, 1000);
    wire::num(&mut h, 14, payload.len() as u64);
    ump(20, &h, out);
    let mut data = vec![id];
    data.extend_from_slice(payload);
    ump(21, &data, out);
    ump(22, &[id], out);
}
fn sabr_meta(out: &mut Vec<u8>) {
    let mut m = Vec::new();
    wire::blob(&mut m, 1, ID.as_bytes());
    wire::blob(&mut m, 2, &format_id(&format(10)).unwrap());
    wire::num(&mut m, 3, 2000);
    wire::num(&mut m, 4, 1);
    wire::blob(&mut m, 5, b"audio/mp4; codecs=\"mp4a.40.2\"");
    ump(42, &m, out);
}
#[test]
fn sabr_long_backoff_returns_retry_guidance_without_shortening_server_wait() {
    for millis in [30_001, u64::MAX] {
        let server = Server::new(move |_, _| {
            let mut policy = Vec::new();
            wire::num(&mut policy, 4, millis);
            let mut out = Vec::new();
            ump(35, &policy, &mut out);
            Reply::bytes(out)
        });
        let downloader = server.downloader();
        let mut source =
            parse_source(sabr_player(&server.origin, 8).body, ID, &downloader.http).unwrap();
        let mut receipt = Receipt {
            version: 1,
            identity: source.identity.clone(),
            progress: Progress::default(),
            etag: None,
            web_client: None,
            lease: None,
            duration_seconds: None,
        };
        let path = suffix(&server.prefix(), ".m4a");
        let mut file = storage::private_file().read(true).open(&path).unwrap();
        let error = sabr::download_cycle(
            &downloader,
            &mut source,
            &mut file,
            &mut receipt,
            &suffix(&server.prefix(), ".json"),
            &mut Sha256::new(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "youtube_rate_limited");
        assert_eq!(failure.retry_after_seconds(), Some(millis.div_ceil(1000)));
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
        assert_eq!(receipt.progress.bytes, 0);
    }
}

#[test]
fn sabr_resumes_at_complete_segments_and_verifies_replayed_bytes() {
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            return sabr_player(o, 10);
        }
        let request = wire::fields(&r.body).unwrap();
        let abr = wire::fields(wire::bytes(&request, 1).unwrap()).unwrap();
        assert_eq!(wire::number(&abr, 40), Some(1));
        assert!(request.iter().any(|(k, _)| *k == 16));
        let n = seen.fetch_add(1, Ordering::SeqCst);
        let mut out = Vec::new();
        sabr_meta(&mut out);
        sabr_segment(0, true, 0, 0, b"INIT", &mut out);
        sabr_segment(1, false, 0, 4, b"ABC", &mut out);
        if n == 0 {
            ump(20, &[0x80], &mut out);
        }
        // Invalid trailing protobuf must preserve earlier segment receipts.
        else {
            sabr_segment(2, false, 1, 7, b"DEF", &mut out);
        }
        Reply::bytes(out)
    });
    assert!(
        server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(
        fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
        b"INITABC"
    );
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"INITABCDEF");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}
#[test]
fn access_faults_recover_or_preserve_incompatible_prefix() {
    for kind in ["403", "source-expiry", "track", "representation"] {
        faults::arm(kind, 7).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") || request.path.starts_with("/embed/") {
                return refresh_page(
                    &request,
                    serde_json::from_slice(&sabr_player(origin, 10).body).unwrap(),
                );
            }
            if request.path.starts_with("/youtubei/") {
                return sabr_player(origin, 10);
            }
            let n = observed.fetch_add(1, Ordering::SeqCst);
            let mut body = Vec::new();
            sabr_meta(&mut body);
            sabr_segment(0, true, 0, 0, b"INIT", &mut body);
            sabr_segment(1, false, 0, 4, b"ABC", &mut body);
            if n > 0 {
                sabr_segment(2, false, 1, 7, b"DEF", &mut body);
            }
            Reply::bytes(body)
        });
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        let evidence = faults::snapshot();
        assert_eq!(evidence["injected"], true);
        assert_eq!(evidence["prefix_bytes"], 7);
        if matches!(kind, "track" | "representation") {
            let error = result.unwrap_err();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(
                failure.code,
                if kind == "track" {
                    "download_audio_track_changed"
                } else {
                    "download_representation_changed"
                }
            );
            assert_eq!(failure.retryable, Some(false));
            assert_eq!(evidence["renewals"], 0);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
                b"INITABC"
            );
        } else {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(evidence["renewals"], 1);
            assert_eq!(
                fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
                b"INITABCDEF"
            );
        }
        faults::reset();
    }
}

#[test]
fn injected_stall_exercises_renewal_and_later_process_resume() {
    for persistent in [false, true] {
        metrics::reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                return sabr_player(origin, 10);
            }
            let n = observed.fetch_add(1, Ordering::SeqCst);
            let mut body = Vec::new();
            sabr_meta(&mut body);
            sabr_segment(0, true, 0, 0, b"INIT", &mut body);
            sabr_segment(1, false, 0, 4, b"ABC", &mut body);
            if n > 0 {
                sabr_segment(2, false, 1, 7, b"DEF", &mut body);
            }
            Reply::bytes(body)
        });
        metrics::arm_stall(7, persistent);
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        let evidence = metrics::snapshot()["stall"].clone();
        assert_eq!(evidence["renewals"], 1);
        assert_eq!(evidence["cycles"], if persistent { 8 } else { 4 });
        assert_eq!(evidence["prefix_bytes"], 7);
        assert_eq!(
            evidence["prefix_sha256"],
            format!("{:x}", Sha256::digest(b"INITABC"))
        );
        if persistent {
            let error = result.unwrap_err();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "download_no_progress");
            assert_eq!(failure.retryable, Some(true));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
                b"INITABC"
            );
            metrics::reset();
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .unwrap();
        } else {
            result.unwrap();
        }
        assert_eq!(
            fs::read(suffix(&server.prefix(), ".m4a")).unwrap(),
            b"INITABCDEF"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        metrics::reset();
    }
}

#[test]
fn stalled_sabr_renews_source_once_and_preserves_durable_prefix() {
    for case in ["recovered", "persistent", "changed-prefix", "cooldown"] {
        let players = Arc::new(AtomicUsize::new(0));
        let contexts = players.clone();
        let media = Arc::new(AtomicUsize::new(0));
        let requests = media.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                contexts.fetch_add(1, Ordering::SeqCst);
                let mut player: Value =
                    serde_json::from_slice(&sabr_player(origin, 10).body).unwrap();
                player["streamingData"]["serverAbrStreamingUrl"] =
                    format!("{origin}/sabr?expire={}", lease::now().unwrap() + 3600).into();
                return Reply::bytes(serde_json::to_vec(&player).unwrap());
            }
            let attempt = requests.fetch_add(1, Ordering::SeqCst);
            let fresh = contexts.load(Ordering::SeqCst) > 1;
            if fresh && case == "cooldown" {
                return Reply {
                    status: 429,
                    headers: vec![("Retry-After".into(), "120".into())],
                    body: vec![],
                };
            }
            let mut body = Vec::new();
            sabr_meta(&mut body);
            if attempt == 0 || (fresh && case != "persistent") {
                sabr_segment(0, true, 0, 0, b"INIT", &mut body);
                let prefix = if fresh && case == "changed-prefix" {
                    b"BAD"
                } else {
                    b"ABC"
                };
                sabr_segment(1, false, 0, 4, prefix, &mut body);
                if attempt == 0 {
                    // Persist a prefix, then force the next invocation to use its lease.
                    ump(20, &[0x80], &mut body);
                } else {
                    sabr_segment(2, false, 1, 7, b"DEF", &mut body);
                }
            }
            Reply::bytes(body)
        });
        let prefix = server.prefix();
        assert!(
            server
                .downloader()
                .download(ID, &prefix, &AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), b"INITABC");
        let result = server
            .downloader()
            .download(ID, &prefix, &AtomicBool::new(false));
        if case == "recovered" {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), b"INITABCDEF");
        } else {
            let error = result.unwrap_err();
            if case == "persistent" || case == "cooldown" {
                let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
                assert_eq!(
                    failure.code,
                    if case == "persistent" {
                        "download_no_progress"
                    } else {
                        "youtube_rate_limited"
                    }
                );
                if case == "cooldown" {
                    assert_eq!(failure.retry_after_seconds(), Some(120));
                }
            }
            assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), b"INITABC");
        }
        assert_eq!(players.load(Ordering::SeqCst), 2, "{case}");
        assert_eq!(
            media.load(Ordering::SeqCst),
            if case == "persistent" { 9 } else { 6 },
            "{case}"
        );
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(suffix(&prefix, ".json")).unwrap()).unwrap();
        assert_eq!(
            storage::digest(&suffix(&prefix, ".m4a")).unwrap(),
            receipt.progress.digest
        );
    }
}

#[test]
fn sabr_503_after_committed_segments_preserves_prefix_and_resumes_exactly() {
    for case in [
        "transient",
        "exhausted",
        "retry-after",
        "unknown-retry-after",
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let resume = Arc::new(AtomicBool::new(false));
        let ready = resume.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                return sabr_player(origin, 10);
            }
            let attempt = seen.fetch_add(1, Ordering::SeqCst);
            let complete = ready.load(Ordering::SeqCst) || (case == "transient" && attempt > 1);
            if attempt > 0 && !complete {
                let headers = match case {
                    "retry-after" => vec![("Retry-After".into(), "120".into())],
                    "unknown-retry-after" => vec![("Retry-After".into(), "unparsed".into())],
                    _ => vec![],
                };
                return Reply {
                    status: 503,
                    headers,
                    body: vec![],
                };
            }
            let mut body = Vec::new();
            sabr_meta(&mut body);
            sabr_segment(0, true, 0, 0, b"INIT", &mut body);
            sabr_segment(1, false, 0, 4, b"ABC", &mut body);
            if complete {
                sabr_segment(2, false, 1, 7, b"DEF", &mut body);
            }
            Reply::bytes(body)
        });
        let prefix = server.prefix();
        let result = server
            .downloader()
            .download(ID, &prefix, &AtomicBool::new(false));
        if case == "transient" {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(requests.load(Ordering::SeqCst), 3);
        } else {
            let error = result.unwrap_err();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            if case == "exhausted" {
                assert_eq!(failure.code, "download_http_temporary");
                assert_eq!(failure.http_status(), Some(503));
                assert_eq!(failure.details()["attempts"], 4);
                assert_eq!(requests.load(Ordering::SeqCst), 5);
            } else {
                assert_eq!(failure.code, "youtube_rate_limited");
                assert_eq!(requests.load(Ordering::SeqCst), 2);
                assert_eq!(
                    failure.retry_after(),
                    Some(if case == "retry-after" {
                        "120"
                    } else {
                        "unparsed"
                    })
                );
                assert_eq!(
                    failure.retry_after_seconds(),
                    if case == "retry-after" {
                        Some(120)
                    } else {
                        None
                    }
                );
            }
            let receipt: Receipt =
                serde_json::from_slice(&fs::read(suffix(&prefix, ".json")).unwrap()).unwrap();
            assert_eq!(receipt.progress.bytes, 7);
            assert_eq!(receipt.progress.sequence, Some(0));
            assert!(receipt.progress.init);
            assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), b"INITABC");
            assert_eq!(
                storage::digest(&suffix(&prefix, ".m4a")).unwrap(),
                receipt.progress.digest
            );
            // The loopback transport deliberately has no production shared gate.
            // Explicitly model a later permitted caller invocation after the pause
            // has been resolved; the failed invocation above sent no early retry.
            resume.store(true, Ordering::SeqCst);
            let before = requests.load(Ordering::SeqCst);
            server
                .downloader()
                .download(ID, &prefix, &AtomicBool::new(false))
                .unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), before + 1);
        }
        assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), b"INITABCDEF");
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(suffix(&prefix, ".json")).unwrap()).unwrap();
        assert_eq!(receipt.progress.bytes, 10);
        assert_eq!(
            storage::digest(&suffix(&prefix, ".m4a")).unwrap(),
            receipt.progress.digest
        );
    }
}

#[test]
fn sabr_gaps_wrong_formats_missing_end_and_short_segments_never_publish() {
    for fault in ["gap", "format", "short", "end", "sequence"] {
        let server = Server::new(move |r, o| {
            if r.path.starts_with("/youtubei/") {
                return sabr_player(o, 10);
            }
            let mut out = Vec::new();
            sabr_meta(&mut out);
            sabr_segment(0, true, 0, 0, b"INIT", &mut out);
            match fault {
                "gap" => sabr_segment(1, false, 0, 5, b"ABC", &mut out),
                "sequence" => {
                    sabr_segment(1, false, 0, 4, b"ABC", &mut out);
                    sabr_segment(2, false, 2, 7, b"DEF", &mut out);
                }
                "format" => {
                    let mut h = Vec::new();
                    wire::num(&mut h, 1, 1);
                    wire::num(&mut h, 3, 160);
                    wire::num(&mut h, 4, 12345);
                    ump(20, &h, &mut out);
                }
                "short" => {
                    sabr_segment(1, false, 0, 4, b"ABC", &mut out);
                    out.truncate(out.len() - 4);
                }
                _ => {
                    sabr_segment(1, false, 0, 4, b"ABC", &mut out);
                    out.truncate(out.len() - 3);
                }
            }
            Reply::bytes(out)
        });
        assert!(
            server
                .downloader()
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .is_err(),
            "{fault}"
        );
        assert!(
            fs::metadata(suffix(&server.prefix(), ".m4a"))
                .unwrap()
                .len()
                < 10,
            "{fault}"
        );
    }
}

#[test]
fn expired_link_refreshes_without_appending_from_a_changed_representation() {
    for changed in [false, true] {
        let players = Arc::new(AtomicUsize::new(0));
        let count = players.clone();
        let server = Server::new(move |r, o| {
            if r.path.starts_with("/watch") || r.path.starts_with("/embed/") {
                let mut p = player(o, 100);
                p["streamingData"]["adaptiveFormats"][0]["lastModified"] = "67890".into();
                return refresh_page(&r, p);
            }
            if r.path.starts_with("/youtubei/") {
                let n = count.fetch_add(1, Ordering::SeqCst);
                let mut p = player(o, 100);
                p["streamingData"]["adaptiveFormats"][0]["url"] =
                    serde_json::json!(format!("{o}/audio?generation={n}"));
                if changed && n > 0 {
                    p["streamingData"]["adaptiveFormats"][0]["lastModified"] =
                        serde_json::json!("67890");
                }
                return Reply::bytes(serde_json::to_vec(&p).unwrap());
            }
            if r.path.ends_with("generation=0") {
                return Reply {
                    status: 403,
                    headers: vec![],
                    body: vec![],
                };
            }
            range(&r, &[5; 100])
        });
        let result = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false));
        if changed {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("representation changed")
            );
            assert_eq!(
                fs::metadata(suffix(&server.prefix(), ".m4a"))
                    .unwrap()
                    .len(),
                0
            );
        } else {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [5; 100]);
        }
        assert_eq!(players.load(Ordering::SeqCst), if changed { 5 } else { 2 });
    }
}
#[test]
fn changed_etag_cannot_mix_http_representations() {
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            return metadata(o, RANGE_BYTES as usize + 23);
        }
        let mut reply = range(&r, &vec![7; RANGE_BYTES as usize + 23]);
        if r.headers["range"].starts_with(&format!("bytes={RANGE_BYTES}-")) {
            reply.headers[1].1 = "\"changed\"".into();
        }
        reply
    });
    assert!(
        server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .unwrap_err()
            .to_string()
            .contains("ETag changed")
    );
    assert_eq!(
        fs::metadata(suffix(&server.prefix(), ".m4a"))
            .unwrap()
            .len(),
        RANGE_BYTES
    );
}
#[test]
fn cancellation_during_http_transfer_preserves_partial_and_can_resume() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let server = Server::new(move |r, o| {
        if r.path.starts_with("/youtubei/") {
            return metadata(o, 100);
        }
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            signal.store(true, Ordering::SeqCst);
        }
        range(&r, &[3; 100])
    });
    assert!(
        server
            .downloader()
            .download(ID, &server.prefix(), &cancelled)
            .unwrap_err()
            .to_string()
            .contains("interrupted")
    );
    assert!(suffix(&server.prefix(), ".json").is_file());
    cancelled.store(false, Ordering::SeqCst);
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &cancelled)
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), [3; 100]);
}
#[test]
fn watch_bootstrap_skips_unrelated_config_calls_without_evaluating_javascript() {
    let text = r#"ytcfg.set('EMERGENCY_BASE_URL','/error'); ytcfg.set({"unrelated":1}); ytcfg.set({"INNERTUBE_CONTEXT":{"client":{"clientName":"WEB"}}}); window['ytInitialPlayerResponse']; var ytInitialPlayerResponse = {"playabilityStatus":{"status":"OK"}};"#;
    assert_eq!(
        embedded_json(text, "ytcfg.set(", "INNERTUBE_CONTEXT").unwrap()["INNERTUBE_CONTEXT"]["client"]
            ["clientName"],
        "WEB"
    );
    assert_eq!(
        embedded_json(text, "ytInitialPlayerResponse", "playabilityStatus").unwrap()["playabilityStatus"]
            ["status"],
        "OK"
    );
    assert!(
        embedded_json(
            "ytInitialPlayerResponse = JSON.parse('dynamic')",
            "ytInitialPlayerResponse",
            "playabilityStatus"
        )
        .is_err()
    );
}

#[test]
fn original_audio_wins_over_default_dub_and_descriptive_tracks() {
    let http = Http::new("https://www.youtube.com").unwrap();
    let mut p = player("https://r1.googlevideo.com", 4);
    let mut original = p["streamingData"]["adaptiveFormats"][0].clone();
    original["url"] = serde_json::json!("https://r1.googlevideo.com/original");
    original["audioTrack"] =
        serde_json::json!({"id":"ru.0","displayName":"Russian original","audioIsDefault":false});
    let mut dub = original.clone();
    dub["url"] = serde_json::json!("https://r1.googlevideo.com/dub");
    dub["audioTrack"] =
        serde_json::json!({"id":"en.0","displayName":"English","audioIsDefault":true});
    let mut descriptive = original.clone();
    descriptive["audioTrack"] = serde_json::json!({"id":"ru.0","displayName":"Russian (original) descriptive","audioIsDefault":true});
    descriptive["bitrate"] = serde_json::json!(256000);
    p["streamingData"]["adaptiveFormats"] = serde_json::json!([original, dub, descriptive]);
    let source = parse_source(serde_json::to_vec(&p).unwrap(), ID, &http).unwrap();
    assert!(source.url.ends_with("/original"));
}

#[test]
fn uncertain_labeled_original_audio_is_rejected() {
    let http = Http::new("https://www.youtube.com").unwrap();
    for case in ["dub", "missing_id", "two_originals", "auto_dub"] {
        let mut p = player("https://r1.googlevideo.com", 4);
        let f = &mut p["streamingData"]["adaptiveFormats"][0];
        f["audioTrack"] = serde_json::json!({"id":"ru.0","displayName":"Russian (original)"});
        match case {
            "dub" => f["audioTrack"]["displayName"] = serde_json::json!("English"),
            "missing_id" => {
                f["audioTrack"].as_object_mut().unwrap().remove("id");
            }
            "auto_dub" => f["audioTrack"]["isAutoDubbed"] = serde_json::json!(true),
            _ => {
                let mut other = f.clone();
                other["audioTrack"]["id"] = serde_json::json!("en.0");
                p["streamingData"]["adaptiveFormats"]
                    .as_array_mut()
                    .unwrap()
                    .push(other);
            }
        }
        assert!(
            parse_source(serde_json::to_vec(&p).unwrap(), ID, &http).is_err(),
            "{case}"
        );
    }
}

#[test]
fn independent_acquisitions_may_change_format_but_never_track_or_video() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let original = seed_source_lease(&server, None);
    let fresh = server.root.join("fresh");
    for change in ["format", "track", "video"] {
        let mut receipt: Receipt =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        match change {
            "format" => receipt.identity.itag = 141,
            "track" => receipt.identity.audio_track = Some("en.0".into()),
            "video" => receipt.identity.video_id = "2teKcIDANI4".into(),
            _ => unreachable!(),
        }
        fs::write(
            suffix(&fresh, ".json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let result = Downloader::same_track(&server.prefix(), &fresh);
        assert_eq!(result.is_ok(), change == "format");
        assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
    }
}

#[test]
fn resume_rejects_a_changed_audio_track_with_identical_representation_fields() {
    let calls = AtomicUsize::new(0);
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            let mut p = player(origin, 4);
            let id = if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                "ru.0"
            } else {
                "en.0"
            };
            p["streamingData"]["adaptiveFormats"][0]["audioTrack"] =
                serde_json::json!({"id":id,"displayName":"Language (original)"});
            Reply::bytes(serde_json::to_vec(&p).unwrap())
        } else {
            range(&request, b"data")
        }
    });
    let downloader = server.downloader();
    let cancelled = AtomicBool::new(false);
    let path = downloader
        .download(ID, &server.prefix(), &cancelled)
        .unwrap();
    let result = downloader.download(ID, &server.prefix(), &cancelled);
    let error = result.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "download_audio_track_changed"
    );
    assert_eq!(fs::read(path).unwrap(), b"data");
}

#[test]
fn sabr_requests_selected_original_track_id() {
    let observed = Arc::new(AtomicBool::new(false));
    let signal = observed.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            let mut reply = sabr_player(origin, 8);
            let mut p: Value = serde_json::from_slice(&reply.body).unwrap();
            p["streamingData"]["adaptiveFormats"][0]["audioTrack"] =
                serde_json::json!({"id":"ru.4","displayName":"Russian original"});
            reply.body = serde_json::to_vec(&p).unwrap();
            reply
        } else {
            let req = wire::fields(&request.body).unwrap();
            let abr = wire::fields(wire::bytes(&req, 1).unwrap()).unwrap();
            signal.store(
                wire::bytes(&abr, 69) == Some(b"ru.4".as_slice()),
                Ordering::SeqCst,
            );
            let mut body = Vec::new();
            sabr_meta(&mut body);
            sabr_segment(0, true, 0, 0, b"init", &mut body);
            sabr_segment(1, false, 1, 4, b"data", &mut body);
            Reply::bytes(body)
        }
    });
    let path = server
        .downloader()
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"initdata");
    assert!(
        observed.load(Ordering::SeqCst),
        "SABR omitted the selected track ID"
    );
}

#[test]
fn audio_metadata_above_64_gib_is_not_rejected_by_a_recording_size_policy() {
    let http = Http::new("https://www.youtube.com").unwrap();
    let mut p = player("https://r1.googlevideo.com", 4);
    p["streamingData"]["adaptiveFormats"][0]["contentLength"] =
        serde_json::json!((65_u64 << 30).to_string());
    assert!(parse_source(serde_json::to_vec(&p).unwrap(), ID, &http).is_ok());
}

#[test]
fn web_sabr_token_is_sent_in_the_streamer_context() {
    let http = Http::new("http://127.0.0.1:12345").unwrap();
    let mut source =
        parse_source(sabr_player("http://127.0.0.1:12345", 8).body, ID, &http).unwrap();
    let token = vec![7_u8; 96];
    source.session.po_token = token.clone();
    for (name, id) in [("WEB", 1), ("MWEB", 2)] {
        source.client = serde_json::json!({"clientName":name,"clientVersion":"fixture"});
        let body = sabr::request(&source, &Progress::default());
        let body = wire::fields(&body).unwrap();
        let context = wire::fields(wire::bytes(&body, 19).unwrap()).unwrap();
        assert_eq!(wire::bytes(&context, 2), Some(token.as_slice()));
        let client = wire::fields(wire::bytes(&context, 1).unwrap()).unwrap();
        assert_eq!(wire::number(&client, 16), Some(id));
    }
}

#[test]
fn player_paths_and_query_replacement_preserve_the_trust_boundary() {
    for page in [
        r#""jsUrl":"https://evil/base.js""#,
        r#""jsUrl":"/s/player/../../evil""#,
        r#""jsUrl":"/s/player/a/base.js?x=1""#,
    ] {
        assert!(player_script_path(page).is_err());
    }
    assert_eq!(
        player_script_path(r#""jsUrl":"/s/player/123/player_ias.vflset/en_US/base.js""#).unwrap(),
        "/s/player/123/player_ias.vflset/en_US/base.js"
    );
    assert_eq!(
        replace_query(
            "https://r.googlevideo.com/a?n=old&x=1&n=duplicate",
            "n",
            "new_1"
        )
        .unwrap(),
        "https://r.googlevideo.com/a?x=1&n=new_1"
    );
    assert!(replace_query("https://r.googlevideo.com/a", "n", "a&pot=x").is_err());
}

#[test]
fn resumed_sabr_declares_the_durable_audio_initialization() {
    let http = Http::new("http://127.0.0.1:12345").unwrap();
    let source = parse_source(sabr_player("http://127.0.0.1:12345", 8).body, ID, &http).unwrap();
    let progress = Progress {
        init: true,
        sequence: Some(7),
        bytes: 100,
        end_ms: 70_000,
        ..Progress::default()
    };
    let body = sabr::request(&source, &progress);
    assert!(
        wire::fields(&body)
            .unwrap()
            .iter()
            .any(|(key, value)| *key == 2
                && matches!(value,wire::Value::Bytes(format)if *format==source.format))
    );
}

#[test]
fn metadata_bootstrap_is_reused_for_download_instead_of_requesting_again() {
    let players = Arc::new(AtomicUsize::new(0));
    let observed = players.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::bytes(serde_json::to_vec(&player(origin, 4)).unwrap())
            } else {
                Reply::bytes(serde_json::to_vec(&serde_json::json!({"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you’re not a bot"}})).unwrap())
            }
        } else if request.path == "/audio" {
            range(&request, b"abcd")
        } else {
            Reply::bytes(b"unavailable".to_vec())
        }
    });
    let downloader = server.downloader();
    downloader.metadata(ID, &AtomicBool::new(false)).unwrap();
    let path = downloader
        .download(ID, &server.prefix(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"abcd");
    assert_eq!(players.load(Ordering::SeqCst), 1);
}

#[test]
fn bot_confirmation_is_retryable_but_permanent_restrictions_are_not() {
    let http = Http::new("https://www.youtube.com").unwrap();
    for (status, reason, code, retryable) in [
        (
            "LOGIN_REQUIRED",
            "Sign in to confirm you’re not a bot",
            "youtube_bot_check",
            true,
        ),
        (
            "LOGIN_REQUIRED",
            "Sign in to confirm your age",
            "youtube_source_unavailable",
            false,
        ),
        (
            "LOGIN_REQUIRED",
            "This is a private video",
            "youtube_source_unavailable",
            false,
        ),
        (
            "UNPLAYABLE",
            "Video unavailable",
            "youtube_source_unavailable",
            false,
        ),
        (
            "ERROR",
            "This content isn't available, try again later",
            "youtube_rate_limited",
            true,
        ),
    ] {
        let raw = serde_json::to_vec(
            &serde_json::json!({"playabilityStatus":{"status":status,"reason":reason}}),
        )
        .unwrap();
        let error = parse_source(raw, ID, &http).err().unwrap();
        let typed = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(typed.code, code);
        assert_eq!(typed.retryable, Some(retryable));
    }
}

#[test]
fn structured_captcha_is_retryable_without_misclassifying_generic_unavailability() {
    let http = Http::new("https://www.youtube.com").unwrap();
    for (captcha, expected) in [
        (serde_json::json!({}), "youtube_bot_check"),
        (Value::Null, "youtube_source_unavailable"),
        (
            serde_json::json!("unexpected"),
            "youtube_source_unavailable",
        ),
    ] {
        let raw = serde_json::to_vec(&serde_json::json!({"playabilityStatus":{"status":"UNPLAYABLE","reason":"Video unavailable","errorScreen":{"playerCaptchaViewModel":captcha}}})).unwrap();
        let error = parse_source(raw, ID, &http).err().unwrap();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            expected
        );
    }
}

#[test]
fn source_recovery_is_bounded_and_only_retries_transient_failures() {
    let cancelled = AtomicBool::new(false);
    let mut attempts = 0;
    let mut pauses = Vec::new();
    let result = recover_request(
        || {
            attempts += 1;
            if attempts < 3 {
                Err(crate::protocol::failure(
                    "youtube_bot_check",
                    Some(true),
                    "fixture bot check",
                ))
            } else {
                Ok("playable")
            }
        },
        |millis| {
            pauses.push(millis);
            Ok(())
        },
        &cancelled,
        true,
    )
    .unwrap();
    assert_eq!(result, "playable");
    assert_eq!(attempts, 3);
    assert_eq!(pauses, [5000, 10000]);
    for code in [
        "youtube_bot_check",
        "youtube_source_unavailable",
        "youtube_http_error",
        "invalid_format",
    ] {
        let mut attempts = 0;
        let mut pauses = Vec::new();
        let error = recover_request::<()>(
            || {
                attempts += 1;
                Err(crate::protocol::failure(
                    code,
                    Some(code == "youtube_bot_check"),
                    "fixture failure",
                ))
            },
            |millis| {
                pauses.push(millis);
                Ok(())
            },
            &cancelled,
            true,
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            code
        );
        assert_eq!(attempts, if code == "youtube_bot_check" { 4 } else { 1 });
        assert_eq!(pauses.len(), attempts - 1);
    }
}

#[test]
fn cancellation_during_source_backoff_prevents_another_request() {
    let cancelled = AtomicBool::new(false);
    let mut attempts = 0;
    let error = recover_request::<()>(
        || {
            attempts += 1;
            Err(crate::protocol::failure(
                "youtube_rate_limited",
                Some(true),
                "fixture rate limit",
            ))
        },
        |_| {
            cancelled.store(true, Ordering::SeqCst);
            Ok(())
        },
        &cancelled,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("interrupted"));
    assert_eq!(attempts, 1);
}

#[test]
fn source_route_refusal_can_reach_another_qualified_client() {
    for status in [403, 503] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let server = Server::new(move |request, origin| {
            seen.fetch_add(1, Ordering::SeqCst);
            if request.headers["user-agent"] == WEB_USER_AGENT {
                return Reply {
                    status,
                    headers: vec![],
                    body: vec![],
                };
            }
            assert_eq!(request.headers["user-agent"], MWEB_USER_AGENT);
            refresh_page(&request, player(origin, 4))
        });
        let source = server
            .downloader()
            .web_source(ID, None, &AtomicBool::new(false), None)
            .unwrap();
        assert_eq!(source.web_client, Some(WebClient::Mweb));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn retained_source_route_refusal_can_reach_a_compatible_client() {
    for (code, status, retryable) in [
        ("youtube_http_error", Some(403), false),
        ("youtube_temporary_failure", Some(503), true),
        ("http_transport", None, true),
    ] {
        let server = Server::new(|_, origin| metadata(origin, 4));
        let mut source = parse_source(
            serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
            ID,
            &server.downloader().http,
        )
        .unwrap();
        source.web_client = Some(WebClient::Web);
        let receipt = Receipt {
            version: 1,
            identity: source.identity.clone(),
            progress: Progress::default(),
            etag: None,
            web_client: Some(WebClient::Web),
            lease: None,
            duration_seconds: None,
        };
        let mut calls = 0;
        let selected = recovery::select(
            &receipt,
            &[],
            |client| {
                calls += 1;
                if client == Some(WebClient::Web) {
                    return Err(crate::protocol::failure_details(
                        code,
                        Some(retryable),
                        "fixture route rejection",
                        serde_json::json!({"http_status":status}),
                    ));
                }
                let mut alternative = source.clone();
                alternative.web_client = client;
                Ok(alternative)
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(selected.web_client, Some(WebClient::Samsung));
    }
}

#[test]
fn every_route_forbidden_stops_after_one_bounded_client_pass() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = Server::new(move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        Reply {
            status: 403,
            headers: vec![],
            body: vec![],
        }
    });
    let error = server
        .downloader()
        .web_source(ID, None, &AtomicBool::new(false), None)
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .http_status(),
        Some(403)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[test]
fn source_route_fallback_preserves_restrictions_cooldowns_and_invalid_metadata() {
    for fault in [
        "private",
        "deleted",
        "401",
        "404",
        "403 wait",
        "503 wait",
        "invalid",
        "wrong video",
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let server = Server::new(move |request, origin| {
            seen.fetch_add(1, Ordering::SeqCst);
            match fault {
                "private" | "deleted" => refresh_page(
                    &request,
                    serde_json::json!({"playabilityStatus":{"status":"UNPLAYABLE","reason":if fault=="private" {"Private video"} else {"This video has been removed"}}}),
                ),
                "invalid" => Reply::bytes(b"invalid watch metadata".to_vec()),
                "wrong video" => {
                    let mut metadata = player(origin, 4);
                    metadata["videoDetails"]["videoId"] = "wrongwrongxx".into();
                    refresh_page(&request, metadata)
                }
                _ => Reply {
                    status: match fault {
                        "401" => 401,
                        "404" => 404,
                        "403 wait" => 403,
                        _ => 503,
                    },
                    headers: if fault.ends_with("wait") {
                        vec![("Retry-After".into(), "120".into())]
                    } else {
                        vec![]
                    },
                    body: vec![],
                },
            }
        });
        let error = server
            .downloader()
            .web_source(ID, None, &AtomicBool::new(false), None)
            .err()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{fault}: {error}");
        if fault.ends_with("wait") {
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .retry_after_seconds(),
                Some(120)
            );
        }
    }
}

#[test]
fn all_failed_retained_routes_preserve_owned_media_and_receipt() {
    let server = Server::new(|_, origin| metadata(origin, 4));
    let receipt = seed_source_lease(&server, Some(WebClient::Web));
    let media = fs::read(suffix(&server.prefix(), ".m4a")).unwrap();
    let journal = fs::read(suffix(&server.prefix(), ".json")).unwrap();
    let mut calls = 0;
    let error = recovery::select(
        &receipt,
        &[],
        |_| {
            calls += 1;
            Err(crate::protocol::failure_details(
                "youtube_http_error",
                Some(false),
                "fixture denied route",
                serde_json::json!({"http_status":403}),
            ))
        },
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "download_recovery_exhausted"
    );
    assert_eq!(calls, 5);
    assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), media);
    assert_eq!(
        fs::read(suffix(&server.prefix(), ".json")).unwrap(),
        journal
    );
}

#[test]
fn transient_watch_status_is_typed_without_retrying_missing_or_forbidden_pages() {
    for status in [429, 503, 404, 403] {
        let server = Server::new(move |_, _| Reply {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        });
        let error = server
            .downloader()
            .web_source_once(ID, WebClient::Web, &AtomicBool::new(false), None)
            .err()
            .unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.retryable, Some(matches!(status, 429 | 503)));
    }
}

#[test]
fn source_recovery_respects_long_retry_after_without_early_requests() {
    let future =
        httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(120));
    for (header, expected) in [
        ("120", Some(120)),
        (future.as_str(), Some(120)),
        ("invalid HTTP date", None),
    ] {
        let cancelled = AtomicBool::new(false);
        let mut attempts = 0;
        let error = recover_request::<()>(
            || {
                attempts += 1;
                Err(http::rate_limited(Some(header), "fixture rate limit"))
            },
            |_| panic!("must not retry before the server cooldown"),
            &cancelled,
            true,
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        if expected.is_some() {
            assert!(matches!(failure.retry_after_seconds(), Some(119 | 120)));
        } else {
            assert_eq!(failure.retry_after_seconds(), None);
        }
        assert_eq!(failure.retry_after(), Some(header));
    }
}

#[test]
fn bootstrap_expiry_and_another_video_require_a_fresh_source() {
    for stale in [true, false] {
        let players = Arc::new(AtomicUsize::new(0));
        let observed = players.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                observed.fetch_add(1, Ordering::SeqCst);
                Reply::bytes(serde_json::to_vec(&player(origin, 4)).unwrap())
            } else {
                range(&request, b"abcd")
            }
        });
        let downloader = server.downloader();
        downloader.metadata(ID, &AtomicBool::new(false)).unwrap();
        if stale {
            downloader.bootstrap.lock().unwrap().as_mut().unwrap().0 =
                Instant::now() - Duration::from_secs(61);
            downloader
                .download(ID, &server.prefix(), &AtomicBool::new(false))
                .unwrap();
        } else {
            assert!(
                downloader
                    .download("of4B3aw7MXo", &server.prefix(), &AtomicBool::new(false))
                    .unwrap_err()
                    .to_string()
                    .contains("unexpected video id")
            );
        }
        assert_eq!(players.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn web_bootstrap_uses_mobile_after_bot_response_without_client_config() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let server = Server::new(move |request, origin| {
        assert!(request.path.starts_with("/watch?v="));
        let page = if observed.fetch_add(1, Ordering::SeqCst) == 0 {
            assert_eq!(request.headers["user-agent"], WEB_USER_AGENT);
            format!(
                "var ytInitialPlayerResponse={};",
                serde_json::json!({"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you’re not a bot"}})
            )
        } else {
            format!(
                "var ytInitialPlayerResponse={};ytcfg.set({});",
                player(origin, 4),
                serde_json::json!({"INNERTUBE_CONTEXT":{"client":{"clientName":if request.headers["user-agent"] == WEB_USER_AGENT { "WEB" } else { "MWEB" },"clientVersion":"fixture"}}})
            )
        };
        Reply::bytes(page.into_bytes())
    });
    let source = server
        .downloader()
        .web_source(ID, None, &AtomicBool::new(false), None)
        .unwrap();
    assert_eq!(source.identity.video_id, ID);
    assert_eq!(source.identity.length, 4);
    assert_eq!(source.web_client, Some(WebClient::Mweb));
    assert_eq!(source.client["clientName"], "MWEB");
    assert_eq!(source.user_agent, MWEB_USER_AGENT);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[test]
fn bot_denial_uses_the_matching_watch_context_for_one_player_request() {
    for (client, name, number, user_agent) in [
        (WebClient::Web, "WEB", "1", WEB_USER_AGENT),
        (WebClient::Mweb, "MWEB", "2", MWEB_USER_AGENT),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = Server::new(move |request, origin| {
            seen.fetch_add(1, Ordering::SeqCst);
            if request.path.starts_with("/watch") {
                let mut page = refresh_page(
                    &request,
                    serde_json::json!({"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you're not a bot"}}),
                );
                page.headers
                    .push(("Set-Cookie".into(), "YSC=anonymous; Secure; Path=/".into()));
                return page;
            }
            assert!(request.path.starts_with("/youtubei/v1/player"));
            assert_eq!(request.headers["user-agent"], user_agent);
            assert_eq!(request.headers["x-youtube-client-name"], number);
            assert_eq!(request.headers["x-youtube-client-version"], "fixture");
            assert_eq!(request.headers["x-goog-visitor-id"], "fixture-visitor");
            assert_eq!(request.headers["cookie"], "YSC=anonymous");
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["videoId"], ID);
            assert_eq!(
                body["context"],
                serde_json::json!({"client":{"clientName":name,"clientVersion":"fixture","visitorData":"fixture-visitor"}})
            );
            assert_eq!(
                body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"],
                1
            );
            metadata(origin, 4)
        });
        let source = server
            .downloader()
            .web_source_once(ID, client, &AtomicBool::new(false), None)
            .unwrap();
        assert_eq!(source.web_client, Some(client));
        assert_eq!(source.identity.video_id, ID);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn watch_player_fallback_rejects_invalid_context_and_retains_terminal_responses() {
    for (client, name, other_name) in [
        (WebClient::Web, "WEB", "MWEB"),
        (WebClient::Mweb, "MWEB", "WEB"),
    ] {
        for fault in [
            "private",
            "client",
            "version",
            "visitor",
            "timestamp",
            "429",
            "503 wait",
            "unknown wait",
            "wrong video",
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let seen = calls.clone();
            let server = Server::new(move |request, origin| {
                seen.fetch_add(1, Ordering::SeqCst);
                if request.path.starts_with("/watch") {
                    let player = serde_json::json!({"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":if fault=="private" {"This video is private"}else{"Sign in to confirm you're not a bot"}}});
                    let mut page = String::from_utf8(refresh_page(&request, player).body).unwrap();
                    page = match fault {
                        "client" => page.replace(name, other_name),
                        "version" => {
                            page.replace("\"clientVersion\":\"fixture\"", "\"clientVersion\":\"\"")
                        }
                        "visitor" => page.replace("fixture-visitor", ""),
                        "timestamp" => page.replace("\"STS\":1", "\"STS\":0"),
                        _ => page,
                    };
                    return Reply::bytes(page.into_bytes());
                }
                if matches!(fault, "429" | "503 wait" | "unknown wait") {
                    return Reply {
                        status: if fault == "429" { 429 } else { 503 },
                        headers: vec![(
                            "Retry-After".into(),
                            if fault == "unknown wait" {
                                "unparsed"
                            } else {
                                "120"
                            }
                            .into(),
                        )],
                        body: vec![],
                    };
                }
                let mut player = player(origin, 4);
                player["videoDetails"]["videoId"] = "wrongwrongxx".into();
                Reply::bytes(serde_json::to_vec(&player).unwrap())
            });
            let error = server
                .downloader()
                .web_source_once(ID, client, &AtomicBool::new(false), None)
                .err()
                .unwrap();
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if matches!(fault, "429" | "503 wait" | "unknown wait" | "wrong video") {
                    2
                } else {
                    1
                },
                "{fault}: {error}"
            );
            if matches!(fault, "429" | "503 wait" | "unknown wait") {
                assert_eq!(
                    error
                        .downcast_ref::<crate::protocol::Failure>()
                        .unwrap()
                        .retry_after_seconds(),
                    if fault == "unknown wait" {
                        None
                    } else {
                        Some(120)
                    }
                );
            }
        }
    }
}

#[test]
fn web_restrictions_and_long_cooldowns_do_not_switch_clients() {
    for status in [200, 403, 429] {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = Server::new(move |request, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.headers["user-agent"], WEB_USER_AGENT);
            Reply {
                status,
                headers: if status != 200 { vec![("Retry-After".into(), "120".into())] } else { vec![] },
                body: format!("var ytInitialPlayerResponse={};", serde_json::json!({"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"This video is private"}})).into_bytes(),
            }
        });
        assert!(
            server
                .downloader()
                .web_source(ID, None, &AtomicBool::new(false), None)
                .is_err()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn mobile_resume_pins_the_recorded_client_and_preserves_changed_representations() {
    for wrong_client in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = Server::new(move |request, origin| {
            observed.fetch_add(1, Ordering::SeqCst);
            if wrong_client {
                assert_eq!(request.headers["user-agent"], MWEB_USER_AGENT);
                return Reply::bytes(format!("var ytInitialPlayerResponse={};ytcfg.set({});", player(origin, 5), serde_json::json!({"INNERTUBE_CONTEXT":{"client":{"clientName":"WEB","clientVersion":"fixture"}}})).into_bytes());
            }
            if request.path.starts_with("/youtubei/") {
                metadata(origin, 5)
            } else {
                refresh_page(&request, player(origin, 5))
            }
        });
        let downloader = server.downloader();
        let mut desktop = parse_source(
            serde_json::to_vec(&player(&server.origin, 4)).unwrap(),
            ID,
            &downloader.http,
        )
        .unwrap();
        desktop.web_client = Some(WebClient::Web);
        let receipt = Receipt {
            version: 1,
            identity: desktop.identity.clone(),
            web_client: Some(WebClient::Mweb),
            lease: None,
            duration_seconds: None,
            etag: None,
            progress: Progress {
                bytes: 2,
                digest: format!("{:x}", Sha256::digest([1, 2])),
                ..Progress::default()
            },
        };
        *downloader.bootstrap.lock().unwrap() = Some((Instant::now(), desktop));
        let prefix = server.prefix();
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(suffix(&prefix, ".json"), &receipt_bytes).unwrap();
        fs::write(suffix(&prefix, ".m4a"), [1, 2]).unwrap();
        let error = downloader
            .download(ID, &prefix, &AtomicBool::new(false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if wrong_client {
                "unsupported client"
            } else {
                "representation changed"
            }),
            "{error}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            if wrong_client { 1 } else { 9 }
        );
        assert_eq!(fs::read(suffix(&prefix, ".json")).unwrap(), receipt_bytes);
        assert_eq!(fs::read(suffix(&prefix, ".m4a")).unwrap(), [1, 2]);
    }
}

#[test]
fn media_rate_limit_preserves_progress_without_early_retries() {
    for (sabr, status) in [false, true]
        .into_iter()
        .flat_map(|sabr| [429, 403, 408, 500, 502, 503, 504].map(|status| (sabr, status)))
    {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/youtubei/") {
                if sabr {
                    sabr_player(origin, 4)
                } else {
                    Reply::bytes(serde_json::to_vec(&player(origin, 4)).unwrap())
                }
            } else {
                observed.fetch_add(1, Ordering::SeqCst);
                Reply {
                    status,
                    headers: vec![("Retry-After".into(), "120".into())],
                    body: Vec::new(),
                }
            }
        });
        let error = server
            .downloader()
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .retry_after_seconds(),
            Some(120)
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
        assert_eq!(receipt.progress.bytes, 0);
        assert_eq!(
            fs::metadata(suffix(&server.prefix(), ".m4a"))
                .unwrap()
                .len(),
            0
        );
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/experiments/client-access/probe.rs"
));

#[test]
fn samsung_client_lease_preserves_context_and_does_not_require_web_tokens() {
    let client: WebClient = serde_json::from_str("\"samsung\"").unwrap();
    assert!(!client.requires_attestation());
    let server = Server::new(|_, origin| metadata(origin, 3));
    let mut response = player(&server.origin, 3);
    response["streamingData"]["adaptiveFormats"][0]["url"] = serde_json::json!(format!(
        "{}/audio?expire={}",
        server.origin,
        lease::now().unwrap() + 3600
    ));
    let mut source = parse_source(
        serde_json::to_vec(&response).unwrap(),
        ID,
        &server.downloader().http,
    )
    .unwrap();
    source.web_client = Some(client);
    source.client = serde_json::json!({"clientName":"TVHTML5","clientVersion":"5.20260707"});
    let mut receipt = Receipt {
        version: 1,
        identity: source.identity.clone(),
        progress: Progress::default(),
        etag: None,
        web_client: Some(client),
        lease: None,
        duration_seconds: None,
    };
    receipt.lease = lease::save(&source, &server.prefix()).unwrap();
    let restored = lease::load(&server.prefix(), &receipt, ID, &server.downloader().http)
        .unwrap()
        .unwrap();
    assert_eq!(restored.web_client, Some(client));
    assert_eq!(restored.user_agent, samsung::USER_AGENT);
    assert_eq!(restored.identity, source.identity);
}

#[test]
fn prepared_alternate_clients_bind_bootstrap_headers_and_private_leases() {
    for (name, wire_name, number) in [
        ("visionos", "VISIONOS", "101"),
        ("embedded", "WEB_EMBEDDED_PLAYER", "56"),
    ] {
        let selected: WebClient = serde_json::from_str(&format!("\"{name}\"")).unwrap();
        assert!(!selected.requires_attestation());
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") {
                return Reply {status:200, headers:vec![("Set-Cookie".into(), "VISITOR_INFO1_LIVE=watch-session; Path=/".into())], body:br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"watch-visitor"}}});"#.to_vec()};
            }
            if request.path.starts_with("/embed/") {
                return Reply {status:200, headers:vec![("Set-Cookie".into(), "VISITOR_INFO1_LIVE=embed-session; Path=/".into())], body:br#"ytcfg.set({"VISITOR_DATA":"embed-visitor","INNERTUBE_CONTEXT":{"client":{}},"WEB_PLAYER_CONTEXT_CONFIGS":{"WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER":{"encryptedHostFlags":"host-flags"}}});"#.to_vec()};
            }
            assert_eq!(request.headers["x-youtube-client-name"], number);
            assert_eq!(request.headers["origin"], origin);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["context"]["client"]["clientName"], wire_name);
            assert_eq!(
                body["context"]["client"]["clientVersion"],
                request.headers["x-youtube-client-version"]
            );
            assert_eq!(
                body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"],
                20746
            );
            if name == "embedded" {
                assert_eq!(request.headers["x-goog-visitor-id"], "embed-visitor");
                assert_eq!(
                    request.headers["cookie"],
                    "VISITOR_INFO1_LIVE=embed-session"
                );
                assert_eq!(
                    body["playbackContext"]["contentPlaybackContext"]["encryptedHostFlags"],
                    "host-flags"
                );
                assert!(body["context"]["thirdParty"]["embedUrl"].is_string());
            } else {
                assert_eq!(request.headers["x-goog-visitor-id"], "watch-visitor");
                assert_eq!(
                    request.headers["cookie"],
                    "VISITOR_INFO1_LIVE=watch-session"
                );
            }
            let mut value = player(origin, 4);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/audio?expire=4102444800").into();
            Reply::bytes(serde_json::to_vec(&value).unwrap())
        });
        let downloader = server.downloader();
        let mut source = downloader
            .web_source_once(ID, selected, &AtomicBool::new(false), None)
            .unwrap();
        assert!(source.direct);
        downloader
            .prepare_web(&mut source, ID, false, &AtomicBool::new(false))
            .unwrap();
        assert!(downloader.runtime.lock().unwrap().is_none());
        let receipt = Receipt {
            version: 1,
            identity: source.identity.clone(),
            progress: Progress::default(),
            etag: None,
            web_client: Some(selected),
            lease: lease::save(&source, &server.prefix()).unwrap(),
            duration_seconds: None,
        };
        let restored = lease::load(&server.prefix(), &receipt, ID, &downloader.http)
            .unwrap()
            .unwrap();
        assert_eq!(restored.web_client, Some(selected));
        assert_eq!(restored.user_agent, source.user_agent);
        assert_eq!(restored.client["clientName"], wire_name);
        let private = fs::read_to_string(suffix(
            &server.prefix(),
            &format!(".lease-{}", receipt.lease.unwrap()),
        ))
        .unwrap();
        for secret in [
            "watch-visitor",
            "embed-visitor",
            "host-flags",
            "watch-session",
            "embed-session",
        ] {
            assert!(!private.contains(secret));
        }
    }
}

#[cfg(unix)]
#[test]
fn attestation_failure_recovers_through_a_compatible_direct_client_but_cooldown_stops() {
    for (code, status, retry_after, seconds, recovers) in [
        ("youtube_attestation_failed", None, None, None, true),
        ("youtube_http_error", Some(403), None, None, true),
        ("youtube_http_error", Some(401), None, None, false),
        ("youtube_rate_limited", Some(429), None, Some(60), false),
        (
            "youtube_rate_limited",
            Some(403),
            Some("120"),
            Some(120),
            false,
        ),
        (
            "youtube_http_error",
            Some(403),
            Some("unparsed"),
            None,
            false,
        ),
        ("youtube_http_error", Some(403), None, Some(120), false),
        (
            "youtube_attestation_failed",
            None,
            Some("120"),
            Some(120),
            false,
        ),
    ] {
        let players = Arc::new(AtomicUsize::new(0));
        let seen = players.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") {
                return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"fresh-visitor"}}});"#.to_vec());
            }
            if request.path.starts_with("/youtubei/") {
                seen.fetch_add(1, Ordering::SeqCst);
                let mut value = player(origin, 4);
                value["streamingData"]["adaptiveFormats"][0]["url"] =
                    format!("{origin}/new?expire=4102444800").into();
                return Reply::bytes(serde_json::to_vec(&value).unwrap());
            }
            assert!(request.path.starts_with("/new?"));
            range(&request, &[9; 4])
        });
        seed_source_lease(&server, Some(WebClient::Mweb));
        let before = fs::read(suffix(&server.prefix(), ".json")).unwrap();
        let downloader = server.downloader();
        let failure = crate::protocol::failure_details(
            code,
            Some(code == "youtube_rate_limited"),
            "private-attestation-response",
            serde_json::json!({"http_status":status,"retry_after":retry_after,"retry_after_seconds":seconds}),
        );
        *downloader.runtime.lock().unwrap() =
            Some(crate::web::Worker::failure_fixture(failure.as_ref()));
        let result = downloader.download(ID, &server.prefix(), &AtomicBool::new(false));
        if recovers {
            assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
            assert_eq!(players.load(Ordering::SeqCst), 1);
        } else {
            let error = result.unwrap_err();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, code);
            assert_eq!(failure.http_status(), status);
            assert_eq!(failure.retry_after(), retry_after);
            assert_eq!(failure.retry_after_seconds(), seconds);
            assert!(!error.to_string().contains("private-attestation"));
            assert_eq!(players.load(Ordering::SeqCst), 0);
            // Preparation may renew the same receipt, but it cannot alter media.
            let after: Receipt =
                serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap())
                    .unwrap();
            let prior: Receipt = serde_json::from_slice(&before).unwrap();
            assert_eq!(after.progress.digest, prior.progress.digest);
            assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
        }
        assert!(downloader.runtime.lock().unwrap().is_none());
    }
}

#[test]
#[cfg(unix)]
fn attestation_http_refusal_preserves_media_when_alternate_identity_or_prefix_differs() {
    for changed_track in [false, true] {
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        let server = Server::new(move |request, origin| {
            if request.path.starts_with("/watch") {
                return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"fresh-visitor"}}});"#.to_vec());
            }
            if request.path.starts_with("/youtubei/") {
                let mut value = player(origin, 4);
                let format = &mut value["streamingData"]["adaptiveFormats"][0];
                format["url"] = format!("{origin}/new?expire=4102444800").into();
                if changed_track {
                    format["audioTrack"] =
                        serde_json::json!({"id":"en.0","displayName":"English (original)"});
                }
                return Reply::bytes(serde_json::to_vec(&value).unwrap());
            }
            assert!(request.path.starts_with("/new?"));
            seen.fetch_add(1, Ordering::SeqCst);
            range(&request, &[8; 4])
        });
        let prior = seed_source_lease(&server, Some(WebClient::Mweb));
        let downloader = server.downloader();
        let failure = crate::protocol::failure_details(
            "youtube_http_error",
            Some(false),
            "private-attestation-response",
            serde_json::json!({"http_status":403}),
        );
        *downloader.runtime.lock().unwrap() =
            Some(crate::web::Worker::failure_fixture(failure.as_ref()));
        let error = downloader
            .download(ID, &server.prefix(), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            if changed_track {
                "download_audio_track_changed"
            } else {
                "download_source_mismatch"
            }
        );
        assert_eq!(reads.load(Ordering::SeqCst), usize::from(!changed_track));
        let after: Receipt =
            serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
        assert_eq!(after.progress.digest, prior.progress.digest);
        assert_eq!(after.web_client, prior.web_client);
        assert_eq!(fs::read(suffix(&server.prefix(), ".m4a")).unwrap(), [9, 9]);
    }
}

#[test]
fn expired_pinned_source_can_resume_through_a_compatible_client() {
    let server = Server::new(|request, origin| {
        if request.path.starts_with("/watch") {
            return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"visitorData":"fresh-visitor"}}}); var ytInitialPlayerResponse = {"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you're not a bot"}};"#.to_vec());
        }
        if request.path.starts_with("/youtubei/") {
            let mut value = player(origin, 4);
            value["streamingData"]["adaptiveFormats"][0]["url"] =
                format!("{origin}/new?expire=4102444800").into();
            return Reply::bytes(serde_json::to_vec(&value).unwrap());
        }
        range(&request, &[9; 4])
    });
    let mut receipt = seed_source_lease(&server, Some(WebClient::Mweb));
    rewrite_source_lease(&server, &mut receipt, |value| {
        value["expires_at"] = 0.into()
    });
    let downloader = server.downloader();
    let cancelled = AtomicBool::new(false);
    let completed = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..100 {
                if completed.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            cancelled.store(true, Ordering::SeqCst);
        });
        let result = downloader.download(ID, &server.prefix(), &cancelled);
        completed.store(true, Ordering::SeqCst);
        result
    });
    assert_eq!(fs::read(result.unwrap()).unwrap(), [9; 4]);
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(suffix(&server.prefix(), ".json")).unwrap()).unwrap();
    assert_eq!(receipt.web_client, Some(WebClient::Samsung));
}

#[test]
fn samsung_source_uses_live_visitor_and_preserves_original_selection() {
    let server = Server::new(|request, origin| {
        if request.path.starts_with("/watch") {
            return Reply { status:200, headers:vec![("Set-Cookie".into(), "VISITOR_INFO1_LIVE=anonymous; Path=/; Secure".into())], body:br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"clientName":"WEB","visitorData":"live-visitor"}}});"#.to_vec() };
        }
        assert_eq!(request.headers["x-goog-visitor-id"], "live-visitor");
        assert_eq!(request.headers["user-agent"], samsung::USER_AGENT);
        assert!(request.headers["cookie"].contains("VISITOR_INFO1_LIVE=anonymous"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["context"]["client"]["clientName"], "TVHTML5");
        assert_eq!(
            body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"],
            20746
        );
        assert_eq!(
            body["playbackContext"]["contentPlaybackContext"]["html5Preference"],
            "HTML5_PREF_WANTS"
        );
        metadata(origin, 3)
    });
    let downloader = server.downloader();
    let source = downloader
        .samsung_source(ID, &AtomicBool::new(false), None)
        .unwrap();
    assert_eq!(source.web_client, Some(WebClient::Samsung));
    assert!(source.direct);
    assert_eq!(source.client["clientName"], "TVHTML5");
    downloader
        .prepare_web(&mut source.clone(), ID, false, &AtomicBool::new(false))
        .unwrap();
    assert!(downloader.runtime.lock().unwrap().is_none());
}

#[test]
fn source_fallback_tries_samsung_only_after_both_web_bot_checks() {
    let watches = Arc::new(AtomicUsize::new(0));
    let seen = watches.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/watch") {
            if seen.fetch_add(1, Ordering::SeqCst) < 2 {
                return Reply::bytes(br#"var ytInitialPlayerResponse = {"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you're not a bot"}};"#.to_vec());
            }
            return Reply::bytes(br#"ytcfg.set({"STS":20746,"INNERTUBE_CONTEXT":{"client":{"clientName":"WEB","visitorData":"live-visitor"}}});"#.to_vec());
        }
        assert_eq!(request.headers["x-youtube-client-name"], "7");
        metadata(origin, 3)
    });
    let source = server
        .downloader()
        .web_source(ID, None, &AtomicBool::new(false), None)
        .unwrap();
    assert_eq!(source.web_client, Some(WebClient::Samsung));
    assert_eq!(watches.load(Ordering::SeqCst), 3);
}

#[test]
fn source_fallback_does_not_probe_other_clients_for_restrictions_or_cooldown() {
    for cooldown in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let server = Server::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            if cooldown {
                Reply {
                    status: 429,
                    headers: vec![("Retry-After".into(), "60".into())],
                    body: vec![],
                }
            } else {
                Reply::bytes(br#"var ytInitialPlayerResponse = {"playabilityStatus":{"status":"UNPLAYABLE","reason":"Private video"}};"#.to_vec())
            }
        });
        let error = server
            .downloader()
            .web_source(ID, None, &AtomicBool::new(false), None)
            .err()
            .unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(
            failure.code,
            if cooldown {
                "youtube_rate_limited"
            } else {
                "youtube_source_unavailable"
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn anonymous_direct_clients_reject_unqualified_protocols_and_honor_player_cooldown() {
    for client in [WebClient::Samsung, WebClient::Visionos, WebClient::Embedded] {
        for case in ["sabr", "cooldown", "wrong-video", "dub"] {
            let server = Server::new(move |request, origin| {
                if request.path.starts_with("/watch") || request.path.starts_with("/embed/") {
                    return Reply::bytes(br#"ytcfg.set({"STS":20746,"VISITOR_DATA":"embed-visitor","INNERTUBE_CONTEXT":{"client":{"clientName":"WEB","visitorData":"live-visitor"}},"WEB_PLAYER_CONTEXT_CONFIGS":{"WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER":{"encryptedHostFlags":"host-flags"}}});"#.to_vec());
                }
                match case {
                    "sabr" => sabr_player(origin, 10),
                    "cooldown" => Reply {
                        status: 429,
                        headers: vec![("Retry-After".into(), "60".into())],
                        body: vec![],
                    },
                    _ => {
                        let mut value = player(origin, 3);
                        if case == "wrong-video" {
                            value["videoDetails"]["videoId"] = "wrong-video".into();
                        } else {
                            value["streamingData"]["adaptiveFormats"][0]["audioTrack"] = serde_json::json!({"id":"dub","displayName":"English dubbed","isAutoDubbed":true});
                        }
                        Reply::bytes(serde_json::to_vec(&value).unwrap())
                    }
                }
            });
            let error = server
                .downloader()
                .web_source_once(ID, client, &AtomicBool::new(false), None)
                .err()
                .unwrap();
            if case == "sabr" {
                assert_eq!(
                    error
                        .downcast_ref::<crate::protocol::Failure>()
                        .unwrap()
                        .code,
                    "youtube_audio_unsupported"
                );
            } else if case == "cooldown" {
                let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
                assert_eq!(failure.code, "youtube_rate_limited");
                assert_eq!(failure.retry_after_seconds(), Some(60));
            }
            assert!(!server.prefix().with_extension("m4a").exists());
        }
    }
}

#[test]
fn independent_jobs_keep_their_own_bootstrap_and_web_worker_lifetime() {
    let players = Arc::new(AtomicUsize::new(0));
    let observed = players.clone();
    let server = Server::new(move |request, origin| {
        if request.path.starts_with("/youtubei/") {
            observed.fetch_add(1, Ordering::SeqCst);
            Reply::bytes(serde_json::to_vec(&player(origin, 4)).unwrap())
        } else {
            range(&request, b"abcd")
        }
    });
    let downloader = server.downloader();
    let one = downloader.for_job();
    let two = downloader.for_job();
    assert!(!Arc::ptr_eq(&one.runtime, &two.runtime));
    let cancelled = AtomicBool::new(false);
    one.metadata(ID, &cancelled).unwrap();
    two.metadata(ID, &cancelled).unwrap();
    assert!(downloader.bootstrap.lock().unwrap().is_none());
    one.download(ID, &server.prefix(), &cancelled).unwrap();
    assert!(two.bootstrap.lock().unwrap().is_some());
    two.download(ID, &server.prefix().with_file_name("second"), &cancelled)
        .unwrap();
    assert_eq!(players.load(Ordering::SeqCst), 2);
}
