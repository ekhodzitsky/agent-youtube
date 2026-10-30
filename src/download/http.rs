use crate::Result;
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ureq::{
    Agent, Body,
    http::{Response, Uri},
    tls::TlsConfig,
};

#[derive(Clone)]
pub struct Http {
    agent: Agent,
    pub origin: String,
    local: bool,
    attestation: bool,
}
impl Http {
    pub fn new(origin: &str) -> Result<Self> {
        let uri: Uri = origin.parse()?;
        let local = uri.scheme_str() == Some("http") && uri.host() == Some("127.0.0.1");
        if origin != "https://www.youtube.com" && !local {
            return Err("expected YouTube HTTPS origin or an explicit loopback test server".into());
        }
        let tls = TlsConfig::builder()
            .unversioned_rustls_crypto_provider(Arc::new(super::tls::provider()?))
            .build();
        let agent = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            // This is the total body budget, not an idle-read timeout. Keep
            // progressing bounded responses viable on slower routes.
            .timeout_recv_body(Some(Duration::from_secs(30)))
            .timeout_per_call(Some(Duration::from_secs(45)))
            .build()
            .new_agent();
        Ok(Self {
            agent,
            origin: origin.trim_end_matches('/').into(),
            local,
            attestation: false,
        })
    }
    pub(crate) fn attestation() -> Result<Self> {
        let mut http = Self::new("https://www.youtube.com")?;
        http.attestation = true;
        Ok(http)
    }
    pub fn validate_url(&self, url: &str) -> Result<()> {
        let uri: Uri = url.parse()?;
        let host = uri.host().ok_or("download URL has no host")?;
        if uri.authority().is_none_or(|a| a.as_str().contains('@')) || url.contains('#') {
            return Err("download URL contains credentials or a fragment".into());
        }
        if self.attestation {
            if uri.scheme_str() == Some("https")
                && uri.port_u16().is_none_or(|p| p == 443)
                && ((host == "www.youtube.com"
                    && matches!(uri.path(), "/" | "/api/jnn/v1/GenerateIT"))
                    || (host == "www.google.com"
                        && (uri.path().starts_with("/js/bg/")
                            || uri.path().starts_with("/js/th/"))
                        && !uri.path().split('/').any(|part| matches!(part, "." | ".."))))
            {
                return Ok(());
            }
            return Err("refusing an untrusted attestation host or path".into());
        }
        if self.local {
            let origin: Uri = self.origin.parse()?;
            if uri.scheme_str() == Some("http") && uri.authority() == origin.authority() {
                return Ok(());
            }
        } else if uri.scheme_str() == Some("https")
            && uri.port_u16().is_none_or(|p| p == 443)
            && (host == "www.youtube.com"
                || (host == "m.youtube.com" && matches!(uri.path(), "/" | "/watch"))
                || host.ends_with(".googlevideo.com"))
        {
            return Ok(());
        }
        Err("refusing an untrusted media or redirect host".into())
    }
    pub fn request(
        &self,
        url: &str,
        body: Option<&[u8]>,
        headers: &[(&str, &str)],
        cancelled: &AtomicBool,
    ) -> Result<Response<Body>> {
        let gate = if self.local {
            None
        } else {
            Some(super::gate::Gate::user()?)
        };
        self.request_with_gate(url, body, headers, cancelled, gate.as_ref())
    }
    pub(crate) fn attestation_bytes(
        &self,
        url: &str,
        body: Option<&[u8]>,
        headers: &[(&str, &str)],
    ) -> Result<Vec<u8>> {
        // The isolated worker has its own hard request watchdog; parent
        // cancellation kills the worker, including an active HTTP request.
        let cancelled = AtomicBool::new(false);
        let response = self.request(url, body, headers, &cancelled)?;
        attestation_body(response, &cancelled)
    }
    fn request_with_gate(
        &self,
        url: &str,
        body: Option<&[u8]>,
        headers: &[(&str, &str)],
        cancelled: &AtomicBool,
        gate: Option<&super::gate::Gate>,
    ) -> Result<Response<Body>> {
        let mut url = url.to_owned();
        for hop in 0..6 {
            check(cancelled)?;
            self.validate_url(&url)?;
            if let Some(gate) = &gate {
                let uri: Uri = url.parse()?;
                gate.before(
                    matches!(uri.host(), Some("www.youtube.com" | "m.youtube.com"))
                        && (uri.path() == "/watch" || (self.attestation && uri.path() == "/")),
                    cancelled,
                )?;
            }
            #[cfg(test)]
            super::metrics::request(&url, self.attestation);
            let dispatch = || {
                if let Some(body) = body {
                    let mut req = self.agent.post(&url).header("Accept-Encoding", "identity");
                    for (key, value) in headers {
                        req = req.header(*key, *value);
                    }
                    req.send(body)
                } else {
                    let mut req = self.agent.get(&url).header("Accept-Encoding", "identity");
                    for (key, value) in headers {
                        req = req.header(*key, *value);
                    }
                    req.call()
                }
            };
            #[cfg(test)]
            let result = if super::metrics::inject_dns_failure(&url) {
                Err(ureq::Error::HostNotFound)
            } else {
                dispatch()
            };
            #[cfg(not(test))]
            let result = dispatch();
            let response = result.map_err(|error| {
                transport_failure(error, hop == 0 && self.agent.config().proxy().is_none())
            })?;
            let status = response.status().as_u16();
            if (status == 429 || (status >= 400 && response.headers().contains_key("retry-after")))
                && let Some(gate) = &gate
            {
                let header = response
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok());
                gate.defer(
                    retry_seconds(header, std::time::SystemTime::now()).unwrap_or(60),
                    cancelled,
                )?;
            }
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let next = response
                    .headers()
                    .get("location")
                    .ok_or("redirect has no Location")?
                    .to_str()?;
                let next = if next.starts_with('/') && !next.starts_with("//") {
                    let uri: Uri = url.parse()?;
                    format!(
                        "{}://{}{}",
                        uri.scheme_str().ok_or("missing URL scheme")?,
                        uri.authority().ok_or("missing URL authority")?,
                        next
                    )
                } else {
                    next.to_owned()
                };
                if rate_limit_redirect(&url, &next) {
                    return Err(challenge_redirect_failure(
                        status, &response, gate, cancelled,
                    )?);
                }
                if self.attestation {
                    return Err(crate::protocol::failure_details(
                        "youtube_http_error",
                        Some(false),
                        "attestation redirect refused",
                        serde_json::json!({"http_status":status}),
                    ));
                }
                if body.is_some() && !matches!(status, 307 | 308) {
                    return Err("unsafe POST redirect".into());
                }
                url = next;
                continue;
            }
            if response
                .headers()
                .get("content-encoding")
                .is_some_and(|v| v != "identity")
            {
                return Err("unexpected HTTP content encoding".into());
            }
            return Ok(response);
        }
        Err("too many HTTP redirects".into())
    }
}
fn transport_failure(
    error: ureq::Error,
    direct_initial: bool,
) -> Box<dyn std::error::Error + Send + Sync> {
    // Rustls handshake errors may arrive directly or inside a stream I/O error.
    // Match typed causes only; certificate diagnostics can contain private names.
    let tls = match &error {
        ureq::Error::Rustls(error) => Some(error),
        ureq::Error::Io(error) => error
            .get_ref()
            .and_then(|e| e.downcast_ref::<rustls::Error>()),
        _ => None,
    };
    if matches!(tls, Some(rustls::Error::FailedToGetRandomBytes)) {
        return crate::protocol::failure_details(
            "tls_random_unavailable",
            Some(true),
            "system randomness unavailable for TLS; media retained for resume",
            serde_json::json!({"transport_phase":"tls","stage":"download","preserved":true}),
        );
    }
    let permanent = match tls {
        Some(rustls::Error::InvalidCertificate(error)) => {
            use rustls::CertificateError;
            let reason = match error {
                CertificateError::UnsupportedSignatureAlgorithmContext { .. }
                | CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext { .. } => {
                    "unsupported_signature_algorithm"
                }
                CertificateError::UnknownIssuer => "unknown_issuer",
                CertificateError::NotValidForName
                | CertificateError::NotValidForNameContext { .. } => "hostname_mismatch",
                CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                    "certificate_expired"
                }
                CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                    "certificate_not_yet_valid"
                }
                _ => "certificate_invalid",
            };
            Some(("tls_certificate_invalid", reason))
        }
        Some(rustls::Error::PeerIncompatible(_)) => {
            Some(("tls_peer_incompatible", "protocol_or_cipher_mismatch"))
        }
        _ => None,
    };
    if let Some((code, reason)) = permanent {
        return crate::protocol::failure_details(
            code,
            Some(false),
            "TLS peer validation or compatibility failed; review TLS configuration; media retained",
            serde_json::json!({"transport_phase":"tls","stage":"download","reason":reason,"preserved":true}),
        );
    }
    // Connect includes TLS; generic I/O also mixes DNS, TLS and body failures.
    if direct_initial
        && matches!(
            error,
            ureq::Error::HostNotFound | ureq::Error::Timeout(ureq::Timeout::Resolve)
        )
    {
        crate::protocol::failure_details(
            "http_transport",
            Some(true),
            "HTTP transport failed; media retained for resume",
            serde_json::json!({"transport_phase":"dns"}),
        )
    } else {
        crate::protocol::failure(
            "http_transport",
            Some(true),
            "HTTP transport failed; media retained for resume",
        )
    }
}
fn challenge_redirect_failure(
    status: u16,
    response: &Response<Body>,
    gate: Option<&super::gate::Gate>,
    cancelled: &AtomicBool,
) -> Result<Box<dyn std::error::Error + Send + Sync>> {
    let header = response
        .headers()
        .get("retry-after")
        .map(|h| h.to_str().unwrap_or("invalid Retry-After"));
    let seconds = retry_seconds(header, std::time::SystemTime::now());
    if let Some(gate) = gate {
        gate.defer(seconds.unwrap_or(60), cancelled)?;
    }
    Ok(crate::protocol::failure_details(
        "youtube_rate_limited",
        Some(true),
        "YouTube redirected to a rate-limit challenge; retry later",
        serde_json::json!({"http_status":status,"retry_after":header,"retry_after_seconds":seconds}),
    ))
}
fn attestation_body(mut response: Response<Body>, cancelled: &AtomicBool) -> Result<Vec<u8>> {
    let status = response.status().as_u16();
    if status != 200 {
        let header = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok());
        let limited = status == 429 || (status >= 400 && header.is_some());
        let transient = matches!(status, 408 | 500 | 502 | 503 | 504);
        return Err(crate::protocol::failure_details(
            if limited {
                "youtube_rate_limited"
            } else if transient {
                "youtube_temporary_failure"
            } else {
                "youtube_http_error"
            },
            Some(limited || transient),
            format!("attestation server returned HTTP {status}"),
            serde_json::json!({"http_status":status,"retry_after":header,"retry_after_seconds":if limited { retry_seconds(header, std::time::SystemTime::now()) } else { None }}),
        ));
    }
    read(&mut response, 8 * 1024 * 1024, cancelled)
}
pub(super) fn media_failure(
    code: &'static str,
    status: Option<u16>,
    message: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        code,
        Some(true),
        message,
        serde_json::json!({"stage":"download","http_status":status,"preserved":true,"retry_after_seconds":1}),
    )
}
pub(super) fn retry_transport<T>(result: Result<T>, message: &'static str) -> Result<T> {
    result.map_err(|error| {
        if error
            .downcast_ref::<crate::protocol::Failure>()
            .is_some_and(|f| f.code == "http_transport")
        {
            media_failure("download_transport", None, message)
        } else {
            error
        }
    })
}
pub(super) fn rate_limited(
    retry_after: Option<&str>,
    message: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    let seconds = retry_seconds(retry_after, std::time::SystemTime::now());
    crate::protocol::failure_details(
        "youtube_rate_limited",
        Some(true),
        message,
        serde_json::json!({"retry_after_seconds":seconds,"retry_after":retry_after}),
    )
}
fn retry_seconds(retry_after: Option<&str>, now: std::time::SystemTime) -> Option<u64> {
    match retry_after {
        Some(value) => value
            .parse::<u64>()
            .ok()
            .map(|seconds| seconds.max(1))
            .or_else(|| {
                httpdate::parse_http_date(value).ok().map(|deadline| {
                    let wait = deadline.duration_since(now).unwrap_or_default();
                    wait.as_secs()
                        .saturating_add(u64::from(wait.subsec_nanos() != 0))
                        .max(1)
                })
            }),
        None => Some(60),
    }
}
fn rate_limit_redirect(from: &str, to: &str) -> bool {
    let (Ok(from), Ok(to)) = (from.parse::<Uri>(), to.parse::<Uri>()) else {
        return false;
    };
    from.scheme_str() == Some("https")
        && matches!(from.host(), Some("www.youtube.com" | "m.youtube.com"))
        && (matches!(
            from.path(),
            "/watch" | "/" | "/api/jnn/v1/GenerateIT" | "/youtubei/v1/player"
        ) || from.path().starts_with("/embed/"))
        && to.scheme_str() == Some("https")
        && to.host() == Some("www.google.com")
        && to.path() == "/sorry/index"
        && to.port_u16().is_none_or(|p| p == 443)
        && to.authority().is_some_and(|a| !a.as_str().contains('@'))
}
pub fn check(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("interrupted; run the same command to resume".into());
    }
    Ok(())
}
fn body_failure(
    error: std::io::Error,
    received: usize,
) -> Box<dyn std::error::Error + Send + Sync> {
    let typed = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<ureq::Error>());
    let reason = if matches!(typed, Some(ureq::Error::Timeout(_))) {
        "timeout"
    } else {
        match error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "timeout",
            std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
            std::io::ErrorKind::ConnectionReset => "connection_reset",
            std::io::ErrorKind::ConnectionAborted => "connection_aborted",
            std::io::ErrorKind::InvalidData => "invalid_data",
            std::io::ErrorKind::Interrupted => "interrupted",
            _ => "io_error",
        }
    };
    crate::protocol::failure_details(
        "http_transport",
        Some(true),
        "HTTP response body failed; media retained for resume",
        serde_json::json!({"transport_phase":"response_body","reason":reason,"body_bytes_received":received}),
    )
}

