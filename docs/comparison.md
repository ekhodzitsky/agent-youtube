# Comparison for agent-driven YouTube transcription

[Project overview](../README.md) · [Documentation](README.md)

Reviewed 2026-10-29 against agent-youtube 0.3.12, source baseline `eee0422`, local
contracts, and freshly opened primary upstream READMEs. This is a workflow comparison,
not a measured overall ranking. Upstream links describe the inspected projects'
documentation, not a guarantee about every version or configuration.

## Current README source recheck — 2026-10-30

Reopened the three primary upstream READMEs below for the 0.8.0 root refresh:
caption retrieval still documents manual/generated text; yt-dlp documents default
continuation of partial files/fragments; Whisper documents multilingual local
recognition and its FFmpeg prerequisite. No new matched performance, caption
retrieval or competitor inference trial is claimed. The root comparison concerns
obtaining recording text. The new local chapters command separately extracts
author-description timestamps without ASR and is not caption retrieval.

## Model selection update — 0.9.0

The current adapter supports e2e_rnnt (Russian) and ml_ctc/ml_ctc_large (Russian,
English, Kazakh, Kyrgyz, Uzbek), with no source-language identification. Required
files are variant-specific, and resume binds the selected model. The table below
retains its named 0.3.12 baseline; its Russian-only row does not describe 0.9.0.
Whisper remains relevant for additional languages and model choices.
[Current model provisioning](installation.md#multilingual-models).

## Why these alternatives

An agent needs either existing text or new recognition from audio. The two
alternatives selected for the root cover those choices: caption retrieval,
and a broad downloader combined with multilingual local ASR. The latter is
a composition, not a claim that yt-dlp itself transcribes audio.

| Criterion | agent-youtube 0.3.12 | youtube-transcript-api | yt-dlp + Whisper |
|---|---|---|---|
| Text source | Acquired audio, local GigaSTT | Existing manual/automatic captions | Downloaded audio, local Whisper |
| Language scope | Russian; no language detection | Available caption languages and translations | Multilingual models and language identification |
| Recognition setup | GigaSTT and four model files | No local ASR model for caption retrieval | Whisper model and inference dependencies |
| Recovery | Integrated persisted download/ASR/publication lifecycle | Caller handles retrieval failures | yt-dlp resumes downloads; caller coordinates recognition and durable job state |
| Reuse | Validated canonical JSON, exact raw ASR, offline exports | Caller stores fetched text and metadata | Caller chooses storage and orchestration |

Choose captions when existing subtitles meet the task: they avoid local inference
and model setup. Choose a Whisper workflow when language or model choice matters
beyond this project's selected GigaSTT models. Choose agent-youtube when
durable recognition, exact retained outputs, and typed agent responses are central.
No approach guarantees access to every YouTube URL.

## Primary sources

- [youtube-transcript-api README](https://github.com/jdepoix/youtube-transcript-api#readme):
  retrieval, manual/generated selection, languages, translation, CLI, and blocked
  requests. No fresh caption retrieval was run for this edit.
- [yt-dlp README](https://github.com/yt-dlp/yt-dlp#readme): downloader features,
  Python/CLI integration, and default continuation of partial downloads.
- [Whisper README](https://github.com/openai/whisper#readme): audio-file recognition,
  multilingual models, language identification, setup and FFmpeg requirement.
  Its published speed figures are not agent-youtube comparison results.
- Local [agent contract](agent-contract.md), [recovery guide](storage-and-recovery.md),
  [Cargo manifest](../Cargo.toml), and [CLI implementation](../src/cli.rs).

Retained research from 2026-10-24 inspected yt-dlp
`fa9886678eb65103303c627b8e013f13f8999b41`, youtube-transcript-api
`b8af68ccf219ecb451018fe41318c12d08729780`, and additional extractors, Whisper
implementations, and hosted services against agent-youtube `700fd67`. The full private
research record remains in Backlog. Its historical feature-gap observations
must not override the current job-control contract.

## Adoption and limits

agent-youtube does not accept captions as evidence of recognized audio. Its schema2
compact response is not a caption list or Whisper result: integrations must
handle structured errors, untrusted text, and evidence semantics. Select
`--response full` when the schema1 envelope is required; see the
[migration contract](agent-contract.md).

No common human-checked corpus has ranked GigaSTT against Whisper here.
Local recognition privacy is shared with local Whisper. Native downloading
without a Node.js or FFmpeg executable is a deployment property, not proof of
faster, safer, or more reliable operation. GigaSTT includes native ONNX
components; the complete recognition stack is not entirely Rust.

[Historical acquisition trials](benchmarks.md#acquisition-comparison) have
different timing scopes and valid audio representations. They establish no
speed winner. Review this comparison when scope or upstream capabilities change.
