// Included only by the private downloader test module. Never used by the CLI.
#[test]
#[ignore = "opt-in live YouTube research; requires a new owned output directory"]
fn research_live_anonymous_clients() -> crate::Result<()> {
    let directory = PathBuf::from(
        std::env::var_os("AGENT_YOUTUBE_CLIENT_PROBE_DIR")
            .ok_or("set AGENT_YOUTUBE_CLIENT_PROBE_DIR to a new output directory")?,
    );
    let resume = std::env::var("AGENT_YOUTUBE_CLIENT_RESUME").as_deref() == Ok("1");
    if !resume {
        fs::create_dir(&directory)?;
    }
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("invalid research directory".into());
    }
    let _lock = storage::Lock::acquire(&directory)?;
    let attempt = std::env::var("AGENT_YOUTUBE_CLIENT_RESUME_ATTEMPT")
        .unwrap_or_else(|_| "1".into())
        .parse::<u8>()?;
    if !(1..=4).contains(&attempt) {
        return Err("research resume attempt must be 1..=4".into());
    }
    let phase = if !resume {
        String::new()
    } else if attempt == 1 {
        ".resume".into()
    } else {
        format!(".resume-{attempt}")
    };
    let stop_after = std::env::var("AGENT_YOUTUBE_CLIENT_STOP_AFTER")
        .ok()
        .map(|s| s.parse::<u64>())
        .transpose()?;
    let id = std::env::var("AGENT_YOUTUBE_CLIENT_VIDEO").unwrap_or_else(|_| "of4B3aw7MXo".into());
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("invalid research video id".into());
    }
    let selected = std::env::var("AGENT_YOUTUBE_CLIENT_NAME").ok();
    let configs: Vec<Value> = if let Some(path) = std::env::var_os("AGENT_YOUTUBE_CLIENT_CONFIG_FILE") {
        serde_json::from_str(&storage::read_text(Path::new(&path), METADATA_LIMIT as u64)?)?
    } else {
        serde_json::from_str(include_str!("clients.json"))?
    };
    let downloader = Downloader::youtube()?;
    let cancelled = AtomicBool::new(false);
    let mut results = Vec::new();
    for config in configs {
        let name = config["name"].as_str().ok_or("missing candidate name")?;
        if selected
            .as_deref()
            .is_some_and(|selected| !selected.split(',').any(|candidate| candidate == name))
        {
            continue;
        }
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("invalid candidate name".into());
        }
        if !results.is_empty() {
            http::pause(5000, &cancelled)?;
        }
        let started = Instant::now();
        let mut row = serde_json::json!({"name":name,"video_id":id,"client_version":config["body"]["context"]["client"]["clientVersion"],"verified_complete_audio":false});
        let outcome = (|| -> crate::Result<()> {
            let mut body = config["body"].clone();
            body["videoId"] = id.clone().into();
            let mut owned_headers: Vec<(String, String)> = config["headers"]
                .as_object()
                .ok_or("missing headers")?
                .iter()
                .map(|(k, v)| Ok((k.clone(), v.as_str().ok_or("invalid header")?.to_owned())))
                .collect::<crate::Result<_>>()?;
            let mut script_path = None;
            let mut bootstrap_player = None;
            if config["signature_bootstrap"] == true {
                let ua = WEB_USER_AGENT;
                let mut response = downloader.http.request(
                    &format!("https://www.youtube.com/watch?v={id}&hl=en"),
                    None,
                    &[("User-Agent", ua)],
                    &cancelled,
                )?;
                if response.status().as_u16() != 200 {
                    return Err("signature bootstrap unavailable".into());
                }
                let cookies = response.headers().get_all("set-cookie").iter()
                    .filter_map(|value| value.to_str().ok())
                    .filter_map(|value| value.split(';').next())
                    .collect::<Vec<_>>().join("; ");
                let page = String::from_utf8(http::read(&mut response, METADATA_LIMIT, &cancelled)?)?;
                let context = embedded_json(&page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
                let sts = context["STS"].as_u64().ok_or("missing signature timestamp")?;
                if !body["playbackContext"].is_object() {
                    body["playbackContext"] = serde_json::json!({"contentPlaybackContext":{}});
                }
                body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"] = sts.into();
                row["signature_timestamp"] = sts.into();
                script_path = player_script_path(&page).ok();
                if config["anonymous_session"] == true {
                    let visitor = context.pointer("/INNERTUBE_CONTEXT/client/visitorData")
                        .and_then(Value::as_str).ok_or("bootstrap has no visitor data")?;
                    owned_headers.push(("X-Goog-Visitor-Id".into(), visitor.into()));
                    owned_headers.push(("Origin".into(), "https://www.youtube.com".into()));
                    if !cookies.is_empty() {
                        owned_headers.push(("Cookie".into(), cookies));
                    }
                    row["anonymous_session"] = true.into();
                }
                if config["embedded_context"] == true {
                    let mut response = downloader.http.request(
                        &format!("https://www.youtube.com/embed/{id}?html5=1"), None,
                        &[("User-Agent", WEB_USER_AGENT), ("Referer", "https://www.reddit.com/")],
                        &cancelled,
                    )?;
                    if response.status().as_u16() != 200 {
                        return Err("embed bootstrap unavailable".into());
                    }
                    let page = String::from_utf8(http::read(&mut response, METADATA_LIMIT, &cancelled)?)?;
                    let context = embedded_json(&page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
                    let visitor = context["VISITOR_DATA"].as_str()
                        .or_else(|| context.pointer("/INNERTUBE_CONTEXT/client/visitorData").and_then(Value::as_str))
                        .ok_or("embed has no visitor data")?;
                    owned_headers.retain(|(key, _)| !key.eq_ignore_ascii_case("X-Goog-Visitor-Id"));
                    owned_headers.push(("X-Goog-Visitor-Id".into(), visitor.into()));
                    body["context"]["thirdParty"] = serde_json::json!({"embedUrl":"https://www.reddit.com/"});
                    let flags = context.pointer("/WEB_PLAYER_CONTEXT_CONFIGS/WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER/encryptedHostFlags")
                        .and_then(Value::as_str).ok_or("embed has no host flags")?;
                    body["playbackContext"]["contentPlaybackContext"]["encryptedHostFlags"] = flags.into();
                    row["embedded_context"] = true.into();
                }
            }
            if config["web_bootstrap"] == true {
                let ua = config["headers"]["User-Agent"]
                    .as_str()
                    .ok_or("missing UA")?;
                let reuse = std::env::var("AGENT_YOUTUBE_CLIENT_REUSE_SOURCE").as_deref() == Ok("1");
                let (page, cookies) = if reuse {
                    if !resume || config["player_route"] != "watch" {
                        return Err("cached bootstrap requires watch-source resume".into());
                    }
                    let cached = directory.join(format!("{name}.bootstrap.html"));
                    owned(&cached)?;
                    row["cached_bootstrap"] = true.into();
                    (
                        storage::read_text(&cached, METADATA_LIMIT as u64)?,
                        String::new(),
                    )
                } else {
                    let mut response = downloader.http.request(
                        &format!("https://www.youtube.com/watch?v={id}&hl=en"),
                        None,
                        &[("User-Agent", ua)],
                        &cancelled,
                    )?;
                    row["bootstrap_http_status"] = response.status().as_u16().into();
                    if response.status().as_u16() != 200 {
                        return Err("Web bootstrap unavailable".into());
                    }
                    let cookies = response
                        .headers()
                        .get_all("set-cookie")
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .filter_map(|v| v.split(';').next())
                        .collect::<Vec<_>>()
                        .join("; ");
                    (
                        String::from_utf8(http::read(&mut response, METADATA_LIMIT, &cancelled)?)?,
                        cookies,
                    )
                };
                storage::atomic_write(
                    &directory.join(format!("{name}.bootstrap{phase}.html")),
                    page.as_bytes(),
                    false,
                )?;
                if let Ok(player) =
                    embedded_json(&page, "ytInitialPlayerResponse", "playabilityStatus")
                {
                    row["bootstrap_playability_status"] =
                        player["playabilityStatus"]["status"].clone();
                    row["bootstrap_hls_present"] = player
                        .pointer("/streamingData/hlsManifestUrl")
                        .and_then(Value::as_str)
                        .is_some()
                        .into();
                    bootstrap_player = Some(player);
                }
                let bootstrap = embedded_json(&page, "ytcfg.set(", "INNERTUBE_CONTEXT")?;
                row["bootstrap_client_name"] =
                    bootstrap["INNERTUBE_CONTEXT"]["client"]["clientName"].clone();
                if bootstrap["INNERTUBE_CONTEXT"]["client"]["clientName"]
                    != body["context"]["client"]["clientName"]
                {
                    return Err(
                        "bootstrap returned a different client; refusing mixed context".into(),
                    );
                }
                body["context"] = bootstrap["INNERTUBE_CONTEXT"].clone();
                let sts = bootstrap["STS"]
                    .as_u64()
                    .ok_or("bootstrap has no signature timestamp")?;
                body["playbackContext"] = serde_json::json!({"contentPlaybackContext":{"html5Preference":"HTML5_PREF_WANTS","signatureTimestamp":sts}});
                row["signature_timestamp"] = sts.into();
                owned_headers.push(("Origin".into(), "https://www.youtube.com".into()));
                let visitor = bootstrap
                    .pointer("/INNERTUBE_CONTEXT/client/visitorData")
                    .and_then(Value::as_str)
                    .ok_or("bootstrap has no Visitor Data")?;
                body["context"]["client"]["visitorData"] = visitor.into();
                owned_headers.push(("X-Goog-Visitor-Id".into(), visitor.into()));
                if !cookies.is_empty() {
                    owned_headers.push(("Cookie".into(), cookies));
                    row["anonymous_bootstrap_cookies"] = true.into();
                }
                row["client_version"] = body["context"]["client"]["clientVersion"].clone();
                script_path = player_script_path(&page).ok();
            }
            if config["web_bootstrap"] == true {
                owned_headers.push((
                    "X-YouTube-Client-Version".into(),
                    body["context"]["client"]["clientVersion"]
                        .as_str()
                        .ok_or("missing version")?
                        .into(),
                ));
            }
            let mut headers: Vec<(&str, &str)> = owned_headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            headers.push(("Content-Type", "application/json"));
            let (status, raw) = if config["player_route"] == "watch" {
                row["player_route"] = "watch".into();
                (
                    200,
                    serde_json::to_vec(
                        bootstrap_player
                            .as_ref()
                            .ok_or("watch has no player response")?,
                    )?,
                )
            } else {
                row["player_route"] = "api".into();
                super::gate::Gate::user()?.before(true, &cancelled)?;
                let mut response = downloader.http.request(
                    "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
                    Some(&serde_json::to_vec(&body)?),
                    &headers,
                    &cancelled,
                )?;
                (
                    response.status().as_u16(),
                    http::read(&mut response, METADATA_LIMIT, &cancelled)?,
                )
            };
            row["http_status"] = status.into();
            storage::atomic_write(
                &directory.join(format!("{name}.source{phase}.json")),
                &raw,
                false,
            )?;
            if status != 200 {
                return Err(format!("player HTTP {status}").into());
            }
            let root: Value = serde_json::from_slice(&raw)?;
            let formats = root["streamingData"]["adaptiveFormats"].as_array();
            row["adaptive_formats"] = formats.map_or(0, Vec::len).into();
            row["ciphered_formats"] = formats.map_or(0, |formats| {
                formats.iter().filter(|format| format["signatureCipher"].is_string()).count()
            }).into();
            row["sabr_present"] = root["streamingData"]["serverAbrStreamingUrl"].is_string().into();
            row["hls_present"] = root["streamingData"]["hlsManifestUrl"].is_string().into();
            row["playability_status"] = root["playabilityStatus"]["status"].clone();
            row["captcha_present"] = root
                .pointer("/playabilityStatus/errorScreen/playerCaptchaViewModel")
                .is_some_and(Value::is_object)
                .into();
            row["bot_confirmation"] = root["playabilityStatus"]["reason"]
                .as_str()
                .is_some_and(|s| s.contains("not a bot"))
                .into();
            if config["hls_probe"] == true {
                let hls_root = if root.pointer("/streamingData/hlsManifestUrl").is_some() {
                    &root
                } else {
                    bootstrap_player.as_ref().unwrap_or(&root)
                };
                let url = hls_root
                    .pointer("/streamingData/hlsManifestUrl")
                    .and_then(Value::as_str);
                row["hls_manifest_present"] = url.is_some().into();
                if let Some(url) = url {
                    if hls_root["videoDetails"]["videoId"] != id
                        || hls_root["playabilityStatus"]["status"] != "OK"
                        || hls_root["videoDetails"]["isLive"] == true
                    {
                        return Err("HLS metadata identity/status/live validation failed".into());
                    }
                    let mut response = downloader.http.request(
                        url,
                        None,
                        &[(
                            "User-Agent",
                            config["headers"]["User-Agent"]
                                .as_str()
                                .ok_or("missing UA")?,
                        )],
                        &cancelled,
                    )?;
                    row["hls_http_status"] = response.status().as_u16().into();
                    let bytes = http::read(&mut response, METADATA_LIMIT, &cancelled)?;
                    storage::atomic_write(
                        &directory.join(format!("{name}.hls{phase}.m3u8")),
                        &bytes,
                        false,
                    )?;
                    return Err(
                        "HLS discovery only; no audio-only original VOD representation validated"
                            .into(),
                    );
                }
                return Err("Safari returned no HLS manifest; no media requested".into());
            }
            let mut source = parse_source(raw, &id, &downloader.http)?;
            if row["cached_bootstrap"] == true {
                let expires = query_value(&source.url, "expire")
                    .ok_or("cached source has no expiry")?
                    .parse::<u64>()?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();
                if expires <= now.saturating_add(60) {
                    return Err("cached source expired; prefix retained".into());
                }
            }
            row["original_aac_validated"] = true.into();
            row["declared_bytes"] = source.identity.length.into();
            row["direct"] = source.direct.into();
            if let Some(n) = query_value(&source.url, "n") {
                let path = script_path
                    .as_deref()
                    .ok_or("research route has no player script")?;
                let mut response = downloader.http.request(
                    &format!("https://www.youtube.com{path}"),
                    None,
                    &[(
                        "User-Agent",
                        config["headers"]["User-Agent"]
                            .as_str()
                            .ok_or("missing UA")?,
                    )],
                    &cancelled,
                )?;
                if response.status().as_u16() != 200 {
                    return Err("player script unavailable".into());
                }
                let player =
                    String::from_utf8(http::read(&mut response, METADATA_LIMIT, &cancelled)?)?;
                let solved = research_native_worker(
                    serde_json::json!({"operation":"N","player":player,"challenge":n}),
                )?;
                let solution = solved["solution"]
                    .as_str()
                    .filter(|v| {
                        !v.is_empty()
                            && v.len() <= 4096
                            && v.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    })
                    .ok_or("invalid n solution")?;
                source.url = replace_query(&source.url, "n", solution)?;
                row["n_solved"] = true.into();
            }
            if !source.direct
                && config["body"]["context"]["client"]["androidSdkVersion"].is_number()
            {
                return Err("research SABR encoder does not carry Android SDK context; no mismatched media request sent".into());
            }
            source.client = body["context"]["client"].clone();
            source.client["clientName"] = config["client_id"].clone();
            source.user_agent = config["headers"]["User-Agent"]
                .as_str()
                .ok_or("missing user agent")?
                .into();
            let native_token = if config["native_token"] == true
                && std::env::var_os("AGENT_YOUTUBE_CLIENT_TOKEN_FILE").is_none()
            {
                let response = research_native_worker(
                    serde_json::json!({"operation":"Mint","video":id,"refresh":true}),
                )?;
                let token: Vec<u8> = serde_json::from_value(response["token"].clone())?;
                if !(64..=4096).contains(&token.len()) {
                    return Err("invalid native token length".into());
                }
                row["native_token_bytes"] = token.len().into();
                row["native_token_expires_in"] = response["expires_in"].clone();
                Some(token)
            } else {
                None
            };
            if let Some(token) = native_token {
                if source.direct {
                    source.url = replace_query(
                        &source.url,
                        "pot",
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&token),
                    )?;
                }
                source.session.po_token = token;
            }
            if std::env::var("AGENT_YOUTUBE_CLIENT_RANGE_DIAGNOSTIC").as_deref() == Ok("1") {
                let span = "1048576-2097151";
                let query_url = replace_query(&source.url, "range", span)?;
                let mut observations = Vec::new();
                for (label, url, range) in [
                    (
                        "absolute-range-header",
                        source.url.as_str(),
                        Some("bytes=1048576-2097151"),
                    ),
                    ("range-query-only", query_url.as_str(), None),
                    (
                        "range-query-relative-header",
                        query_url.as_str(),
                        Some("bytes=0-1048575"),
                    ),
                ] {
                    let mut headers = vec![("User-Agent", source.user_agent.as_str())];
                    if let Some(range) = range {
                        headers.push(("Range", range));
                    }
                    let mut response = downloader.http.request(url, None, &headers, &cancelled)?;
                    let mut observation = serde_json::json!({"mode":label,"status":response.status().as_u16(),"content_range":response.headers().get("content-range").and_then(|v|v.to_str().ok()),"content_length":response.headers().get("content-length").and_then(|v|v.to_str().ok())});
                    let bytes = http::read(&mut response, 1048577, &cancelled)?;
                    observation["body_bytes"] = bytes.len().into();
                    storage::atomic_write(
                        &directory.join(format!("{label}.fragment")),
                        &bytes,
                        false,
                    )?;
                    observations.push(observation);
                }
                row["range_diagnostics"] = observations.into();
                return Ok(());
            }
            if let Some(path) = std::env::var_os("AGENT_YOUTUBE_CLIENT_TOKEN_FILE") {
                let token: Value =
                    serde_json::from_str(&storage::read_text(Path::new(&path), 65536)?)?;
                let bytes: Vec<u8> = serde_json::from_value(token["token"].clone())?;
                if !(64..=4096).contains(&bytes.len()) {
                    return Err("invalid research token length".into());
                }
                if source.direct {
                    source.url = replace_query(
                        &source.url,
                        "pot",
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
                    )?;
                }
                source.session.po_token = bytes;
                row["web_gvs_token_supplied"] = true.into();
            }
            let storage_name = std::env::var("AGENT_YOUTUBE_CLIENT_RESUME_FROM")
                .unwrap_or_else(|_| name.to_owned());
            if (!resume && storage_name != name)
                || !storage_name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("invalid research resume source".into());
            }
            let path = directory.join(format!("{storage_name}.m4a"));
            let journal = directory.join(format!("{storage_name}.receipt.json"));
            owned(&path)?;
            owned(&journal)?;
            let mut receipt = if resume {
                let receipt: Receipt = serde_json::from_str(&storage::read_text(&journal, 4096)?)?;
                if receipt.version != 1 || receipt.identity != source.identity {
                    return Err("candidate representation changed; prefix retained".into());
                }
                receipt
            } else {
                let receipt = Receipt {
                    version: 1,
                    identity: source.identity.clone(),
                    etag: None,
                    web_client: None,
                    lease: None,
                duration_seconds: None,
                    progress: Progress {
                        digest: format!("{:x}", Sha256::digest([])),
                        ..Progress::default()
                    },
                };
                storage::atomic_write(&journal, &serde_json::to_vec(&receipt)?, false)?;
                receipt
            };
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            if !resume {
                options.create_new(true);
            }
            let mut file = options.open(&path)?;
            let mut hasher = verify_partial(&mut file, &receipt.progress)?;
            row["resumed_from_bytes"] = receipt.progress.bytes.into();
            row["resumed_prefix_sha256"] = receipt.progress.digest.clone().into();
            file.set_len(receipt.progress.bytes)?;
            file.seek(SeekFrom::Start(receipt.progress.bytes))?;
            file.sync_all()?;
            let mut stalled = 0;
            let mut rounds = 0;
            while receipt.progress.bytes < receipt.identity.length {
                if started.elapsed().as_secs() >= 600 || rounds >= 512 {
                    return Err("bounded research transfer deadline; media retained".into());
                }
                rounds += 1;
                let before = receipt.progress.bytes;
                // No source refresh or client fallback here: keep candidate outcomes isolated.
                if source.direct {
                    if std::env::var("AGENT_YOUTUBE_CLIENT_RANGE_QUERY").as_deref() == Ok("1") {
                        let end = receipt
                            .progress
                            .bytes
                            .saturating_add(RANGE_BYTES - 1)
                            .min(receipt.identity.length - 1);
                        source.url = replace_query(
                            &source.url,
                            "range",
                            &format!("{}-{end}", receipt.progress.bytes),
                        )?;
                        row["range_query_supplied"] = true.into();
                    }
                    downloader.range(
                        &source,
                        &mut file,
                        &mut receipt,
                        &journal,
                        &mut hasher,
                        &cancelled,
                    )?;
                } else {
                    sabr::download_cycle(
                        &downloader,
                        &mut source,
                        &mut file,
                        &mut receipt,
                        &journal,
                        &mut hasher,
                        &cancelled,
                    )?;
                }
                row["received_bytes"] = receipt.progress.bytes.into();
                if stop_after.is_some_and(|limit| receipt.progress.bytes >= limit) {
                    return Err(
                        "research interruption requested after committed prefix; media retained"
                            .into(),
                    );
                }
                if before == receipt.progress.bytes {
                    stalled += 1;
                } else {
                    stalled = 0;
                }
                if stalled >= 4 {
                    return Err(
                        "candidate made no progress without attestation; media retained".into(),
                    );
                }
            }
            file.sync_all()?;
            let digest = storage::digest(&path)?;
            if receipt.progress.bytes != receipt.identity.length
                || digest != receipt.progress.digest
            {
                return Err("candidate failed complete-media integrity validation".into());
            }
            row["verified_complete_transfer"] = true.into();
            let wav = directory.join(format!("{name}{phase}.wav"));
            crate::audio::decode(&path, &wav, &cancelled)?;
            let pcm = crate::audio::Wav::open(&wav)?;
            let seconds = pcm.frames as f64 / f64::from(crate::audio::SAMPLE_RATE);
            let expected = root["videoDetails"]["lengthSeconds"]
                .as_str()
                .ok_or("missing declared duration")?
                .parse::<f64>()?;
            if !expected.is_finite() || (seconds - expected).abs() > 2.0 {
                return Err("decoded duration does not match the declared recording".into());
            }
            row["decoded_seconds"] = seconds.into();
            row["verified_complete_audio"] = true.into();
            row["sha256"] = digest.into();
            Ok(())
        })();
        if let Err(error) = outcome {
            let code = error
                .downcast_ref::<crate::protocol::Failure>()
                .map_or("candidate_failed", |e| e.code);
            row["error_code"] = code.into();
            if let Some(failure) = error.downcast_ref::<crate::protocol::Failure>() {
                row["media_http_status"] = failure.http_status().into();
            }
            storage::atomic_write(
                &directory.join(format!("{name}.error{phase}.txt")),
                error.to_string().as_bytes(),
                false,
            )?;
        }
        row["seconds"] = started.elapsed().as_secs_f64().into();
        let stop = row["http_status"] == 429 || row["error_code"] == "youtube_rate_limited";
        println!("CLIENT_PROBE {}", serde_json::to_string(&row)?);
        results.push(row);
        storage::atomic_write(
            &directory.join(format!("results{phase}.json")),
            &serde_json::to_vec_pretty(
                &serde_json::json!({"schema_version":1,"results":results,"stopped_for_cooldown":stop}),
            )?,
            true,
        )?;
        if stop {
            break;
        }
    }
    if results.is_empty() {
        return Err("no matching research client".into());
    }
    Ok(())
}

