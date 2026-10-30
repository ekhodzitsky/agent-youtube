// Included only by Web runtime tests. The caller bounds the test process lifetime.
#[test]
#[ignore = "opt-in live attestation research using an owned watch-page snapshot"]
fn research_watch_context_attestation() -> crate::Result<()> {
    use std::{fs, path::Path};
    let path = std::env::var_os("AGENT_YOUTUBE_CONTEXT_PAGE").ok_or("set AGENT_YOUTUBE_CONTEXT_PAGE")?;
    let output = std::env::var_os("AGENT_YOUTUBE_CONTEXT_OUTPUT").ok_or("set AGENT_YOUTUBE_CONTEXT_OUTPUT")?;
    let id = std::env::var("AGENT_YOUTUBE_CONTEXT_VIDEO").unwrap_or_else(|_| "of4B3aw7MXo".into());
    let bytes = crate::storage::read_text(Path::new(&path), 16 * 1024 * 1024)?;
    let started = Instant::now();
    let http = Http::attestation()?;
    let result = Minter::from_page(&http, bytes.as_bytes()).and_then(|mut minter| minter.mint(&id));
    let mut summary = json!({"schema_version":1,"video_id":id,"watch_context":true,"token_issued":false,"complete_audio":false});
    match result {
        Ok(response) => {
            summary["token_issued"] = true.into();
            summary["token_bytes"] = response.token.len().into();
            summary["expires_in"] = response.expires_in.into();
            // Private owned evidence, never copied into a project record.
            let token = Path::new(&output).with_extension("token.json");
            crate::storage::atomic_write(&token, &serde_json::to_vec(&response)?, false)?;
        }
        Err(_) => summary["error_code"] = "watch_context_attestation_failed".into(),
    }
    summary["seconds"] = started.elapsed().as_secs_f64().into();
    let encoded = serde_json::to_vec_pretty(&summary)?;
    crate::storage::atomic_write(Path::new(&output), &encoded, false)?;
    assert!(fs::metadata(output)?.len() > 0);
    println!("CONTEXT_PROBE {}", serde_json::to_string(&summary)?);
    Ok(())
}
