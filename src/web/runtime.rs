use super::{Request, Response, USER_AGENT, player, valid_solution};
use crate::{Result, download::http::Http};
use base64::{Engine, engine::general_purpose::STANDARD};
use boa_engine::{Context, Source};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const REQUEST_KEY: &str = "O43z0dpjhgX20SCx4KAo";
// Public protocol identifier distributed by BgUtils, not a user credential.
const PUBLIC_API_KEY: &str = "AIzaSyDyT5W0Jh49F30Pqqtyfdf7pDLFKLJoAnw";

#[derive(Default)]
pub(super) struct Runtime {
    minter: Option<Minter>,
    player: Option<([u8; 32], Context)>,
}
struct Minter {
    context: Context,
    expires: Instant,
}
fn context() -> Context {
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(10_000_000);
    context.runtime_limits_mut().set_recursion_limit(512);
    context
}
fn eval(context: &mut Context, code: &str) -> Result<boa_engine::JsValue> {
    // Script exceptions may embed attestation data or signed URLs.
    context
        .eval(Source::from_bytes(code))
        .map_err(|_| "Web script execution failed".into())
}
fn wait(context: &mut Context, expression: &str) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context
            .run_jobs()
            .map_err(|_| "Web promise execution failed")?;
        eval(context, "__pump()")?;
        let value = eval(context, &format!("JSON.stringify({expression})"))?;
        if let Some(text) = value.as_string() {
            let value: Value = serde_json::from_str(&text.to_std_string_escaped())?;
            if !value.is_null() {
                return Ok(value);
            }
        }
        if Instant::now() >= deadline {
            return Err("Web script did not settle within 15 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn decode(text: &str) -> Result<Vec<u8>> {
    let mut text = text.replace('-', "+").replace('_', "/").replace('.', "=");
    while !text.len().is_multiple_of(4) {
        text.push('=')
    }
    Ok(STANDARD.decode(text)?)
}
fn integrity_rpc(http: &Http, snapshot: &Value) -> Result<Value> {
    let bytes = http.attestation_bytes(
        "https://www.youtube.com/api/jnn/v1/GenerateIT",
        Some(&serde_json::to_vec(&json!([REQUEST_KEY, snapshot]))?),
        &[
            ("Content-Type", "application/json+protobuf"),
            ("x-goog-api-key", PUBLIC_API_KEY),
            ("User-Agent", USER_AGENT),
            ("x-user-agent", "grpc-web-javascript/0.1"),
        ],
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn integrity(response: &Value) -> Result<(Vec<u8>, Duration)> {
    let token = response
        .get(0)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("attestation returned no integrity token")?;
    let ttl = response
        .get(1)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or("attestation returned no positive token lifetime")?;
    let bytes = decode(token)?;
    if bytes.is_empty() {
        return Err("empty integrity token".into());
    }
    // Refresh conservatively. Never cache a fallback token as successful attestation.
    Ok((
        bytes,
        Duration::from_secs(ttl.min(86_400).saturating_sub(60).max(1)),
    ))
}
fn interpreter_url(url: &str) -> Result<String> {
    let url = if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_owned()
    };
    let uri: ureq::http::Uri = url.parse().map_err(|_| "invalid interpreter URI")?;
    if uri.scheme_str() != Some("https")
        || uri.host() != Some("www.google.com")
        || !(uri.path().starts_with("/js/bg/") || uri.path().starts_with("/js/th/"))
        || uri.port_u16().is_some_and(|p| p != 443)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || uri.path().split('/').any(|p| p == "." || p == "..")
        || url.contains('#')
    {
        return Err("untrusted interpreter origin".into());
    }
    Ok(url)
}

impl Minter {
    fn new() -> Result<Self> {
        let http = Http::attestation()?;
        let bytes = http.attestation_bytes(
            "https://www.youtube.com/",
            None,
            &[("User-Agent", USER_AGENT)],
        )?;
        Self::from_page(&http, &bytes)
    }
    fn from_page(http: &Http, bytes: &[u8]) -> Result<Self> {
        let (page_challenge, config) = super::page::extract(std::str::from_utf8(bytes)?)?;
        let url = page_challenge
            .pointer("/interpreterUrl/privateDoNotAccessOrElseTrustedResourceUrlWrappedValue")
            .and_then(Value::as_str)
            .ok_or("page challenge has no interpreter URL")?;
        let url = interpreter_url(url)?;
        let script = http.attestation_bytes(&url, None, &[("User-Agent", USER_AGENT)])?;
        let script = String::from_utf8(script)?;
        let program = page_challenge
            .get("program")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("page challenge has no program")?;
        let global = page_challenge
            .get("globalName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("page challenge has no global name")?;
        let mut context = context();
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
        .map_err(|_| "cannot initialize Web APIs")?;
        eval(&mut context, include_str!("environment.js"))?;
        eval(
            &mut context,
            &format!(
                "globalThis.yt={{config_:JSON.parse({})}};window.yt=globalThis.yt;",
                serde_json::to_string(&serde_json::to_string(&config)?)?
            ),
        )?;
        eval(
            &mut context,
            include_str!("../../vendor/botguard/dom.min.js"),
        )?;
        let setup = format!(
            r#"
            delete globalThis.navigator;
            globalThis.__snapshot=null;globalThis.__failed=false;
            runBotguard({}, {}, {}, {}).then(r=>{{__snapshot=r;__signals=webPoSignalOutput}},()=>{{__failed=true}});
        "#,
            serde_json::to_string(&script)?,
            serde_json::to_string(program)?,
            serde_json::to_string(global)?,
            serde_json::to_string(USER_AGENT)?
        );
        eval(&mut context, &setup)?;
        let snapshot = wait(&mut context, "__failed ? false : __snapshot")?;
        if !snapshot.is_string() {
            return Err("BotGuard snapshot failed".into());
        }
        let response = integrity_rpc(http, &snapshot)?;
        let (bytes, lifetime) = integrity(&response)?;
        eval(
            &mut context,
            &format!(
                r#"
            globalThis.__ready=null;
            Promise.resolve((0,__signals[0])(new Uint8Array({}))).then(
                mint=>{{__tokenMinter=mint;__ready=true}},()=>{{__ready=false}});
        "#,
                serde_json::to_string(&bytes)?
            ),
        )?;
        if wait(&mut context, "__ready")? != true {
            return Err("cannot initialize PO-token minter".into());
        }
        Ok(Self {
            context,
            expires: Instant::now() + lifetime,
        })
    }
    fn mint(&mut self, video: &str) -> Result<Response> {
        eval(
            &mut self.context,
            &format!(
                r#"
            globalThis.__minted=null;
            Promise.resolve((0,__tokenMinter)(new Uint8Array({})))
                .then(b=>{{__minted=Array.from(b)}},()=>{{__minted=false}});
        "#,
                serde_json::to_string(video.as_bytes())?
            ),
        )?;
        let token: Vec<u8> = serde_json::from_value(wait(&mut self.context, "__minted")?)?;
        if !(64..=4096).contains(&token.len()) {
            return Err("invalid minted PO token".into());
        }
        let expires_in = self
            .expires
            .saturating_duration_since(Instant::now())
            .as_secs();
        if expires_in == 0 {
            return Err("integrity token expired while minting".into());
        }
        Ok(Response {
            token,
            expires_in,
            solution: String::new(),
            failed: false,
            failure: None,
        })
    }
}
impl Runtime {
    fn mint_with(
        &mut self,
        video: &str,
        refresh: bool,
        create: impl FnOnce() -> Result<Minter>,
    ) -> Result<Response> {
        if video.len() != 11
            || !video
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("invalid video binding".into());
        }
        if refresh
            || self
                .minter
                .as_ref()
                .is_none_or(|m| Instant::now() >= m.expires)
        {
            self.minter = None;
            self.minter = Some(create()?);
        }
        self.minter
            .as_mut()
            .ok_or("missing token minter")?
            .mint(video)
    }
    pub(super) fn handle(&mut self, request: Request) -> Result<Response> {
        match request {
            Request::Mint { video, refresh } => self.mint_with(&video, refresh, Minter::new),
            Request::N { player, challenge } => {
                if !valid_solution(&challenge) {
                    return Err("invalid player challenge".into());
                }
                let digest: [u8; 32] = Sha256::digest(player.as_bytes()).into();
                if self.player.as_ref().is_none_or(|(key, _)| *key != digest) {
                    self.player = None;
                    let prepared = player::prepare(&player)?;
                    let mut context = context();
                    eval(&mut context, &format!("globalThis._result={{}};{prepared}"))?;
                    self.player = Some((digest, context));
                }
                let (_, context) = self.player.as_mut().ok_or("missing player runtime")?;
                let value = eval(
                    context,
                    &format!("_result.n({})", serde_json::to_string(&challenge)?),
                )?;
                let solution = value
                    .as_string()
                    .ok_or("player returned no solution")?
                    .to_std_string_escaped();
                if !valid_solution(&solution) {
                    return Err("invalid player solution".into());
                }
                Ok(Response {
                    token: Vec::new(),
                    expires_in: 0,
                    solution,
                    failed: false,
                    failure: None,
                })
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/experiments/client-access/context.rs"
    ));
    #[test]
    fn rejects_fallback_tokens_and_bounds_expiry() {
        assert!(integrity(&json!([null, 43200, null, "fallback"])).is_err());
        assert!(integrity(&json!(["YQ==", 0])).is_err());
        assert_eq!(
            integrity(&json!(["YQ==", 43200])).unwrap().1.as_secs(),
            43140
        );
        assert_eq!(integrity(&json!(["YQ==", 1])).unwrap().1.as_secs(), 1);
    }
    #[test]
    fn cached_minter_keeps_lifetime_and_mints_for_each_video() {
        let mut context = context();
        eval(&mut context,"globalThis.__pump=()=>{};globalThis.counter=0;globalThis.__tokenMinter=v=>{const b=new Uint8Array(64+v.length);b.set(v,64);b[0]=++counter;return b}").unwrap();
        let mut runtime = Runtime {
            minter: Some(Minter {
                context,
                expires: Instant::now() + Duration::from_secs(120),
            }),
            player: None,
        };
        let a = runtime
            .handle(Request::Mint {
                video: "a2hbJlPxj4M".into(),
                refresh: false,
            })
            .unwrap();
        let b = runtime
            .handle(Request::Mint {
                video: "of4B3aw7MXo".into(),
                refresh: false,
            })
            .unwrap();
        assert_eq!(&a.token[64..], b"a2hbJlPxj4M");
        assert_eq!(&b.token[64..], b"of4B3aw7MXo");
        assert_eq!((a.token[0], b.token[0]), (1, 2));
        assert!(b.expires_in > 0 && b.expires_in <= a.expires_in && a.expires_in <= 120);
        assert!(
            runtime
                .handle(Request::Mint {
                    video: "invalid".into(),
                    refresh: false
                })
                .is_err()
        );
    }
    #[test]
    fn interpreter_origin_is_restricted_without_redirects() {
        for url in [
            "http://www.google.com/js/th/a.js",
            "https://evil.com/js/th/a.js",
            "https://u@www.google.com/js/th/a.js",
            "https://www.google.com:444/js/th/a.js",
            "https://www.google.com/js/th/../../url",
            "https://www.google.com/url?x=1",
            "https://www.google.com/js/th/a.js#x",
            "//127.0.0.1/js/th/a.js",
        ] {
            assert!(interpreter_url(url).is_err(), "{url}");
        }
        assert_eq!(
            interpreter_url("//www.google.com/js/th/public.js").unwrap(),
            "https://www.google.com/js/th/public.js"
        );
    }
    fn fake_minter(tag: u8, expires: Instant) -> Minter {
        let mut context = context();
        eval(&mut context,&format!("globalThis.__pump=()=>{{}};globalThis.__tokenMinter=v=>{{const b=new Uint8Array(64+v.length);b.set(v,64);b[0]={tag};return b}}")).unwrap();
        Minter { context, expires }
    }
    #[test]
    fn expiry_and_explicit_refresh_replace_the_minter_without_reusing_bindings() {
        let mut runtime = Runtime {
            minter: Some(fake_minter(1, Instant::now() - Duration::from_secs(1))),
            player: None,
        };
        let valid = || Instant::now() + Duration::from_secs(120);
        let first = runtime
            .mint_with("a2hbJlPxj4M", false, || Ok(fake_minter(2, valid())))
            .unwrap();
        assert_eq!(first.token[0], 2);
        let cached = runtime
            .mint_with("of4B3aw7MXo", false, || {
                panic!("unexpired minter must be reused")
            })
            .unwrap();
        assert_eq!(cached.token[0], 2);
        assert_eq!(&cached.token[64..], b"of4B3aw7MXo");
        let refreshed = runtime
            .mint_with("a2hbJlPxj4M", true, || Ok(fake_minter(3, valid())))
            .unwrap();
        assert_eq!(refreshed.token[0], 3);
        assert_eq!(&refreshed.token[64..], b"a2hbJlPxj4M");
        assert!(
            runtime
                .mint_with("a2hbJlPxj4M", true, || Err(
                    "fixture acquisition failure".into()
                ))
                .is_err()
        );
        assert!(runtime.minter.is_none());
    }
}
