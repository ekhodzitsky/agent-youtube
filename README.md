# agent-youtube

A JSON-only YouTube CLI for agents and scripts. Retrieve author-published timestamps without downloading audio, or turn Russian speech into local transcripts through GigaSTT, with checkpoints for interrupted jobs and exact recognizer output retained.

[![Source version: 0.8.2](docs/assets/version.svg)](Cargo.toml)
[![License: MIT](docs/assets/license.svg)](LICENSE)
[![CI: disabled](docs/assets/ci.svg)](.github/workflows/check.yml)
[![Minimum Rust for source builds: 1.94](docs/assets/msrv.svg)](docs/verification.md#badge-sources)

[Documentation](docs/README.md) · [JSON contract](docs/agent-contract.md) · [Readiness](docs/readiness.md)

**Experimental; no production-security approval.** YouTube can deny requests.
Recovery handles supported failures; it cannot guarantee access to a recording.

## Quick start

Use Linux x86_64 GNU, Git and Rust **1.94+**. Building needs Cargo dependencies;
chapter retrieval needs network access to YouTube. No API key, paid service,
recognizer, model files or external downloader is needed for this example.

```sh
git clone https://github.com/ekhodzitsky/agent-youtube.git
cd agent-youtube
cargo install --path . --locked
agent-youtube chapters 'https://www.youtube.com/watch?v=rfscVS0vtbw'
```

Success returns one JSON document with `ok: true`, `command: "chapters"` and exit
code 0. A local check on 2026-10-30 returned **35 timestamps**, beginning with
`0` seconds / `Introduction`, from the **16,012-second** course above. These are
published description entries, not generated summaries or proof that YouTube
renders them as chapter navigation. No matching description entries returns `status: "not_found"`;
denied or malformed metadata returns an error instead.

For **Russian transcription**, first follow the [model and runtime setup](docs/installation.md):
GigaSTT **2.22.0** and four GigaAM v3 INT8 files (**311.84 MiB**, plus runtime and
job storage). Recognition stays local; YouTube receives acquisition requests.
GigaSTT uses native ONNX components; the entire recognition stack is not Rust.

Transcription returns JSON, writes a validated `transcript.json` and retains exact
raw ASR. Failed recognition retains media for `resume`; successful publication
cleans owned temporary media. Completed text exports to JSON, Markdown, text,
SRT or VTT without downloading or recognizing again.
[Transcribe, status, resume and export](docs/usage.md).

## Comparison

For obtaining text from a recording:

| Approach | Text source | Choose when |
|---|---|---|
| **agent-youtube** | Acquired audio; local Russian GigaSTT | Persisted recovery and typed JSON are central |
| **youtube-transcript-api** | Existing manual or automatic captions | Available captions avoid local inference |
| **yt-dlp + Whisper** | Download followed by multilingual local ASR | Language/model choice matters more |

Whisper also runs locally; yt-dlp already resumes downloads. There is no matched
measurement establishing better recognition or acquisition success here.
[Comparison, reviewed sources and conditions](docs/comparison.md).

## Measured recovery

Historical Linux runs, development build **0.3.10**, GigaSTT **2.21.0**, GigaAM v3
INT8, 2026-10-28. The measured outcome is validated completion, not word accuracy.

| Workload | Decoded audio | Completed jobs |
|---|---:|---:|
| Uninterrupted short recording | 52.965 s | 1 / 1 |
| SIGKILL and caller resume; 30 s chunks | 1,053.838 s | 1 / 1 |

Both cleaned owned media after publication; the interrupted job recovered its
saved prefix. These two observations are not a representative success rate,
speed ranking or fresh 0.8.2 ASR qualification.
[Inputs, original results, methodology and reproduction](docs/benchmarks.md#job-recovery-qualification).

## Limits

Transcription supports Russian completed recordings and AAC/MP4 audio; captions,
playlists and live transcription are unsupported. Description timestamps are
language-independent; automatic chapters are not extracted. Word times are
estimates; chunk boundaries can affect recognition. Audio streams to disk without
a fixed duration cap; disk, memory and filesystem limits still apply.

New transcription jobs have a one-hour **waiting budget**, excluding active work.
Version 0.7.0 introduced separate job ownership; finish earlier unfinished jobs
with their original binary. macOS/Windows are unqualified. Coverage is unmeasured;
source badges do not claim passing hosted CI. The selected TLS provider remains
experimental. [Support and remaining risks](docs/readiness.md).

[Help](docs/usage.md#troubleshooting) · [Development](docs/development.md) · [License](LICENSE)
