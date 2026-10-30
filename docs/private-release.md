# Local release preparation

The supported source artifact is a reviewed Git snapshot including the required
nested dependency forks. `cargo package` omits those nested crates and is not the
source distribution route. Public source is hosted as agent-youtube. Crate
publication is disabled; this procedure prepares local artifacts only.
There is no release upload or GitHub Actions trigger in this procedure.

Run [local checks](development.md) first, review the staged paths for private data,
commit the completed changes to main, then build the committed executable:

```sh
cargo build --release --locked
target/release/agent-youtube --version
python3 tools/prepare_release.py --output runs/releases/0.7.0 \
  --binary target/release/agent-youtube
```

The preparer refuses an uncommitted checkout, missing required fork/license files,
linked/unsafe archive entries or private-state names. Output must be a new real
owned directory under ignored runs/. The executable must identify the exact clean
HEAD commit and package version; its running-image hash must match copied bytes.
It records SHA-256 for every artifact in release.json. It performs no network
upload and adds no runtime/model weights. Existing output is never overwritten.

The source tar.gz uses git archive from that immutable commit and a deterministic
gzip header. It includes local codec/Boa/provider source, root LICENSE/NOTICE and
vendored notices; it contains no .git history, task tracker, media or local models.
The adjacent third-party notice collection covers the exact current locked graph.
License-text collection does not approve every redistribution scenario. Do not
upload artifacts without an explicit owner request. The manifest field
`private: true` means this prepared bundle is local-only, not that Git source is private.

Verify an extracted source candidate in a separate owned directory with its own
Cargo target. Provision the development Python requirement, then run fmt, strict
Clippy, cargo test and the Python qualification suite. Use
`python3 tools/check_docs.py --source-tree` for documentation links in an extracted
archive; the ordinary Git-index mode refuses mismatched or absent repository roots. Registry/Git dependencies
must be available or downloadable; offline builds require their Cargo cache.
The source archive excludes Git history, so a standalone build may report unavailable
revision/dirty metadata. Compare the production source fingerprint and schema/model
compatibility, not just the version string. A new build is a different executable.

An annotated version tag designates a source commit;
its message is an artifact designation, not a security approval or availability SLA.
The local release.json supplies actual source/binary digests. Do not move an existing
version tag; use a new patch version after further changes. Crates.io publication
and release artifact uploads are not enabled. Pushing a
source tag is separate from uploading a release bundle and does not run the
manual-only workflow; hosted Actions remain disabled.

Keep prior bundles and the matching GigaSTT/model files for rollback. Never downgrade
an unfinished job to a binary that rejects its retry/checkpoint/media-backend format.
The [current support matrix](readiness.md) defines security, platform and source
limits; a successful package build does not remove them.

## Independent public history

The public main branch begins at the reviewed 0.7.0 source root. Earlier public
tags are removed; original history and mappings remain in private local backups.
Do not move published version tags silently. New changes need new version tags.
Historical numerical reports retain their original values and hashes; identity
labels are sanitized summaries, not byte-identical exports or new measurements.
Original recognizer JSON and canonical transcripts remain unchanged and local.
Git ref replacement cannot promise erasure of previously public provider caches
or copies held by other people. No original private Git object is published.
