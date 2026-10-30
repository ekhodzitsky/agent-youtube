# agent-youtube

The `agent-youtube` CLI: a JSON-only Rust tool that turns Russian YouTube recordings into local,
timestamped transcripts for agents and scripts. It downloads audio, delegates
recognition to GigaSTT, and resumes interrupted jobs from validated checkpoints.
Exact recognizer output remains available alongside compact JSON for agents.

[![Source version: 0.7.0](docs/assets/version.svg)](Cargo.toml)
[![License: MIT](docs/assets/license.svg)](LICENSE)
[![CI: manual only](docs/assets/ci.svg)](.github/workflows/check.yml)
[![Minimum Rust for source builds: 1.94](docs/assets/msrv.svg)](docs/verification.md#badge-sources)

[Readiness and support](docs/readiness.md) · [Documentation](docs/README.md) · [JSON contract](docs/agent-contract.md) · [Comparison](docs/comparison.md)

**Experimental public source; no production-security approval.** Repository,
crate and executable are named `agent-youtube`.
Downloading contacts YouTube; recognition and transcripts stay local. Source access
can fail, including because of rate limits or account requirements. Recovery is not
an access guarantee. Crates.io publication and hosted CI are disabled.

Version 0.7.0 starts an independent CLI and job-ownership namespace. Use a new
work directory and run setup for this application; unfinished jobs from earlier
binaries must be completed with their original executable.

## Measured recovery

Historical Linux qualification on **2026-10-28**, development build **0.3.10**,
GigaSTT **2.21.0**, GigaAM v3 `e2e_rnnt` INT8:

| Scenario | Decoded audio | Completed jobs | Retained raw ASR chunks |
|---|---:|---:|---:|
| Uninterrupted short recording | 52.965 s | 1 / 1 | 1 |
| Forced process death, resume (30 s chunks) | 1,053.838 s | 1 / 1 | 36 |

Both produced validated transcripts and cleaned owned temporary media. The second
run recovered a saved acquisition prefix after SIGKILL. These are two observed
outcomes, not a representative success rate, speed comparison or proof of speech
accuracy. The source badge describes today's checkout, not those historical builds.
[Inputs, build identity, checks and reproduction](docs/benchmarks.md#job-recovery-qualification).
[0.5.1 full-audio corpus and recovery acceptance](docs/acquisition-051-qualification.md):
14/15 complete downloads, including two retained-prefix recoveries; no general availability guarantee.

## Quick start

Use a Linux checkout, Rust **1.94+** for building, and **GigaSTT 2.22.0** with the
four GigaAM v3 `e2e_rnnt` INT8 model files.
[Provision the models first](docs/installation.md). The explicit `setup` command
installs the pinned recognizer locally; transcription never installs dependencies.
Historical measurements above still use 2.21.0. Network access is needed for downloading;
no paid service is required. Allow disk space for temporary audio and the models.

```sh
git clone https://github.com/ekhodzitsky/agent-youtube.git
cd agent-youtube
cargo install --path . --locked
agent-youtube setup --model-dir "$PWD/models/e2e_rnnt"
agent-youtube doctor
agent-youtube transcribe 'https://www.youtube.com/watch?v=of4B3aw7MXo' \
  --work-dir runs/example
agent-youtube export runs/example/transcript.json --format srt \
  --output runs/example/transcript.srt
```

Success returns one JSON document with `ok: true` and exit code 0. The job contains
`transcript.json`; the export creates `transcript.srt`. Progress is JSONL on stderr.
`doctor` checks prerequisites, not live access or recognition accuracy. Existing
exports are protected against overwrite. If interrupted, continue the same job:

```sh
agent-youtube status --work-dir runs/example
agent-youtube resume --work-dir runs/example
```

New jobs automatically use a one-hour recovery **waiting budget**. Download,
decode and recognition time do not consume it. Set `--retry-window-seconds 0` to
return transient failures after bounded internal recovery. Existing saved jobs
retain their original policy. [Retry semantics and 0.6.0 migration](docs/automatic-recovery.md).

Completed jobs export JSON, Markdown, text, SRT or VTT without another download or
ASR pass. Failed recognition retains media; cleanup follows durable validated
publication. [Usage and retry controls](docs/usage.md) explain automatic continuation.

## Alternatives

| Approach | Text source | Choose when |
|---|---|---|
| **agent-youtube** | Audio acquired for local Russian GigaSTT | You need integrated recovery, typed JSON and retained evidence |
| **youtube-transcript-api** | Existing manual or automatic captions | Available captions meet the task without local inference |
| **yt-dlp + Whisper** | Download followed by local multilingual ASR | Language/model choice matters more than an integrated job contract |

Whisper also supports local recognition; yt-dlp already resumes downloads.
There is no matched evidence that agent-youtube is more accurate or reliably acquires
more videos. [Verified sources and adoption differences](docs/comparison.md).

## Constraints

Russian recognition only; no caption retrieval, playlist jobs or live-stream
transcription. Word times are estimates, and external checkpoint cuts can change
recognition context. macOS Web downloading remains unverified. Test line/branch
coverage is unmeasured; processed-frame coverage does not prove complete speech
recognition. The [MIT license](LICENSE) covers project code; dependency/model terms
remain separate. The TLS provider has no
production security approval; see the [TLS hardening and remaining risks](docs/tls-hardening.md). Details and historical ASR
measurements remain in the documentation; none establishes universal accuracy.

[Troubleshooting](docs/usage.md#troubleshooting) · [Development](docs/development.md) ·
[Verification and licensing](docs/verification.md)