pub fn read(
    response: &mut Response<Body>,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>> {
    let mut reader = response.body_mut().as_reader();
    let mut result = Vec::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        check(cancelled)?;
        let n = reader
            .read(&mut buffer)
            .map_err(|error| body_failure(error, result.len()))?;
        if n == 0 {
            break;
        }
        if result.len().checked_add(n).is_none_or(|len| len > limit) {
            return Err("HTTP response exceeds its byte limit".into());
        }
        result.extend_from_slice(&buffer[..n]);
    }
    Ok(result)
}
pub fn pause(millis: u64, cancelled: &AtomicBool) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_millis(millis.min(30_000));
    while std::time::Instant::now() < deadline {
        check(cancelled)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    check(cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progressing_body_can_finish_after_the_old_five_second_budget() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
                .unwrap();
            for _ in 0..6 {
                std::thread::sleep(Duration::from_secs(1));
                if stream.write_all(b"x").is_err() {
                    break;
                }
            }
        });
        let http = Http::new(&origin).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut response = http.request(&origin, None, &[], &cancelled).unwrap();
        let result = read(&mut response, 1024, &cancelled);
        server.join().unwrap();
        assert_eq!(result.unwrap(), b"xxxxxx");
    }

    #[test]
    fn body_timeout_diagnostics_are_typed_and_do_not_copy_private_messages() {
        for error in [
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "https://private.example/?token=secret",
            ),
            ureq::Error::Timeout(ureq::Timeout::RecvBody).into_io(),
        ] {
            let error = body_failure(error, 7);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.details()["reason"], "timeout");
            assert_eq!(failure.details()["body_bytes_received"], 7);
            assert_eq!(failure.retryable, Some(true));
            let json = serde_json::to_string(failure.details()).unwrap();
            assert!(!json.contains("private") && !json.contains("secret"));
        }
    }

    #[test]
    fn body_disconnect_reports_phase_and_received_bytes_without_private_data() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\ndata",
                )
                .unwrap();
        });
        let http = Http::new(&origin).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut response = http.request(&origin, None, &[], &cancelled).unwrap();
        let error = read(&mut response, 1024, &cancelled).unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "http_transport");
        assert_eq!(failure.details()["transport_phase"], "response_body");
        assert_eq!(failure.details()["reason"], "unexpected_eof");
        assert_eq!(failure.details()["body_bytes_received"], 4);
        server.join().unwrap();
    }

    #[test]
    fn production_agent_client_hello_only_offers_qualified_groups() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = [0; 5];
            stream.read_exact(&mut header).unwrap();
            assert_eq!(header[0], 22);
            let length = u16::from_be_bytes([header[3], header[4]]) as usize;
            assert!(length < 16384);
            let mut hello = vec![0; length];
            stream.read_exact(&mut hello).unwrap();
            assert_eq!(hello[0], 1);
            let mut offset = 4 + 2 + 32;
            offset += 1 + hello[offset] as usize;
            let suites = u16::from_be_bytes([hello[offset], hello[offset + 1]]) as usize;
            offset += 2 + suites;
            offset += 1 + hello[offset] as usize;
            let extensions = u16::from_be_bytes([hello[offset], hello[offset + 1]]) as usize;
            offset += 2;
            let end = offset + extensions;
            while offset < end {
                let kind = u16::from_be_bytes([hello[offset], hello[offset + 1]]);
                let size = u16::from_be_bytes([hello[offset + 2], hello[offset + 3]]) as usize;
                offset += 4;
                if kind == 10 {
                    let groups = &hello[offset + 2..offset + size];
                    let (groups, remainder) = groups.as_chunks::<2>();
                    assert!(remainder.is_empty());
                    return groups
                        .iter()
                        .copied()
                        .map(u16::from_be_bytes)
                        .collect::<Vec<_>>();
                }
                offset += size;
            }
            panic!("ClientHello omitted supported_groups")
        });
        let http = Http::new(&format!("http://{address}")).unwrap();
        // Capture a real ClientHello; no server certificate or trust bypass is used.
        assert!(http.agent.get(format!("https://{address}")).call().is_err());
        // Deliberate 0.5 policy: X25519 first, then patched P-256/P-384.
        assert_eq!(task.join().unwrap(), vec![0x001d, 0x0017, 0x0018]);
    }

    #[test]
    fn shared_http_cooldown_process_fixture() {
        let Some(directory) = std::env::var_os("AGENT_YOUTUBE_HTTP_GATE_ROOT") else {
            return;
        };
        let origin = std::env::var("AGENT_YOUTUBE_HTTP_GATE_ORIGIN").unwrap();
        let gate = super::super::gate::Gate::at(directory.into()).unwrap();
        let http = Http::new(&origin).unwrap();
        let error = http
            .request_with_gate(&origin, None, &[], &AtomicBool::new(false), Some(&gate))
            .unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "youtube_rate_limited");
        assert!(failure.retry_after_seconds().unwrap() > 0);
    }

    #[test]
    fn attestation_status_is_structured_and_rate_limits_block_other_requests() {
        use std::{io::Write, net::TcpListener};
        for (status, header, code, seconds) in [
            (429, Some("120"), "youtube_rate_limited", Some(120)),
            (429, None, "youtube_rate_limited", Some(60)),
            (503, Some("unparsed"), "youtube_rate_limited", None),
            (408, Some("120"), "youtube_rate_limited", Some(120)),
            (500, Some("120"), "youtube_rate_limited", Some(120)),
            (502, Some("120"), "youtube_rate_limited", Some(120)),
            (504, Some("120"), "youtube_rate_limited", Some(120)),
            (403, Some("120"), "youtube_rate_limited", Some(120)),
            (502, None, "youtube_temporary_failure", None),
            (403, None, "youtube_http_error", None),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                let retry = header
                    .map(|h| format!("Retry-After: {h}\r\n"))
                    .unwrap_or_default();
                write!(stream, "HTTP/1.1 {status} Test\r\n{retry}Content-Length: 11\r\nConnection: close\r\n\r\nsecret-body").unwrap();
                listener
            });
            let directory = std::env::temp_dir().join(format!(
                "agent-youtube-attest-http-{}-{}",
                std::process::id(),
                status
            ));
            let gate = super::super::gate::Gate::at(directory.clone()).unwrap();
            let http = Http::new(&origin).unwrap();
            let cancelled = AtomicBool::new(false);
            let response = http
                .request_with_gate(&origin, None, &[], &cancelled, Some(&gate))
                .unwrap();
            let error = attestation_body(response, &cancelled).unwrap_err();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, code);
            assert_eq!(failure.http_status(), Some(status as u64));
            assert_eq!(failure.retry_after_seconds(), seconds);
            assert_eq!(failure.retry_after(), header);
            assert!(!error.to_string().contains("secret"));
            if code == "youtube_rate_limited" {
                assert!(
                    super::super::gate::Gate::at(directory.clone())
                        .unwrap()
                        .before(false, &cancelled)
                        .is_err()
                );
            }
            let listener = thread.join().unwrap();
            if status == 429 && header == Some("120") {
                let before = std::fs::read(directory.join("state.json")).unwrap();
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "download::http::tests::shared_http_cooldown_process_fixture",
                    ])
                    .env("AGENT_YOUTUBE_HTTP_GATE_ROOT", &directory)
                    .env("AGENT_YOUTUBE_HTTP_GATE_ORIGIN", &origin)
                    .output()
                    .unwrap();
                assert!(
                    child.status.success(),
                    "{}",
                    String::from_utf8_lossy(&child.stdout)
                );
                assert_eq!(std::fs::read(directory.join("state.json")).unwrap(), before);
            }
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn attestation_scope_does_not_expand_media_or_redirect_permissions() {
        let http = Http::attestation().unwrap();
        for url in [
            "https://www.youtube.com/",
            "https://www.youtube.com/api/jnn/v1/GenerateIT",
            "https://www.google.com/js/bg/fixture.js",
            "https://www.google.com/js/th/fixture.js",
        ] {
            assert!(http.validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "https://www.google.com/sorry/index",
            "https://www.google.com/js/bg/../evil",
            "https://www.google.com/js/bg/fixture#secret",
            "https://user@www.google.com/js/bg/fixture",
            "https://www.google.com:444/js/bg/fixture",
            "https://r1.googlevideo.com/media",
            "https://www.youtube.com/watch?v=fixture",
        ] {
            assert!(http.validate_url(url).is_err(), "{url}");
        }
        assert!(
            Http::new("https://www.youtube.com")
                .unwrap()
                .validate_url("https://www.google.com/js/bg/fixture.js")
                .is_err()
        );
    }

    #[test]
    fn media_retries_keep_structured_cooldown_guidance() {
        let result: Result<()> = Err(rate_limited(Some("120"), "fixture cooldown"));
        let error = retry_transport(result, "retryable: fixture transport").unwrap_err();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        assert_eq!(failure.code, "youtube_rate_limited");
        assert_eq!(failure.retry_after_seconds(), Some(120));
        let result: Result<()> = Err(crate::protocol::failure(
            "http_transport",
            Some(true),
            "fixture disconnect",
        ));
        assert_eq!(
            retry_transport(result, "retryable: fixture transport")
                .unwrap_err()
                .to_string(),
            "retryable: fixture transport"
        );
    }

    #[test]
    fn mobile_bootstrap_accepts_only_exact_https_metadata_paths() {
        let http = Http::new("https://www.youtube.com").unwrap();
        for url in [
            "https://m.youtube.com/",
            "https://m.youtube.com/watch?v=fixture",
        ] {
            assert!(http.validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://m.youtube.com/watch",
            "https://m.youtube.com:444/watch",
            "https://m.youtube.com.evil.invalid/watch",
            "https://user@m.youtube.com/watch",
            "https://m.youtube.com/watch#fragment",
            "https://m.youtube.com/media",
            "https://m.youtube.com/watch/other",
            "https://m.youtube.com/../watch",
        ] {
            assert!(http.validate_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn rate_limit_challenge_is_recognized_without_allowing_external_media() {
        let from = "https://www.youtube.com/watch?v=fixture";
        assert!(rate_limit_redirect(
            from,
            "https://www.google.com/sorry/index?continue=ignored"
        ));
        assert!(rate_limit_redirect(
            "https://m.youtube.com/watch?v=fixture",
            "https://www.google.com/sorry/index?continue=ignored"
        ));
        for path in [
            "/",
            "/api/jnn/v1/GenerateIT",
            "/youtubei/v1/player",
            "/embed/a2hbJlPxj4M",
        ] {
            assert!(rate_limit_redirect(
                &format!("https://www.youtube.com{path}"),
                "https://www.google.com/sorry/index"
            ));
        }
        for to in [
            "https://evil.invalid/sorry/index",
            "https://www.google.com/js/bg/x",
            "http://www.google.com/sorry/index",
            "https://user@www.google.com/sorry/index",
            "https://www.google.com:444/sorry/index",
        ] {
            assert!(!rate_limit_redirect(from, to));
        }
        assert!(!rate_limit_redirect(
            "https://r1.googlevideo.com/audio",
            "https://www.google.com/sorry/index"
        ));
        assert!(
            Http::new("https://www.youtube.com")
                .unwrap()
                .validate_url("https://www.google.com/sorry/index")
                .is_err()
        );
        for (header, expected) in [
            (None, Some(60)),
            (Some("12"), Some(12)),
            (Some("invalid HTTP date"), None),
            (Some("99999"), Some(99999)),
        ] {
            let error = rate_limited(header, "fixture quota");
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .retry_after_seconds(),
                expected
            );
        }
    }
    #[test]
    fn challenge_redirect_preserves_server_retry_after_and_shared_cooldown() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let date = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(180));
        for (index, header, minimum, maximum) in [
            (0, Some("120"), Some(120), Some(120)),
            (1, Some(date.as_str()), Some(175), Some(180)),
            (2, Some("unparsed"), None, None),
            (3, None, Some(60), Some(60)),
        ] {
            let directory = std::env::temp_dir().join(format!(
                "agent-youtube-challenge-gate-{}-{index}",
                std::process::id()
            ));
            let gate = super::super::gate::Gate::at(directory.clone()).unwrap();
            let mut response = Response::builder().status(302);
            if let Some(header) = header {
                response = response.header("retry-after", header);
            }
            let response = response.body(Body::builder().data(Vec::new())).unwrap();
            let cancelled = AtomicBool::new(false);
            let before = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let error =
                challenge_redirect_failure(302, &response, Some(&gate), &cancelled).unwrap();
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "youtube_rate_limited");
            assert_eq!(failure.http_status(), Some(302));
            assert_eq!(failure.retry_after(), header);
            match (minimum, maximum) {
                (Some(min), Some(max)) => {
                    assert!((min..=max).contains(&failure.retry_after_seconds().unwrap()))
                }
                _ => assert_eq!(failure.retry_after_seconds(), None),
            }
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(directory.join("state.json")).unwrap())
                    .unwrap();
            assert!(
                u128::from(state["cooldown_until_ms"].as_u64().unwrap())
                    >= before + u128::from(minimum.unwrap_or(60)) * 1000
            );
            assert!(
                super::super::gate::Gate::at(directory.clone())
                    .unwrap()
                    .before(false, &cancelled)
                    .is_err()
            );
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn tls_entropy_failures_are_sanitized_retryable_json() {
        for wrapped in [false, true] {
            let tls = rustls::Error::FailedToGetRandomBytes;
            let error = if wrapped {
                ureq::Error::Io(std::io::Error::other(tls))
            } else {
                ureq::Error::Rustls(tls)
            };
            let error = transport_failure(error, true);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "tls_random_unavailable");
            assert_eq!(failure.retryable, Some(true));
            assert!(!error.to_string().contains("OS Error"));
        }
    }

    #[test]
    fn tls_certificate_failures_are_sanitized_permanent_and_not_retried() {
        let tls = rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnsupportedSignatureAlgorithmContext {
                signature_algorithm_id: b"private-oid-context".to_vec(),
                supported_algorithms: Vec::new(),
            },
        );
        for wrapped in [false, true] {
            let make_error = || {
                if wrapped {
                    ureq::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        tls.clone(),
                    ))
                } else {
                    ureq::Error::Rustls(tls.clone())
                }
            };
            let error = transport_failure(make_error(), true);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "tls_certificate_invalid");
            assert_eq!(failure.retryable, Some(false));
            assert_eq!(
                failure.details()["reason"],
                "unsupported_signature_algorithm"
            );
            assert_eq!(failure.details()["preserved"], true);
            assert!(!format!("{failure:?}").contains("private-oid-context"));
            let result: Result<()> = retry_transport(Err(error), "generic transport message");
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "tls_certificate_invalid"
            );
            let mut calls = 0;
            let mut pauses = 0;
            let result: Result<()> = super::super::recover_request(
                || {
                    calls += 1;
                    Err(transport_failure(make_error(), true))
                },
                |_| {
                    pauses += 1;
                    Ok(())
                },
                &AtomicBool::new(false),
                false,
            );
            assert!(result.is_err());
            assert_eq!((calls, pauses), (1, 0));
        }
    }

    #[test]
    fn tls_compatibility_reasons_never_expose_peer_context() {
        let cases = [
            (
                rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered),
                "tls_peer_incompatible",
                "protocol_or_cipher_mismatch",
            ),
            (
                rustls::Error::InvalidCertificate(
                    rustls::CertificateError::NotValidForNameContext {
                        expected: rustls::pki_types::ServerName::try_from(
                            "private-expected.invalid",
                        )
                        .unwrap(),
                        presented: vec!["private-presented.invalid".into()],
                    },
                ),
                "tls_certificate_invalid",
                "hostname_mismatch",
            ),
            (
                rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
                "tls_certificate_invalid",
                "unknown_issuer",
            ),
        ];
        for (error, code, reason) in cases {
            let error = transport_failure(ureq::Error::Rustls(error), false);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, code);
            assert_eq!(failure.retryable, Some(false));
            assert_eq!(failure.details()["reason"], reason);
            assert!(!format!("{failure:?}").contains("private-"));
        }
    }

    #[test]
    fn only_initial_unproxied_dns_errors_are_marked_for_cdn_fallback() {
        for error in [
            ureq::Error::HostNotFound,
            ureq::Error::Timeout(ureq::Timeout::Resolve),
        ] {
            let error = transport_failure(error, true);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "http_transport");
            assert_eq!(failure.details()["transport_phase"], "dns");
        }
        for (error, initial_unproxied) in [
            (ureq::Error::HostNotFound, false),
            (ureq::Error::Timeout(ureq::Timeout::Resolve), false),
            (ureq::Error::Timeout(ureq::Timeout::Connect), true),
            (ureq::Error::Timeout(ureq::Timeout::RecvResponse), true),
            (
                ureq::Error::Io(std::io::Error::other("private-address")),
                true,
            ),
            (ureq::Error::Tls("private-tls-detail"), true),
            (ureq::Error::ConnectionFailed, true),
            (ureq::Error::StatusCode(503), true),
        ] {
            let error = transport_failure(error, initial_unproxied);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, "http_transport");
            assert!(failure.details()["transport_phase"].is_null());
            assert!(!error.to_string().contains("private"));
        }
    }

    #[test]
    fn nontext_challenge_retry_after_remains_unknown() {
        let response = Response::builder()
            .status(302)
            .header(
                "retry-after",
                ureq::http::HeaderValue::from_bytes(b"\xffprivate").unwrap(),
            )
            .body(Body::builder().data(Vec::new()))
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let error = challenge_redirect_failure(302, &response, None, &cancelled).unwrap();
        let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
        // This pair tells retry callers that a server interval was present but
        // cannot safely be interpreted, so the missing-header fallback is unsafe.
        assert!(failure.retry_after().is_some());
        assert_eq!(failure.retry_after_seconds(), None);
        assert!(!failure.details().to_string().contains("private"));
    }

    #[test]
    fn retry_after_dates_are_rounded_up_and_past_dates_do_not_underflow() {
        use std::time::{Duration, UNIX_EPOCH};
        let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_500);
        let date = httpdate::fmt_http_date(now + Duration::from_secs(100));
        assert_eq!(retry_seconds(Some(&date), now), Some(100));
        let date = httpdate::fmt_http_date(now - Duration::from_secs(100));
        assert_eq!(retry_seconds(Some(&date), now), Some(1));
        assert_eq!(
            retry_seconds(Some("18446744073709551615"), now),
            Some(u64::MAX)
        );
    }

    #[test]
    fn server_rate_limit_prevents_a_second_http_request_through_another_gate() {
        use std::{
            io::Write,
            net::TcpListener,
            time::{SystemTime, UNIX_EPOCH},
        };
        for status in [429, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(stream, "HTTP/1.1 {status} Limited\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                listener
            });
            let directory = std::env::temp_dir().join(format!(
                "agent-youtube-http-gate-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let gate = super::super::gate::Gate::at(directory.clone()).unwrap();
            let http = Http::new(&origin).unwrap();
            let cancelled = AtomicBool::new(false);
            let response = http
                .request_with_gate(&origin, None, &[], &cancelled, Some(&gate))
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
            let second = super::super::gate::Gate::at(directory.clone()).unwrap();
            let error = http
                .request_with_gate(&origin, None, &[], &cancelled, Some(&second))
                .err()
                .unwrap();
            assert_eq!(
                error
                    .downcast_ref::<crate::protocol::Failure>()
                    .unwrap()
                    .code,
                "youtube_rate_limited"
            );
            let listener = thread.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
