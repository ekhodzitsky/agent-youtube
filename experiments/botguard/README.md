# BotGuard / Boa controls

This isolated workspace retains the classic `Create` feasibility probe, offline
JavaScript semantics controls, and paired comparison with a maintained Node
provider. The production implementation is in `src/web/` and uses a fresh homepage
challenge/configuration pair. See the [acquisition and recovery contract](../../docs/storage-and-recovery.md).

The default probe now uses the corrected Boa 0.22.0 copies in `vendor/` and browser
Annex B semantics. No Node, Deno, V8, browser or external downloader is invoked by
the Rust probe. Downloaded JavaScript runs in the Rust interpreter.

Classic Create can issue genuine integrity and video-bound PO tokens, but those
tokens stayed pending at GVS and stopped after the initial 69.892 seconds in the
controlled native SABR test. **Issuance and an initial audio prefix do not prove
media authorization.** The production homepage flow returned accepted status and
completed the full long recording. Fallback tokens remain rejected.

## Run the classic control

From the repository root:

```sh
cargo build --release --locked --manifest-path experiments/botguard/Cargo.toml
prlimit --as=4294967296 -- experiments/botguard/target/release/agent-youtube-botguard-probe a2hbJlPxj4M
```

The probe has a 90-second process deadline, 30-second HTTP deadline, 15-second stage
waits, an 8 MiB input/HTTP limit, VM loop/recursion limits and a 64 MiB worker stack.
`prlimit` is an external research invocation supplying its heap/address-space cap;
the production worker sets its own Unix process limit. The probe's JavaScript has
no host filesystem, process or network API. No cookies are read. Its stdout contains
redacted stage/byte summaries, never attestation values.

`AGENT_YOUTUBE_BG_API=google` selects Google's fixed WAA endpoint; the default `youtube`
selects JNN. This is a comparison option, not a product setting.

`AGENT_YOUTUBE_BG_BUNDLE` accepts a local fuller DOM bundle exposing
`runBotguard(script, program, globalName, userAgent)` and `webPoSignalOutput`.
Use the fixed bundled environment from this repository:

```sh
AGENT_YOUTUBE_BG_BUNDLE=vendor/botguard/dom.min.js \
  prlimit --as=4294967296 -- experiments/botguard/target/release/agent-youtube-botguard-probe a2hbJlPxj4M
```

It is checked with a fixed mock VM before live attestation. The optional
`AGENT_YOUTUBE_BG_MEDIA_CHECK=1` makes a bounded MWEB player/direct-audio-prefix check,
using shared native player preparation. That client may be unavailable; the probe
reports that separately. Even `audio_prefix_received` does not establish GVS token
acceptance. This checker is not the production Web SABR path.

## Eval regressions and paired reference

```sh
experiments/botguard/target/release/eval-probe
```

The diagnostic now exits 0 for 24 cases: repeated calls, dynamic locals, closures,
recursion, deletion/redeclaration, static lexical/parameter TDZ, callable aliases,
arguments and mapped parameters, arrows, with receivers, and browser Annex B.
All agree with V8. The unit test runs the same controls. Stock Boa fails the original
eval-scope and arguments cases; the general correction does not use name-specific
bypasses. `vendor/changes.patch` and its provenance make the small fork reviewable.

To compare a fresh paired challenge with the maintained provider, build the pinned
bgutil-ytdlp-pot-provider reference from an explicitly inspected checkout, then run:

```sh
AGENT_YOUTUBE_BG_BUNDLE=vendor/botguard/dom.min.js \
  node experiments/botguard/compare.mjs runs/botguard-research/upstream-bgutil/server
```

Node is used only by this explicitly requested comparison driver. It reports Rust
VM issuance, Rust HTTP exchange of a V8 snapshot, reference issuance/minting and
media acceptance separately. Paired fixtures and private logs are created under a
fresh ignored `runs/` directory with owner-only access. `AGENT_YOUTUBE_BG_INPUT` accepts one
such `challenge.json`; `snapshot.json` tests only the HTTP exchange. Inputs are
limited to 8 MiB and values are never printed by the Rust probe.

`diagnostic-eval.patch` remains the **unsafe, unapplied counterfactual** that first
established causality. It breaks a valid callable alias and must never be adopted.
The current fork instead records each invocation's actual eval binding presence
and correct implicit arguments access. The original failed controls remain historical local evidence. The shared
[engine patch and provenance](vendor/README.md) describe the adopted correction;
[full-source qualification](../../docs/acquisition-051-qualification.md) has a
separate, explicitly dated scope.

## Dependency cost and checks

This is a separately locked workspace. Five Boa crates provide execution, existing
Web APIs and shared native AST preparation; default engine features are disabled
except browser Annex B. TLS uses the application's ureq/RustCrypto configuration.
The Linux normal/build graph contains 231 package versions including the probe;
the production application contains 237. Neither active graph contains V8,
deno_core, QuickJS, ring, aws-lc-sys, openssl-sys, cc or cmake. The probe lockfile also
includes engine development packages for reproducible upstream tests; those are
not normal/build runtime dependencies of the application.

```sh
cargo fmt --manifest-path experiments/botguard/Cargo.toml --check
cargo clippy --locked --manifest-path experiments/botguard/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path experiments/botguard/Cargo.toml
cargo test --locked --manifest-path experiments/botguard/Cargo.toml -p boa_engine --no-default-features --features annex-b --lib
cargo test --locked --manifest-path experiments/botguard/Cargo.toml -p boa_ast --lib
```

The application and probe share the corrected engine source. Both packages remain
private and unpublished. No engine registry cache or other project was modified.
