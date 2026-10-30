use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};
#[path = "support/contract.rs"]
mod contract;

const URL: &str = "https://www.youtube.com/watch?v=a2hbJlPxj4M";

#[test]
fn chapters_cli_fixture() {
    let Some(origin) = std::env::var_os("AGENT_YOUTUBE_CHAPTER_FIXTURE") else {
        return;
    };
    let downloader =
        agent_youtube::download::Downloader::loopback(origin.to_str().unwrap()).unwrap();
    if let Err(error) = agent_youtube::cli::run_with_downloader(
        vec!["chapters".into(), URL.into()],
        Arc::new(AtomicBool::new(false)),
        downloader,
    ) {
        agent_youtube::protocol::error(error.as_ref(), false);
        std::process::exit(1);
    }
}

fn invoke(status: u16, body: String) -> (Value, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let root = std::env::temp_dir().join(format!(
        "agent-youtube-chapters-{}-{}",
        std::process::id(),
        listener.local_addr().unwrap().port()
    ));
    fs::create_dir(&root).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("metadata request missing: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        assert!(request.starts_with("GET /watch?v=a2hbJlPxj4M&hl=en "));
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        let retry = if status == 429 {
            "Retry-After: 12\r\n"
        } else {
            ""
        };
        write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\n{retry}Connection: close\r\n\r\n{body}", body.len()).unwrap();
        // The listener closes after the single metadata response. No audio/API is served.
    });
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "chapters_cli_fixture", "--nocapture"])
        .env("AGENT_YOUTUBE_CHAPTER_FIXTURE", origin)
        .env("PATH", "/no-recognizer-or-external-tools")
        .env("GIGASTT_FORBIDDEN_FIXTURE_SETTING", "must-not-be-read")
        .current_dir(&root)
        .output()
        .unwrap();
    server.join().unwrap();
    let mut documents = Vec::new();
    contract::collect(&output, &mut documents);
    contract::validate(&documents);
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        0,
        "metadata-only command created job artifacts"
    );
    fs::remove_dir(root).unwrap();
    (documents.remove(0), output.status.success())
}

fn page(description: &str) -> String {
    format!(
        "<script>var ytInitialPlayerResponse={};</script>",
        json!({
        "playabilityStatus":{"status":"OK"},"videoDetails":{"videoId":"a2hbJlPxj4M",
        "title":"Author recording","lengthSeconds":"180","shortDescription":description}})
    )
}

#[test]
fn metadata_only_chapters_success_absence_and_failures_match_schema() {
    let (available, ok) = invoke(200, page("0:00 Intro\n1:00 Main\n2:00 End"));
    assert!(ok);
    assert_eq!(available["command"], "chapters");
    assert_eq!(available["result"]["status"], "available");
    assert_eq!(available["result"]["chapters"].as_array().unwrap().len(), 3);
    let (absent, ok) = invoke(200, page("No published timestamps"));
    assert!(ok);
    assert_eq!(absent["result"]["status"], "not_found");
    assert_eq!(absent["result"]["chapters"], json!([]));
    for (status, body, code) in [
        (
            200,
            page("1:00 Main\n0:00 Intro"),
            "youtube_chapters_invalid",
        ),
        (
            200,
            "<html>consent, secret-response-body</html>".into(),
            "youtube_metadata_invalid",
        ),
        (403, "secret-response-body".into(), "youtube_http_error"),
        (429, "secret-response-body".into(), "youtube_rate_limited"),
    ] {
        let (failure, ok) = invoke(status, body);
        assert!(!ok);
        assert_eq!(failure["error"]["code"], code);
        assert!(!failure.to_string().contains("secret-response-body"));
        if status == 429 {
            assert_eq!(failure["error"]["retryable"], true);
            assert_eq!(failure["error"]["details"]["retry_after_seconds"], 12);
        }
    }
}