// Unit-test executables cannot handle the CLI's private worker entry point.
// Use the explicitly built local CLI; keep the process and protocol bounded.
fn research_native_worker(request: Value) -> crate::Result<Value> {
    use std::process::{Command, Stdio};
    let binary = std::env::var_os("AGENT_YOUTUBE_CLIENT_BINARY")
        .ok_or("set AGENT_YOUTUBE_CLIENT_BINARY to the built agent-youtube executable")?;
    let mut input = serde_json::to_vec(&request)?;
    input.push(b'\n');
    if input.len() > 8 * 1024 * 1024 {
        return Err("research worker input too large".into());
    }
    let mut child = Command::new(binary)
        .arg("--internal-web-runtime")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let sent = child
        .stdin
        .take()
        .ok_or("missing worker input")?
        .write_all(&input);
    if let Err(error) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let deadline = Instant::now() + Duration::from_secs(100);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err("research worker exited unsuccessfully".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("research worker timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or("missing worker output")?
        .take(65537)
        .read_to_end(&mut output)?;
    if output.len() > 65536 {
        return Err("research worker output too large".into());
    }
    let response: Value = serde_json::from_slice(&output)?;
    if response["failed"] == true {
        return Err("native research attestation/challenge failed".into());
    }
    Ok(response)
}

#[test]
#[ignore = "opt-in full production source, lease, worker and download qualification"]
fn research_production_client() -> crate::Result<()> {
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_PROBE_DIR").ok_or("set output directory")?);
    fs::create_dir(&directory)?;
    let _lock = storage::Lock::acquire(&directory)?;
    let id = std::env::var("AGENT_YOUTUBE_CLIENT_VIDEO")?;
    let binary = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_BINARY").ok_or("set worker binary")?);
    let cancelled = AtomicBool::new(false);
    metrics::reset();
    if let Ok(kind) = std::env::var("AGENT_YOUTUBE_RESEARCH_ACCESS_FAULT") {
        faults::arm(&kind, std::env::var("AGENT_YOUTUBE_RESEARCH_FAULT_AFTER_BYTES")?.parse()?)?;
    }
    if let Ok(mode) = std::env::var("AGENT_YOUTUBE_RESEARCH_STALL") {
        if !matches!(mode.as_str(), "renew" | "persistent") {
            return Err("invalid research stall mode".into());
        }
        let after_bytes = std::env::var("AGENT_YOUTUBE_RESEARCH_STALL_AFTER_BYTES")?.parse::<u64>()?;
        if after_bytes == 0 { return Err("stall requires a nonempty retained prefix".into()); }
        metrics::arm_stall(after_bytes, mode == "persistent");
    }
    if std::env::var("AGENT_YOUTUBE_RESEARCH_INJECT_SABR_DNS").as_deref() == Ok("1") {
        let after_bytes = std::env::var("AGENT_YOUTUBE_RESEARCH_DNS_AFTER_BYTES")
            .ok().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(0);
        metrics::arm_dns_failure(after_bytes);
    }
    let downloader = Downloader::youtube()?;
    let started = Instant::now();
    let selected = std::env::var("AGENT_YOUTUBE_PRODUCTION_CLIENT").unwrap_or_else(|_| "samsung".into());
    let mut row = serde_json::json!({"schema_version":1,"video_id":id,"client":selected,"production_download":true,"verified_complete_audio":false});
    let outcome = (|| -> crate::Result<()> {
        let source = if selected == "auto" { downloader.source(&id, &cancelled, None)? } else {
            let client: WebClient = serde_json::from_value(selected.clone().into())?;
            downloader.web_source_once(&id, client, &cancelled, None)?
        };
        row["initial_client"] = serde_json::to_value(source.web_client)?;
        let root: Value = serde_json::from_slice(&source.raw)?;
        let expected = root["videoDetails"]["lengthSeconds"].as_str().ok_or("missing duration")?.parse::<f64>()?;
        row["category_evidence"] = serde_json::json!({"upload_date":root.pointer("/microformat/playerMicroformatRenderer/uploadDate").and_then(Value::as_str),
            "audio_track_ids":root["streamingData"]["adaptiveFormats"].as_array().map(|formats|formats.iter().filter_map(|f|f.pointer("/audioTrack/id").and_then(Value::as_str)).collect::<std::collections::BTreeSet<_>>())});
        row["declared_seconds"] = expected.into();
        row["declared_bytes"] = source.identity.length.into();
        *downloader.bootstrap.lock().map_err(|_| "bootstrap lock failed")? = Some((Instant::now(), source));
        *downloader.runtime.lock().map_err(|_| "worker lock failed")? = Some(crate::web::Worker::test_executable(&binary)?);
        let stop_after = std::env::var("AGENT_YOUTUBE_PRODUCTION_STOP_AFTER").ok().map(|value| value.parse::<u64>()).transpose()?;
        let completed = AtomicBool::new(false);
        let path = thread::scope(|scope| {
            if let Some(limit) = stop_after {
                let cancelled = &cancelled;
                let completed = &completed;
                let directory = &directory;
                scope.spawn(move || {
                    while !completed.load(Ordering::SeqCst) {
                        if let Ok(text) = storage::read_text(&directory.join("audio.json"), 65536)
                            && let Ok(receipt) = serde_json::from_str::<Receipt>(&text)
                            && receipt.progress.bytes >= limit
                        { cancelled.store(true, Ordering::SeqCst); break; }
                        thread::sleep(Duration::from_millis(10));
                    }
                });
            }
            let result = downloader.download(&id, &directory.join("audio"), &cancelled);
            completed.store(true, Ordering::SeqCst);
            result
        })?;
        row["acquisition_seconds"] = started.elapsed().as_secs_f64().into();
        let receipt: Receipt = serde_json::from_str(&storage::read_text(&directory.join("audio.json"), 65536)?)?;
        row["final_client"] = serde_json::to_value(receipt.web_client)?;
        row["sha256"] = storage::digest(&path)?.into();
        row["bytes"] = fs::metadata(&path)?.len().into();
        let decode_started = Instant::now();
        let wav = directory.join("audio.wav");
        crate::audio::decode(&path, &wav, &cancelled)?;
        let pcm = crate::audio::Wav::open(&wav)?;
        let seconds = pcm.frames as f64 / f64::from(crate::audio::SAMPLE_RATE);
        if !expected.is_finite() || (seconds - expected).abs() > 2.0 {
            return Err("production download duration mismatch".into());
        }
        row["decode_seconds"] = decode_started.elapsed().as_secs_f64().into();
        row["decoded_seconds"] = seconds.into();
        row["verified_complete_audio"] = true.into();
        Ok(())
    })();
    if let Err(error) = outcome {
        row["error_code"] = error.downcast_ref::<crate::protocol::Failure>().map_or("candidate_failed", |failure| failure.code).into();
        let failure = error.downcast_ref::<crate::protocol::Failure>();
        row["retryable"] = serde_json::json!(failure.and_then(|failure| failure.retryable));
        row["http_status"] = serde_json::json!(failure.and_then(crate::protocol::Failure::http_status));
        row["retry_after_seconds"] = serde_json::json!(failure.and_then(crate::protocol::Failure::retry_after_seconds));
        storage::atomic_write(&directory.join("error.txt"), error.to_string().as_bytes(), false)?;
    }
    row["cancelled"] = cancelled.load(Ordering::SeqCst).into();
    row["metrics"] = metrics::snapshot();
    row["seconds"] = started.elapsed().as_secs_f64().into();
    storage::atomic_write(&directory.join("result.json"), &serde_json::to_vec_pretty(&row)?, false)?;
    println!("PRODUCTION_CLIENT_PROBE {}", serde_json::to_string(&row)?);
    Ok(())
}

#[test]
#[ignore = "opt-in production restart of an interrupted owned acquisition trial"]
fn research_production_resume() -> crate::Result<()> {
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_PROBE_DIR").ok_or("set existing owned output directory")?);
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err("invalid research directory".into()); }
    let _lock = storage::Lock::acquire(&directory)?;
    let prior: Value = serde_json::from_str(&storage::read_text(&directory.join("result.json"), 65536)?)?;
    let receipt: Receipt = serde_json::from_str(&storage::read_text(&directory.join("audio.json"), 65536)?)?;
    let expected = prior["declared_seconds"].as_f64().ok_or("missing original declared duration")?;
    if prior["verified_complete_audio"] != false || prior["video_id"] != receipt.identity.video_id { return Err("expected an incomplete matching trial".into()); }
    metrics::reset();
    if let Ok(kind) = std::env::var("AGENT_YOUTUBE_RESEARCH_ACCESS_FAULT") {
        faults::arm(&kind, std::env::var("AGENT_YOUTUBE_RESEARCH_FAULT_AFTER_BYTES")?.parse()?)?;
    }
    if let Ok(mode) = std::env::var("AGENT_YOUTUBE_RESEARCH_STALL") {
        if !matches!(mode.as_str(), "renew" | "persistent") {
            return Err("invalid research stall mode".into());
        }
        let after_bytes = std::env::var("AGENT_YOUTUBE_RESEARCH_STALL_AFTER_BYTES")?.parse::<u64>()?;
        if after_bytes == 0 { return Err("stall requires a nonempty retained prefix".into()); }
        metrics::arm_stall(after_bytes, mode == "persistent");
    }
    if std::env::var("AGENT_YOUTUBE_RESEARCH_INJECT_SABR_DNS").as_deref() == Ok("1") {
        let after_bytes = std::env::var("AGENT_YOUTUBE_RESEARCH_DNS_AFTER_BYTES")
            .ok().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(0);
        metrics::arm_dns_failure(after_bytes);
    }
    let downloader = Downloader::youtube()?;
    let binary = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_BINARY").ok_or("set worker binary")?);
    *downloader.runtime.lock().map_err(|_| "worker lock failed")? = Some(crate::web::Worker::test_executable(&binary)?);
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();
    let mut row = serde_json::json!({"schema_version":1,"video_id":receipt.identity.video_id,"production_resume":true,"verified_complete_audio":false,"resumed_from_bytes":receipt.progress.bytes,"prefix_sha256":receipt.progress.digest});
    let outcome = (|| -> crate::Result<()> {
        let path = downloader.download(&receipt.identity.video_id, &directory.join("audio"), &cancelled)?;
        row["acquisition_seconds"] = started.elapsed().as_secs_f64().into();
        row["sha256"] = storage::digest(&path)?.into();
        row["bytes"] = fs::metadata(&path)?.len().into();
        let decode_started = Instant::now();
        let wav = directory.join("audio.wav");
        crate::audio::decode(&path, &wav, &cancelled)?;
        let seconds = crate::audio::Wav::open(&wav)?.frames as f64 / f64::from(crate::audio::SAMPLE_RATE);
        if !expected.is_finite() || (expected-seconds).abs() > 2.0 { return Err("resumed audio duration mismatch".into()); }
        row["decode_seconds"] = decode_started.elapsed().as_secs_f64().into();
        row["decoded_seconds"] = seconds.into();
        row["declared_seconds"] = expected.into();
        row["verified_complete_audio"] = true.into();
        Ok(())
    })();
    if let Err(error) = outcome {
        row["error_code"] = error.downcast_ref::<crate::protocol::Failure>().map_or("candidate_failed", |failure| failure.code).into();
        let failure = error.downcast_ref::<crate::protocol::Failure>();
        row["retryable"] = serde_json::json!(failure.and_then(|failure| failure.retryable));
        row["http_status"] = serde_json::json!(failure.and_then(crate::protocol::Failure::http_status));
        row["retry_after_seconds"] = serde_json::json!(failure.and_then(crate::protocol::Failure::retry_after_seconds));
        storage::atomic_write(&directory.join("error.resume.txt"), error.to_string().as_bytes(), false)?;
    }
    row["metrics"] = metrics::snapshot();
    row["seconds"] = started.elapsed().as_secs_f64().into();
    storage::atomic_write(&directory.join("result.resume.json"), &serde_json::to_vec_pretty(&row)?, false)?;
    println!("PRODUCTION_RESUME_PROBE {}", serde_json::to_string(&row)?);
    Ok(())
}

#[test]
#[ignore = "opt-in production compatible-client recovery using a copied verified research prefix"]
fn research_production_client_recovery() -> crate::Result<()> {
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_PROBE_DIR").ok_or("set output directory")?);
    fs::create_dir(&directory)?;
    let _lock = storage::Lock::acquire(&directory)?;
    let prior = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_RECOVERY_PREFIX").ok_or("set research source prefix")?);
    let mut receipt: Receipt = serde_json::from_str(&storage::read_text(&suffix(&prior, ".receipt.json"), 65536)?)?;
    let prefix = directory.join("audio");
    let cancelled = AtomicBool::new(false);
    let mut prior_file = File::open(suffix(&prior, ".m4a"))?;
    verify_partial(&mut prior_file, &receipt.progress)?;
    prior_file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    prior_file.take(receipt.progress.bytes.min(RANGE_BYTES)).read_to_end(&mut bytes)?;
    receipt.progress = Progress { bytes:bytes.len() as u64, digest:format!("{:x}", Sha256::digest(&bytes)), ..Progress::default() };
    receipt.lease = None;
    storage::atomic_write(&suffix(&prefix, ".m4a"), &bytes, false)?;
    storage::atomic_write(&suffix(&prefix, ".json"), &serde_json::to_vec(&receipt)?, false)?;
    let id = receipt.identity.video_id.clone();
    let binary = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_BINARY").ok_or("set worker binary")?);
    let downloader = Downloader::youtube()?;
    *downloader.runtime.lock().map_err(|_| "worker lock failed")? = Some(crate::web::Worker::test_executable(&binary)?);
    let started = Instant::now();
    let mut row = serde_json::json!({"schema_version":1,"video_id":id,"production_recovery":true,"verified_complete_audio":false,"prefix_bytes":receipt.progress.bytes,"prefix_sha256":receipt.progress.digest});
    let outcome = (|| -> crate::Result<()> {
        let excluded = [receipt.web_client];
        let source = downloader.replace_source(&id, &prefix, &mut receipt, &excluded, &cancelled)?;
        let root: Value = serde_json::from_slice(&source.raw)?;
        let expected = root["videoDetails"]["lengthSeconds"].as_str().ok_or("missing duration")?.parse::<f64>()?;
        row["selected_client"] = serde_json::to_value(receipt.web_client)?;
        let path = downloader.download(&id, &prefix, &cancelled)?;
        row["sha256"] = storage::digest(&path)?.into();
        row["bytes"] = fs::metadata(&path)?.len().into();
        let wav = directory.join("audio.wav");
        crate::audio::decode(&path, &wav, &cancelled)?;
        let seconds = crate::audio::Wav::open(&wav)?.frames as f64 / f64::from(crate::audio::SAMPLE_RATE);
        if !expected.is_finite() || (seconds - expected).abs() > 2.0 { return Err("recovery duration mismatch".into()); }
        row["decoded_seconds"] = seconds.into();
        row["verified_complete_audio"] = true.into();
        Ok(())
    })();
    if let Some(mut worker) = downloader.runtime.lock().map_err(|_| "worker lock failed")?.take() { worker.close()?; }
    if let Err(error) = outcome {
        row["error_code"] = error.downcast_ref::<crate::protocol::Failure>().map_or("candidate_failed", |failure| failure.code).into();
        storage::atomic_write(&directory.join("error.txt"), error.to_string().as_bytes(), false)?;
    }
    row["seconds"] = started.elapsed().as_secs_f64().into();
    storage::atomic_write(&directory.join("result.json"), &serde_json::to_vec_pretty(&row)?, false)?;
    println!("PRODUCTION_RECOVERY_PROBE {}", serde_json::to_string(&row)?);
    Ok(())
}

// Real retained media over an explicitly owned loopback oracle. This hook is
// test-only; the CLI never accepts alternate YouTube origins or these options.
#[test]
#[ignore = "requires an owned loopback media oracle and retained complete source"]
fn research_loopback_retained_media() -> crate::Result<()> {
    let directory = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CLIENT_PROBE_DIR").ok_or("set owned media directory")?);
    let resume = std::env::var("AGENT_YOUTUBE_CLIENT_RESUME").as_deref() == Ok("1");
    if !resume { fs::create_dir(&directory)?; }
    let _lock = storage::Lock::acquire(&directory)?;
    let origin = std::env::var("AGENT_YOUTUBE_LOOPBACK_ORIGIN")?;
    let id = std::env::var("AGENT_YOUTUBE_CLIENT_VIDEO")?;
    let expected = std::env::var("AGENT_YOUTUBE_LOOPBACK_SECONDS")?.parse::<f64>()?;
    if !expected.is_finite() || expected <= 0.0 { return Err("invalid expected duration".into()); }
    let downloader = Downloader::loopback(&origin)?;
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();
    let mut row = serde_json::json!({"schema_version":1,"video_id":id,"verified_complete_audio":false,"declared_seconds":expected,"controlled_loopback":true});
    let result = (|| -> crate::Result<()> {
        let path = downloader.download(&id, &directory.join("audio"), &cancelled)?;
        row["acquisition_seconds"] = started.elapsed().as_secs_f64().into();
        let receipt: Receipt = serde_json::from_str(&storage::read_text(&directory.join("audio.json"),65536)?)?;
        row["declared_bytes"] = receipt.identity.length.into();
        row["bytes"] = fs::metadata(&path)?.len().into();
        row["sha256"] = storage::digest(&path)?.into();
        let decode_started = Instant::now();
        let wav = directory.join("audio.wav");
        crate::audio::decode(&path,&wav,&cancelled)?;
        let seconds = crate::audio::Wav::open(&wav)?.frames as f64 / f64::from(crate::audio::SAMPLE_RATE);
        if (seconds-expected).abs()>2.0 { return Err("owned media duration mismatch".into()); }
        row["decode_seconds"] = decode_started.elapsed().as_secs_f64().into();
        row["decoded_seconds"] = seconds.into();
        row["verified_complete_audio"] = true.into();
        Ok(())
    })();
    if let Err(error) = result {
        let failure = error.downcast_ref::<crate::protocol::Failure>();
        row["error_code"] = failure.map_or("candidate_failed",|f|f.code).into();
        row["retryable"] = serde_json::json!(failure.and_then(|f|f.retryable));
        row["retry_after_seconds"] = serde_json::json!(failure.and_then(crate::protocol::Failure::retry_after_seconds));
    }
    row["wall_seconds"] = started.elapsed().as_secs_f64().into();
    let name = if resume { "result.resume.json" } else { "result.json" };
    storage::atomic_write(&directory.join(name),&serde_json::to_vec_pretty(&row)?,false)?;
    println!("LOOPBACK_MEDIA_PROBE {}",serde_json::to_string(&row)?);
    Ok(())
}
