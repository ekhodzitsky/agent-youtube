# Development

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Development

Development uses a single branch, `main`; completed work is committed and pushed directly
to it. Separate branches and pull requests require an explicit owner request.

```sh
python3 -m venv tools-local/checks
tools-local/checks/bin/python -m pip install -r tools/check-requirements.txt
export PATH="$PWD/tools-local/checks/bin:$PATH"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s experiments/qualification
python3 tools/check_docs.py
```

Twenty-seven direct dependencies: serde, serde_json, sha2, ctrlc, syom, bandlimit, ryf,
ureq, rustls, rustls-rustcrypto, base64, boa_engine, boa_runtime, boa_ast, boa_parser,
boa_interner, libc, httpdate, flate2, tar, crypto-bigint, pkcs1, p256, p384,
x25519-dalek, getrandom and zeroize. httpdate parses Retry-After dates and adds no
transitive dependencies. Rustls is a direct API dependency for the constrained provider;
it was already in ureq's graph, so this adds no package versions or features. The five Boa crates execute Web scripts, implement existing
Web APIs and prepare the player through a native AST. libc supplies the child
Unix resource limit and private-cache checks; it adds no new package version.
syom uses unmodified upstream 0.10.0 candidate source at the Git revision pinned
in Cargo.toml and Cargo.lock; its former vendored copy is removed. The three
codecs still add no transitive runtime crates. The lockfile contains **265 package versions** across platforms. The exact current
graph is reported by `cargo metadata --locked`; counts are not a release invariant.
Its active graph contains no V8, deno_core, QuickJS, ring, aws-lc-sys, openssl-sys,
cc or cmake. System networking, randomness and resource limits use OS interfaces.
The fixed 2 MiB DOM bundle embeds 20 JS packages with retained notices; no JS package
manager runs during installation or downloading. Corrected Boa sources and their
provenance are under `experiments/botguard/vendor/`; bundle notices are under
`vendor/botguard/`.

ureq disables its default features and uses Rustls with certificate/hostname
verification and web PKI roots. RustCrypto source matches upstream revision
`70f76c039e587192688af18a80d5d6435dedaf22`. Since 0.6.1 the selected
`vendor/rustls-rustcrypto-current` snapshot changes only manifest publication
policy and the paste macro dependency to pastey 0.2.3; `upstream.json` records
every original file hash and tests verify Rust-source equality. Cargo.lock pins its complete
graph, including rsa 0.10.0-rc.19 and crypto-bigint 0.7.10. Both TLS 1.2 and 1.3
remain available. The application unconditionally refuses private-key loading
and offers checked X25519, P-256 and P-384 exchange. The direct crypto-bigint
constraint requires the patched >=0.7.6 family before NIST ECDH is enabled. RSA, P-256 and P-384 public certificate
signature validation remains enabled. No cryptographic primitive is implemented
in this project; ASR/media schemas and backend identities are unchanged.

The provider remains experimental and upstream warns against production use.
The [scoped comparison and update](tls-provider-scope.md) qualify controlled
private operation, not unrestricted production security approval. The crypto-bigint
advisory is removed by selecting the patched family; the RSA advisory still
appears in raw cargo audit and is not suppressed. Private RSA operations cannot
be loaded through the configured client provider. This application boundary is
not a security audit or a waiver for the dependency. The older `vendor/rustls-rustcrypto` snapshot is retained as source/license
provenance for research adapters and is not selected. The current snapshot exists
solely for the manifest patch until upstream adopts a maintained macro dependency.
Raw cargo audit still fails on RSA; paste is no longer in the selected graph.

Only ASR remains an external command adapter. gigastt's ONNX Runtime and the installed
binary's native libraries mean the entire recognition stack is not pure Rust. Current behavior supersedes archived
records that describe yt-dlp, FFmpeg or the earlier keep-all-media behavior.

## GigaSTT integration boundary

GigaSTT owns recognition, internal overlapping windows, word stitching, word
timestamps and any model-side VAD or text normalization. agent-youtube owns acquisition,
PCM preparation, durable progress, worker lifetime, evidence validation and JSON
publication. Do not add a second recognizer, automatic text correction, overlap
deduplication or VAD-based retry policy here to compensate for recognition errors.
Reproduce model errors with the exact input and GigaSTT options before assigning
them upstream; disagreement between two ASR passes is not proof of an error.

