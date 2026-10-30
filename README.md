# agent-youtube

YouTube transcripts for AI agents.

[![Source version: 0.10.1](docs/assets/version.svg)](Cargo.toml)
[![License: MIT](docs/assets/license.svg)](LICENSE)
[![CI: disabled](docs/assets/ci.svg)](.github/workflows/check.yml)
[![Minimum Rust for source builds: 1.94](docs/assets/msrv.svg)](docs/verification.md#badge-sources)

Give your agent the text of a lecture, interview or podcast to summarize,
search or analyze. This CLI downloads the audio, transcribes it **locally with
GigaSTT**, and returns JSON. Interrupted jobs can resume. Your agent handles
summarization and analysis.

Choose Russian recognition or a multilingual model for English, Russian,
Kazakh, Kyrgyz and Uzbek. [Models and setup](#language-support).

## Examples

**Turn a video into text.** After the [one-time setup](#install):

```sh
agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --work-dir runs/first-job
```

On success, `result.transcript` contains the text passages and estimated timestamps.
The transcript is saved to `runs/first-job/transcript.json`; exact
recognizer output is retained alongside it. Hand the text to your agent, or export it:

```sh
agent-youtube export runs/first-job/transcript.json --format md --output transcript.md
```

**Continue interrupted work.** Inspect the job, then resume when idle and resumable:

```sh
agent-youtube status --work-dir runs/first-job
agent-youtube resume --work-dir runs/first-job
```

**Get the author's chapters.** No speech model or audio download needed:

```sh
agent-youtube chapters 'https://www.youtube.com/watch?v=rfscVS0vtbw'
```

A run on 2026-10-30 returned 35 timestamps. First `result.chapters` entry:

```json
{"start_seconds":0,"title":"Introduction"}
```

[Commands and job lifecycle](docs/usage.md) | [JSON contract](docs/agent-contract.md)

## Built for agents

- **Structured tool calls.** One JSON result on stdout, JSON progress on stderr,
  error codes and retry guidance. No terminal text to scrape.
- **Work survives interruptions.** Download and recognition checkpoints preserve
  completed work. Temporary acquisition failures retry within a saved waiting
  budget; a stopped process continues when your agent calls `resume`.
- **Local transcription.** Audio is recognized on your machine. YouTube receives
  acquisition requests; audio is not uploaded to a transcription service.
- **Long recordings.** Audio streams to disk and is processed in chunks, without
  a fixed duration cap or loading the entire recording into RAM.

The downloader and media pipeline run in Rust; no yt-dlp, FFmpeg, Node.js or browser
is required. GigaSTT performs recognition and includes native ONNX components.
Export JSON, Markdown, plain text, SRT or VTT from saved transcripts without
another download or recognition run.

## Install

Linux x86_64 GNU with **glibc 2.39+**. No Rust toolchain is needed for the
binary release. Download and verify the [signed release](docs/installation.md#binary-release),
then extract it:

```sh
tar -xzf agent-youtube-0.10.1-x86_64-unknown-linux-gnu.tar.gz
cd agent-youtube-0.10.1
./agent-youtube setup --model-variant ml_ctc
./agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --work-dir runs/first-job
```

[Download Linux binary](https://github.com/ekhodzitsky/agent-youtube/releases/tag/v0.10.1).
Optionally install the executable into a directory on PATH for the examples above.
Setup downloads about **234.8 MiB** of pinned model/runtime files. Allow extra disk
space for extraction, caches and jobs. Recognition is local; no paid transcription
service or user API key is required. [Build from source](docs/installation.md#build-from-source)
with Rust 1.94+ if your system does not match the binary requirements.

Setup downloads and verifies the selected model and GigaSTT automatically.
Repeat the same setup command after an interruption; verified files are reused.
Setup returns `state: "ready"` and `next_action: "transcribe"`. Transcription returns
`ok: true`, text passages and saved artifact paths on success.
[Local files, offline setup and other models](docs/installation.md).

## Compared with

| Tool | Text comes from | Choose it for |
|---|---|---|
| **agent-youtube** | Audio transcribed locally with GigaSTT | JSON job control, checkpoints and saved transcript artifacts |
| **youtube-transcript-api** | Existing manual or automatic captions | Text retrieval without speech recognition |
| **yt-dlp + Whisper** | Audio transcribed locally with multilingual ASR | Additional languages and model choices |

Whisper also runs locally; yt-dlp already resumes downloads.
[Comparison and sources](docs/comparison.md). No measured claim of superior
recognition accuracy or YouTube access.

## Recovery evidence

Linux, **0.9.1 development build**, GigaSTT **2.22.0**, 2026-10-30.
Full YouTube audio acquisition, decoding and real recognition:

| Recording / scenario | Decoded audio | Completed jobs |
|---|---:|---:|
| English / resume after HTTP-body timeout | 19.064 s | 1 / 1 |
| Russian / fresh job | 52.965 s | 1 / 1 |

Both published validated transcripts with contiguous input coverage. The failed
English job continued after correcting an overly short response-body deadline.
These checks measure completion, not word accuracy or universal YouTube access.
[Method and reference comparison](docs/verification.md#http-body-timeout-qualification--091-2026-10-30).
[Earlier recovery results](docs/benchmarks.md#job-recovery-qualification).

## Language support

[GigaSTT](https://github.com/ekhodzitsky/gigastt#choose-a-model) supports Russian,
English, Kazakh, Kyrgyz and Uzbek through `ml_ctc` / `ml_ctc_large`.
Select `--model-variant ml_ctc` or `--model-variant ml_ctc_large` during
[setup or transcription](docs/installation.md#multilingual-models).
The default `e2e_rnnt` recognizes Russian with punctuation and casing; CTC models
return bare multilingual text. The source language is not detected.
Author-description chapters work across languages.

## Scope

**Experimental.** Completed recordings, AAC/MP4 audio.
No caption retrieval, playlists, live transcription or automatic chapters.
YouTube can refuse access. Disk and memory limits still apply; timestamps and recognition can be wrong.

The TLS provider remains experimental; unrestricted production-security approval
is outstanding. [Support, security limits and migration](docs/readiness.md).

[Documentation](docs/README.md) | [Help](docs/usage.md#troubleshooting) ·
[Development](docs/development.md) | [Changelog](CHANGELOG.md) | [MIT license](LICENSE)
