mod media;
#[path = "../../../src/web/player.rs"]
mod player;

use base64::{Engine, engine::general_purpose::STANDARD};
use boa_engine::{Context, Source};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::Arc,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const REQUEST_KEY: &str = "O43z0dpjhgX20SCx4KAo";
// API key publicly distributed in YouTube player code and BgUtils.
// See vendor/botguard/README.md for first-party provenance and scope limits.
const PUBLIC_API_KEY: &str = "AIzaSyDyT5W0Jh49F30Pqqtyfdf7pDLFKLJoAnw";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.10 Safari/605.1.1";
const LIMIT: u64 = 8 * 1024 * 1024;

fn main() {
    // The experimental VM has no host I/O bindings. Bound the entire probe,
    // including jobs that never settle, independently of JavaScript timers.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(90));
        eprintln!("probe deadline exceeded");
        std::process::exit(124);
    });
    let result = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| run().map_err(|e| e.to_string()));
    let result = match result {
        Ok(worker) => worker
            .join()
            .unwrap_or_else(|_| Err("probe worker panicked".into())),
        Err(_) => Err("could not start probe worker".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
                .build(),
        )
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent()
}

fn rpc(agent: &ureq::Agent, method: &str, payload: &Value) -> Result<Value> {
    if !matches!(method, "Create" | "GenerateIT") {
        return Err("unsupported attestation method".into());
    }
    let base = match std::env::var("AGENT_YOUTUBE_BG_API").as_deref() {
        Ok("google") => "https://jnn-pa.googleapis.com/$rpc/google.internal.waa.v1.Waa",
        Ok("youtube") | Err(std::env::VarError::NotPresent) => "https://www.youtube.com/api/jnn/v1",
        _ => return Err("AGENT_YOUTUBE_BG_API must be youtube or google".into()),
    };
    let url = format!("{base}/{method}");
    let mut response = agent
        .post(&url)
        .header("Content-Type", "application/json+protobuf")
        .header("x-goog-api-key", PUBLIC_API_KEY)
        .header("User-Agent", USER_AGENT)
        .header("x-user-agent", "grpc-web-javascript/0.1")
        .header("Accept-Encoding", "identity")
        .send(serde_json::to_vec(payload)?)
        .map_err(|_| format!("{method}: HTTP request failed"))?;
    Ok(serde_json::from_slice(&read_limited(
        response.body_mut().as_reader(),
    )?)?)
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err("probe input exceeds its byte limit".into());
    }
    Ok(bytes)
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let mut normalized = text.replace('-', "+").replace('_', "/").replace('.', "=");
    while !normalized.len().is_multiple_of(4) {
        normalized.push('=');
    }
    Ok(STANDARD.decode(normalized)?)
}

fn challenge(raw: Value) -> Result<Value> {
    let parsed = if let Some(scrambled) = raw.get(1).and_then(Value::as_str) {
        let bytes: Vec<_> = decode_base64(scrambled)?
            .into_iter()
            .map(|b| b.wrapping_add(97))
            .collect();
        serde_json::from_slice(&bytes)?
    } else {
        raw.get(0).cloned().ok_or("missing challenge")?
    };
    if !parsed.is_array()
        || parsed.get(4).and_then(Value::as_str).is_none()
        || parsed.get(5).and_then(Value::as_str).is_none()
    {
        return Err("invalid challenge structure".into());
    }
    Ok(parsed)
}