The adapter sends only unfinished PCM16 mono 16 kHz files in one
`transcribe-batch` invocation, with `--pool-size 1 --retries 0 --offline`, JSON
output and the pinned `e2e_rnnt` model configuration. GigaSTT loads one engine and
reuses its session pool. Extra punctuation and ITN passes are disabled; this does
not strip punctuation inherently emitted by the end-to-end model. Source deletion
and movement stay disabled: agent-youtube cleans owned media only after durable validated
publication. Environment settings that could silently change recognition are
rejected. Partial batch output is adopted only after worker teardown and validation;
already valid raw results are excluded from the next invocation.

External chunks (300 seconds by default) are recovery units, not an implementation
of GigaSTT's recognition windows. In the inspected 2.21.0/2.22.0 source, mono file
recognition already streams audio with bounded window memory, but accumulates its
word result until the file finishes. Batch writes the final JSON non-atomically and
does not skip an existing output or expose durable within-file resume. Keeping
bounded files therefore limits restart work and per-result size, and supplies
observable completion for agent-youtube's inactivity timeout. Removing these chunks today
would sacrifice these properties. The optional quiet-boundary setting changes only
checkpoint cuts; it is not a second VAD/recognition engine and remains opt-in.

Cuts do change recognition context. They must not be described as lossless with
respect to ASR text merely because PCM sample coverage is exact. A future upstream
interface with durable, identity-bound progress and context-preserving restart
could replace this mechanism; no such adapter is implemented speculatively.

The [historical benchmarks](benchmarks.md), [recognizer upgrade](gigastt-upgrade.md)
and [current readiness](readiness.md) retain the exact source/build/model identities
for earlier experiments. Do not describe a source inspection as binary validation.

The complete project audit (local historical record)
records subsystem coverage, corrected evidence/worker defects, dependency advisory
reachability, long-recording costs and embedding limits. Agent integrations can invoke
the supplied CLI and parse its JSON. A Rust embedding binary using native Web access
must dispatch the private worker mode because the worker starts the current executable.
Call `agent_youtube::dispatch_web_worker(std::env::args_os().skip(1))` before signals or application
output, returning immediately on `Ok(true)` and exiting nonzero on error. Only the exact
single private argument is handled; all other arguments return `Ok(false)` without side
effects. The worker owns stdin/stdout JSONL until EOF and enforces its existing resource,
protocol and lifecycle limits. See [the embedding example](../examples/embed.rs), verified
with successful local player IPC and structured failure without network access.

Offline tests use real loopback HTTP servers for Range/SABR and generated AAC/M4A fixtures.
Python 3 supplies the fixture ASR executable and child CLI wrapper; no external downloader,
Node.js, FFmpeg, model or internet connection
is required for the test suite. Tasks and verification evidence live in the local-only Backlog.md queue. The
repository follows agent-dev-kb 0.10.0 core instructions. Portable instructions
are in AGENTS.md and agents/; local tool/session configuration is not required
to build or use the CLI.

## Local qualification

`python3 tools/check_docs.py` checks local documentation targets against the Git
index, so retained ignored Backlog files cannot hide broken links. Stage intended
documents before running it. `--working-tree` also admits new nonignored files
while editing. Extracted source archives use `--source-tree`, which excludes
private-state paths and does not read a parent repository index. An unmatched
Git root is an error rather than a misleading zero-document success. No mode
runs GitHub Actions.

`python3 tools/qualify_vendors.py --upstream-root /path/to/sibling/repositories`
reads pinned syom/bandlimit commits with `git archive`, overlays the vendored
runtime source and runs library tests in an isolated directory under `runs/`.
It never modifies the source repositories. Development test dependencies are
isolated from the production graph. Missing corpora and upstream test failures
remain failures in its JSON report; this is not a complete codec certification.
Production integration/checkpoint tests, including the registry ryf package,
remain part of `cargo test`.

Production code denies unchecked unwrap/expect, unused results and unsafe code.
The existing OS calls have function-scoped unsafe exceptions. Test-only assertion
helpers are exempt from the production panic restrictions.

## Contract and local release checks

The local tests require Python 3.11+ and the pinned development-only `jsonschema`
package. It is not installed or invoked by the product. Rust tests validate actual
CLI outputs/artifacts/events against the closed Draft 2020-12 schema through
`tools/check_contract.py`; Python tests reject malformed states, unknown properties
and false readiness claims. The validator does not print instance contents.

[Local release preparation](private-release.md) uses a clean immutable Git
snapshot and includes nested forks; `cargo package` is not an equivalent bundle.
Use isolated target directories for experimental builds. Changes to production
source require new build identities and relevant checks, not inherited approval.
