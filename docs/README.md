# agent-youtube documentation

[Project overview](../README.md)

The root README follows root README standard v1. Detailed content from the
previous root README has been retained below. Commands assume the repository
root. Public source excludes local operational data and the private tracker.

The pipeline is owned Rust downloading (AAC/M4A only), syom, bandlimit, ryf,
then GigaSTT. Text is a transcription, never a summary. Audio and transcripts
stay local; YouTube receives the video/client and attestation requests needed
to obtain its stream. Media, models, logs, and generated transcripts stay out
of Git. See the installation guide for vendored codec provenance.

| Guide | Contents |
|---|---|
| [Installation](installation.md) | Rust, GigaSTT, model files, and runtime prerequisites |
| [Usage](usage.md) | Transcribe/export, accepted URLs, job lifecycle, and waiting budgets |
| [Agent contract](agent-contract.md) | JSON schemas, structured errors, evidence, and progress |
| [Storage and recovery](storage-and-recovery.md) | Acquisition, validation, owned artifacts, restart, and cleanup |
| [Timestamps and quality](timestamps-and-quality.md) | Timing semantics, reference evaluation, and historical verification |
| [Comparison](comparison.md) | Alternatives, primary sources, and adoption trade-offs |
| [Benchmarks](benchmarks.md) | Historical results, configurations, reproduction, and limitations |
| [Development](development.md) | Local checks, dependencies, security findings, and embedding |
| [Verification](verification.md) | Evidence map, badge meaning, licensing, and remaining gaps |

## Current release and operation

- [Published timestamps without audio](usage.md#published-timestamps-without-audio):
  chapters command, author-description provenance, absence/errors and limits.

- [Independent source migration audit](migration-audit.md): identity, storage,
  machine-contract, Git and privacy boundaries for 0.7.0.

- [Readiness and support — 0.8.2](readiness.md): current disposition, evidence map,
  security/license limits, operational checks and rollback.
- [Local release preparation](private-release.md): complete source/binary
  artifacts and identity/notices manifests without uploading; crate publication is disabled.
- [Automatic recovery](automatic-recovery.md): current waiting-budget and legacy
  migration contract, introduced in 0.6.0.
- [Managed runtime](runtime-setup.md): setup trust model and saved selection rules.

## Dated qualification archive

Each report retains its original version and measurements. Historical artifacts
and private raw logs are local-only; a clean clone has sanitized numerical records.
Use the current readiness page for today's disposition. Project labels in these
summaries are normalized or neutral; they do not claim earlier builds bore the
current public identity. Original records remain local and are not rewritten.

| Report | Original scope |
| --- | --- |
| [Release readiness](release-readiness.md) | 0.3.13 private candidate |
| [Distribution audit](distribution-readiness.md) | 2026-10-29 history/notices snapshot |
| [TLS reachability](tls-security-review.md) | Pre-0.5.0 provider review |
| [Acquisition cohort](acquisition-qualification.md) | 0.3.13, 15 selected sources |
| [Artifact restoration](release-artifact-restoration.md) | 0.3.14 and isolated TLS prototype |
| [GigaSTT upgrade](gigastt-upgrade.md) | 2.22.0 short integration trial |
| [GigaSTT restart](gigastt-restart.md) | 17.56-minute, 30-second checkpoints |
| [TLS provider comparison](tls-provider-scope.md) | 0.5.0 functional comparison |
| [TLS risk assessment](tls-risk-assessment.md) | Defects found in 0.5.0 |
| [TLS hardening](tls-hardening.md) | Defect remediation in 0.5.1 |
| [Multi-hour ASR](multi-hour-defaults.md) | Historical 3.82-hour default-settings restart |
| [Full-audio acceptance](acquisition-051-qualification.md) | 0.5.1 corpus and controlled recovery |
