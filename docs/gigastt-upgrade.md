# GigaSTT 2.22.0 upgrade qualification

On 2026-10-29 the newest stable upstream CLI release was
[GigaSTT 2.22.0](https://github.com/ekhodzitsky/gigastt/releases/tag/v2.22.0).
The generic GitHub latest-release endpoint selected `node-v2.22.0`, which contains
Node addons rather than the CLI. CLI tags and published assets were checked
separately. This qualification uses Linux x86_64 only.

The official Linux archive was installed under the ignored
`tools-local/gigastt-2.22.0/` directory. SHA-256 matched both the upstream checksum
file and GitHub asset digest. Minisign was not checked. The executable reports
`gigastt 2.22.0` and has SHA-256
`efb3c09b0ddf7872fe47bccc91f556ae728cf01035a94efe6f8e0c0795591cd1`.
[Installation instructions](installation.md) select it explicitly through a
project-local PATH entry or `--gigastt`. No global executable was replaced.

`agent-youtube doctor` passed executable, model-file and required-flag checks. A fresh
full transcription using the existing GigaAM v3 e2e_rnnt INT8 model files then
processed 847,436 mono 16 kHz frames: 52.96475 seconds. It retained one exact raw
ASR JSON chunk and published the validated canonical transcript. Manifest hashes
for both files were checked independently, and owned media/chunks were absent
after successful publication. Reopening the completed job reused the publication
without changing transcript bytes. This demonstrates integration and publication,
not measured speech accuracy or equivalence with the older recognizer.

The runtime trial used the previously qualified agent-youtube 0.3.14 binary; its exact
pre-commit producer identity is retained in the
[sanitized result record](gigastt-upgrade-results.json). The only Rust source
change in this upgrade updates the CLI help's verified recognizer version.
Local fmt, strict all-target Clippy and all 334 Rust tests passed (14 opt-in tests
ignored). Raw command outputs, transcripts and logs remain in ignored local run
storage; no GitHub Actions were run.

Model files, pipeline flags, ASR schemas, media backend and checkpoint formats
were not changed. Historical 2.21.0 measurements remain labeled 2.21.0. Existing
jobs retain their saved recognizer path and identity: updating the current PATH
does not migrate them or authorize mixing ASR identities within an unfinished
job. Retain the old executable for unfinished jobs that need it, or start a new
job with the new version. Old transcript and raw ASR files are never rewritten
as part of this update.

[Subsequent 17.56-minute hard-death and ASR restart qualification](gigastt-restart.md)
adds recovery and sampled resource evidence without changing this original short trial.
