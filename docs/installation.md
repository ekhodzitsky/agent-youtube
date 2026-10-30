# Install

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Install

Build with Rust **1.94+**. The current quick start uses **GigaSTT 2.22.0**
and the GigaAM v3 e2e_rnnt INT8 models. Historical 2.21.0 measurements retain
their original version and scope.
HTTP/TLS and media processing use Rust. Web attestation and player functions run
in a private subprocess of the same Rust executable; yt-dlp, Node.js, Deno, a browser,
FFmpeg and FFprobe are not required. The current Web downloader is verified on Linux.
Earlier Unix process-group checks also covered macOS; the new Web worker has not yet
been verified there.

```sh
cargo install --path . --locked
```

FFmpeg and FFprobe are not required. Patched snapshots of **syom 0.7.1** and
**bandlimit 0.1.7** are included under `vendor/`; **ryf 0.7.3** comes from
crates.io with its version and checksum locked and only SIMD enabled. Builds do
not depend on sibling checkouts; offline builds require registry packages in the
Cargo cache. All three codec runtime dependency tables are empty. Provenance and
local patches are in [vendor/README.md](../vendor/README.md).

### Managed Linux setup

Provision the four model files into `models/e2e_rnnt` using the verified sequence
below, then run from the project directory:

```sh
agent-youtube setup --model-dir "$PWD/models/e2e_rnnt"
agent-youtube doctor
```

`setup` downloads the pinned GigaSTT 2.22.0 Linux x86_64 GNU archive over verified
HTTPS, checks compiled archive/executable SHA-256 pins, retains its notices and
publishes a manifest under ignored `tools-local/runtime`. It uses Rust TLS,
gzip and tar readers; no `gh`, `curl`, shell extractor or PATH modification is
needed. Model files are checked and fingerprinted, not downloaded or replaced.
The manifest records their actual content; repeat setup and managed selection
reject changed model or executable bytes. `setup` does not load models or prove
inference correctness; its JSON states those checks as `not_checked`.

For an explicit location or an already downloaded official archive:

```sh
agent-youtube setup --runtime-dir tools-local/runtime --model-dir "$HOME/.gigastt/models" \
  --archive /absolute/gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz
agent-youtube doctor --runtime-dir tools-local/runtime
```

New `doctor`/`transcribe` invocations select a valid prepared runtime automatically
when the default directory exists. An incomplete or corrupt directory fails with
`runtime_invalid`; it never silently falls back to PATH. Without that directory,
legacy PATH selection remains available. Use an absolute `--runtime-dir` from
other working directories, or explicit `--gigastt`/`--model-dir` to bypass discovery.
`--gigastt` and `--runtime-dir` are mutually exclusive. Resume always uses saved
job options. Existing manifests are immutable; prepare a new directory to change
a runtime's pinned configuration. No automatic upgrade or global install occurs.

The [runtime contract and verification](runtime-setup.md) describe interrupted
installation, integrity failures and platform limits. Other platforms use an
explicit externally provisioned recognizer as below.

### Manual or external recognizer

Install GigaSTT separately; do not assume an old executable on PATH is compatible.
The newest stable CLI release checked on 2026-10-29 is
[2.22.0](https://github.com/ekhodzitsky/gigastt/releases/tag/v2.22.0).
GitHub's generic latest-release endpoint may select a Node addon or model release;
use the CLI tag explicitly. Version 2.0.12 lacks the required JSON batch interface.

Install the Linux x86_64 archive locally to this project:

```sh
mkdir -p tools-local/gigastt-2.22.0
cd tools-local/gigastt-2.22.0
gh release download v2.22.0 --repo ekhodzitsky/gigastt \
  --pattern 'gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz' \
  --pattern 'gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz.sha256'
sha256sum -c gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz
cd ../..
export PATH="$PWD/tools-local/gigastt-2.22.0:$PATH"
gigastt --version
```

On Apple Silicon use the corresponding `aarch64-apple-darwin` archive and
`shasum -a 256 -c`; that platform is outside the new Linux qualification.
Checksums detect corruption; upstream also publishes minisign signatures for
independent authentication. This installation checked SHA-256 against both the
release checksum file and GitHub asset digest; it did not verify minisign.
Never commit `tools-local/` or model binaries. The PATH selection applies to the
current shell; repeat it in later sessions, or pass
`--gigastt "$PWD/tools-local/gigastt-2.22.0/gigastt"` explicitly to `doctor` and
`transcribe`. Existing jobs retain their saved recognizer selection.

The Linux archive SHA-256 is
`cd790191a600df48aa5c56e124953322af89df4bbd60d2c980436b0597c46194`.
The [upgrade qualification](gigastt-upgrade.md) records doctor, full live
recognition and output verification with the existing models. No global
installation, model replacement or old transcript rewrite is required.

### Models

This version selects **GigaAM v3 `e2e_rnnt` INT8** explicitly. It supplies punctuation
and casing without a separate punctuation model. Four nonempty readable files must
exist in `~/.gigastt/models` or your `--model-dir`:

```text
v3_e2e_rnnt_encoder_int8.onnx
v3_e2e_rnnt_decoder.onnx
v3_e2e_rnnt_joint.onnx
v3_e2e_rnnt_vocab.txt
```

The official GitHub model release is available independently of Hugging Face:

```sh
mkdir -p models/e2e_rnnt
gh release download models-v3-2026-10-09 --repo ekhodzitsky/gigastt \
  --pattern 'v3_e2e_rnnt_encoder_int8.onnx' \
  --pattern 'v3_e2e_rnnt_decoder.onnx' \
  --pattern 'v3_e2e_rnnt_joint.onnx' \
  --pattern 'v3_e2e_rnnt_vocab.txt' \
  --pattern 'SHA256SUMS.txt' --dir models/e2e_rnnt
python3 tools/check_models.py --model-dir models/e2e_rnnt
agent-youtube setup --model-dir "$PWD/models/e2e_rnnt"
agent-youtube doctor
agent-youtube transcribe 'https://www.youtube.com/watch?v=of4B3aw7MXo' \
  --work-dir runs/first-job
```

The documented release contains 326,991,430 bytes (311.84 MiB) across the four
model files. Allow additional space for the recognizer, caches and job media. Respect the upstream GigaAM/model license terms;
they are external assets. The pipeline passes `--offline`, disables extra punctuation
and ITN postprocessors and never silently downloads models. `doctor` checks filenames,
readability and CLI flags; an actual recognition run proves ONNX/runtime compatibility.
Each run records SHA-256 of all four model files and the selected gigastt executable.

The provisioning commands require the optional GitHub CLI and Python 3.11+
for the read-only checksum helper. They are not needed by the running product.
The download command uses the optional GitHub CLI only to provision model assets.
It is not a product/runtime dependency. The checker verifies the four exact
release hashes offline and does not load or modify models. Use a new model/runtime
directory for a different set; never overwrite models used by unfinished jobs.
The public release encoder SHA-256 is `16805903b102d8b27bb0253de044f25e33a8a5bf07fce0c4da4ac1a5a7544091`.
Older measurements used encoder `cf51b300...`; those are distinct model identities.
[Current first-run qualification and support](readiness.md) separates their evidence.