fn shape(value: &Value) -> Value {
    match value {
        Value::Array(a) => Value::Array(a.iter().take(8).map(shape).collect()),
        Value::String(s) => json!({"string_bytes": s.len()}),
        Value::Object(_) => json!("object"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("boolean"),
        Value::Null => Value::Null,
    }
}

fn integrity_data(response: &Value) -> Result<(Vec<u8>, u64)> {
    let token = response
        .get(0)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("GenerateIT did not return an integrity token")?;
    let ttl = response
        .get(1)
        .and_then(Value::as_u64)
        .filter(|ttl| *ttl > 0)
        .ok_or("GenerateIT did not return a positive estimated TTL")?;
    let bytes = decode_base64(token)?;
    if bytes.is_empty() {
        return Err("empty integrity token".into());
    }
    Ok((bytes, ttl))
}

fn eval(context: &mut Context, code: &str, stage: &str) -> Result<boa_engine::JsValue> {
    context.eval(Source::from_bytes(code)).map_err(|error| {
        // Never print arbitrary script errors: they may contain attestation data.
        eprintln!(
            "{stage}: JavaScript exception ({})",
            error.as_native().map_or("opaque", |e| match e.kind() {
                boa_engine::JsNativeErrorKind::Syntax => "syntax",
                boa_engine::JsNativeErrorKind::Type => "type",
                boa_engine::JsNativeErrorKind::Reference => "reference",
                _ => "other",
            })
        );
        format!("{stage} failed").into()
    })
}

fn wait(context: &mut Context, expression: &str, stage: &str) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context
            .run_jobs()
            .map_err(|_| format!("{stage}: promise job failed"))?;
        eval(context, "__pump()", stage)?;
        let value = eval(context, &format!("JSON.stringify({expression})"), stage)?;
        if let Some(text) = value.as_string() {
            let value: Value = serde_json::from_str(&text.to_std_string_escaped())?;
            if !value.is_null() {
                return Ok(value);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("{stage}: did not settle within 15 seconds").into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn run() -> Result<()> {
    let binding = std::env::args()
        .nth(1)
        .ok_or("usage: agent-youtube-botguard-probe VIDEO_ID")?;
    if binding.len() != 11
        || !binding
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("expected an 11-character video ID".into());
    }
    let agent = agent();
    let input: Option<Value> = std::env::var_os("AGENT_YOUTUBE_BG_INPUT")
        .map(|path| -> Result<Value> {
            Ok(serde_json::from_slice(&read_limited(
                std::fs::File::open(path)?,
            )?)?)
        })
        .transpose()?;
    if let Some(snapshot) = input
        .as_ref()
        .and_then(|v| v.get("snapshot"))
        .filter(|v| v.is_string())
    {
        let response = rpc(&agent, "GenerateIT", &json!([REQUEST_KEY, snapshot]))?;
        eprintln!("HTTP control response shape: {}", shape(&response));
        let (_, ttl) = integrity_data(&response)?;
        println!(
            "{}",
            json!({"http_control_integrity_issued":true,"ttl":ttl})
        );
        return Ok(());
    }
    let challenge = if let Some(input) = &input {
        challenge(json!([input["challenge"]]))?
    } else {
        challenge(rpc(&agent, "Create", &json!([REQUEST_KEY]))?)?
    };
    let script = script_from_challenge(&challenge)?;
    eprintln!("challenge received; interpreter {} bytes", script.len());
    let mut context = Context::default();
    boa_runtime::register_extensions(
        (
            boa_runtime::extensions::EncodingExtension,
            boa_runtime::extensions::UrlExtension,
            boa_runtime::extensions::StructuredCloneExtension,
            boa_runtime::extensions::MicrotaskExtension,
            boa_runtime::extensions::Base64Extension,
        ),
        None,
        &mut context,
    )
    .map_err(|_| "could not register Web API extensions")?;
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(10_000_000);
    context.runtime_limits_mut().set_recursion_limit(512);
    eval(&mut context, include_str!("environment.js"), "environment")?;
    eval(
        &mut context,
        &format!(
            "navigator.userAgent = {};",
            serde_json::to_string(USER_AGENT)?
        ),
        "user agent",
    )?;
    if let Some(config) = input.as_ref().and_then(|v| v.get("config")) {
        if !config.is_object() {
            return Err("fixture config must be an object".into());
        }
        eval(
            &mut context,
            &format!(
                "globalThis.yt = {{config_: {}}}; window.yt = globalThis.yt;",
                serde_json::to_string(config)?
            ),
            "page context",
        )?;
    }
    let bundle = std::env::var_os("AGENT_YOUTUBE_BG_BUNDLE");
    if let Some(bundle) = &bundle {
        let script = String::from_utf8(read_limited(std::fs::File::open(bundle)?)?)?;
        eval(&mut context, &script, "DOM bundle")?;
        // Preflight uses only fixed public strings, so its error can be inspected
        // before any attestation data is exposed to the VM.
        eval(
            &mut context,
            r#"
            delete globalThis.navigator;
            globalThis.__domResult = null;
            globalThis.__probe_vm = {a(program, setup) {
                setup(callback => callback('dom-ready'), () => {}, () => {}, () => {});
                return [() => {}];
            }};
            runBotguard('', '', '__probe_vm', 'probe').then(
                () => { __domResult = 'ready'; },
                e => { __domResult = String(e).slice(0, 300); });
        "#,
            "DOM preflight",
        )?;
        let result = wait(&mut context, "__domResult", "DOM preflight")?;
        if result != "ready" {
            return Err(format!("DOM preflight: {result}").into());
        }
        let setup = format!(
            r#"
            delete globalThis.navigator;
            globalThis.__snapshot = null;
            globalThis.__bundleError = null;
            runBotguard({}, {}, {}, {}).then(r => {{ __snapshot = r; __signals = webPoSignalOutput; }},
                e => {{ __bundleError = {{error: 'DOM bundle rejected',
                    api: ['TextEncoder','TextDecoder','URL','URLSearchParams','AbortController','Buffer','Deno','structuredClone']
                        .filter(api => String(e.message).includes(api))}}; }});
        "#,
            serde_json::to_string(script_from_challenge(&challenge)?)?,
            serde_json::to_string(&challenge[4])?,
            serde_json::to_string(&challenge[5])?,
            serde_json::to_string(USER_AGENT)?
        );
        eval(&mut context, &setup, "DOM bundle setup")?;
    } else {
        eval(&mut context, script, "interpreter")?;
        eprintln!("interpreter evaluated");
        let setup = format!(
            r#"
        globalThis.__snapshot = null;
        globalThis.__signals = [];
        globalThis.__vmFunctions = null;
        globalThis.__setupError = false;
        globalThis.__events = [];
        Promise.resolve(globalThis[{}].a({}, (...f) => {{ __vmFunctions = f; }}, true,
            undefined, () => {{}}, [[], []], undefined, false,
            [()=>{{}}, code=>{{ if (typeof code === 'number') __events.push(code); }},()=>{{}},()=>{{}},()=>{{}}]))
            .catch(() => {{ __setupError = true; }});
    "#,
            serde_json::to_string(&challenge[5])?,
            serde_json::to_string(&challenge[4])?
        );
        eval(&mut context, &setup, "VM setup")?;
        let setup = wait(
            &mut context,
            "__setupError ? 'error' : (__vmFunctions ? 'ready' : null)",
            "VM setup",
        )?;
        if setup != "ready" {
            return Err("VM setup rejected".into());
        }
        eprintln!("VM setup complete");
        eval(
            &mut context,
            "(0, __vmFunctions[0])(r => { __snapshot = r; }, [undefined, undefined, __signals, undefined])",
            "snapshot",
        )?;
    }
    let snapshot = wait(
        &mut context,
        if bundle.is_some() {
            "__bundleError || __snapshot"
        } else {
            "__snapshot"
        },
        "snapshot",
    )?;
    if snapshot.is_object() {
        return Err(format!("{snapshot}").into());
    }
    let missing = eval(
        &mut context,
        "JSON.stringify(Array.from(__missing).slice(0, 100))",
        "diagnostics",
    )?;
    eprintln!(
        "Missing environment APIs: {}",
        missing
            .as_string()
            .ok_or("invalid diagnostics")?
            .to_std_string_escaped()
    );
    let events = eval(
        &mut context,
        "JSON.stringify(typeof __events === 'undefined' ? [] : __events.slice(0, 100))",
        "diagnostics",
    )?;
    eprintln!(
        "VM client error codes: {}",
        events
            .as_string()
            .ok_or("invalid diagnostics")?
            .to_std_string_escaped()
    );
    if !snapshot.is_string() {
        return Err("snapshot is not a string".into());
    }
    eprintln!(
        "snapshot produced ({} bytes); exchanging for integrity token",
        snapshot.as_str().map_or(0, str::len)
    );
    let integrity = rpc(&agent, "GenerateIT", &json!([REQUEST_KEY, snapshot]))?;
    eprintln!("GenerateIT response shape: {}", shape(&integrity));
    let (bytes, ttl) = integrity_data(&integrity)?;
    eprintln!("GenerateIT: token issued; estimated TTL {} seconds", ttl);
    let mint = format!(
        r#"
        globalThis.__minted = null;
        Promise.resolve((0, __signals[0])(new Uint8Array({})))
            .then(mint => mint(new Uint8Array({})))
            .then(bytes => {{ __minted = Array.from(bytes); }}, () => {{ __minted = 'error'; }});
    "#,
        serde_json::to_string(&bytes)?,
        serde_json::to_string(binding.as_bytes())?
    );
    eval(&mut context, &mint, "mint")?;
    let minted = wait(&mut context, "__minted", "mint")?;
    let bytes: Vec<u8> = serde_json::from_value(minted)?;
    if !(64..=4096).contains(&bytes.len()) {
        return Err("mint returned an unexpected token length".into());
    }
    let acceptance = match std::env::var("AGENT_YOUTUBE_BG_MEDIA_CHECK").as_deref() {
        Ok("1") => media::check(&agent, &binding, &bytes)?,
        Err(std::env::VarError::NotPresent) => json!("not_tested"),
        _ => return Err("AGENT_YOUTUBE_BG_MEDIA_CHECK must be 1 when set".into()),
    };
    // Tokens deliberately never go to stdout or retained project records.
    println!(
        "{}",
        json!({"challenge":true,"snapshot":true,"integrity_token":true,"po_token_bytes":bytes.len(),"acceptance":acceptance})
    );
    Ok(())
}

fn script_from_challenge(challenge: &Value) -> Result<&str> {
    challenge[1]
        .as_array()
        .and_then(|a| a.iter().filter_map(Value::as_str).find(|s| !s.is_empty()))
        .ok_or_else(|| "challenge does not include an inline interpreter".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_scrambled_challenges() -> Result<()> {
        let input = json!(["id", ["script"], null, "hash", "program", "global"]);
        assert_eq!(challenge(json!([input.clone()]))?, input);
        let scrambled: Vec<_> = serde_json::to_vec(&input)?
            .into_iter()
            .map(|b| b.wrapping_sub(97))
            .collect();
        assert_eq!(challenge(json!([null, STANDARD.encode(scrambled)]))?, input);
        assert!(challenge(json!([])).is_err());
        assert!(challenge(json!([null, "%%"])).is_err());
        assert!(challenge(json!([[null]])).is_err());
        Ok(())
    }

    #[test]
    fn fallback_and_invalid_integrity_responses_are_never_success() -> Result<()> {
        for value in [
            json!([null, 600, null, "fallback"]),
            json!(["", 600]),
            json!(["%%", 600]),
            json!(["AQ==", 0]),
            json!(["AQ==", -1]),
            json!(["AQ==", "600"]),
            json!([]),
        ] {
            assert!(integrity_data(&value).is_err());
        }
        assert_eq!(integrity_data(&json!(["AQ==", 600]))?, (vec![1], 600));
        assert_eq!(
            shape(&json!(["private-token", {"private": "value"}])),
            json!([{"string_bytes":13}, "object"])
        );
        Ok(())
    }

    #[test]
    fn timer_callbacks_and_promises_settle_without_host_io() -> Result<()> {
        let mut context = Context::default();
        eval(
            &mut context,
            include_str!("environment.js"),
            "environment test",
        )?;
        eval(
            &mut context,
            "globalThis.result = null; setTimeout(() => Promise.resolve(42).then(n => result = n), 0)",
            "timer test",
        )?;
        assert_eq!(wait(&mut context, "result", "timer test")?, json!(42));
        context.runtime_limits_mut().set_loop_iteration_limit(100);
        assert!(
            context
                .eval(Source::from_bytes("while (true) {} "))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn interval_can_cancel_itself() -> Result<()> {
        let mut context = Context::default();
        eval(
            &mut context,
            include_str!("environment.js"),
            "environment test",
        )?;
        eval(
            &mut context,
            "globalThis.result=null;let count=0;const id=setInterval(()=>{if(++count===2){clearInterval(id);result=count}},0)",
            "interval test",
        )?;
        assert_eq!(wait(&mut context, "result", "interval test")?, json!(2));
        assert_eq!(
            eval(&mut context, "__timers.length", "timer count")?.as_number(),
            Some(0.0)
        );
        Ok(())
    }

    #[test]
    fn oversized_probe_input_is_rejected() {
        assert!(read_limited(std::io::repeat(0)).is_err());
    }
}
