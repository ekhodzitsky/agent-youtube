# Install

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Source-build commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Binary release

The prebuilt archive targets Linux x86_64 GNU with glibc **2.39 or newer**,
libgcc_s and libm. It is not a static or musl build. Other platforms and older
glibc systems need a source build and separate qualification. GigaSTT additionally
loads native ONNX libraries; see its [requirements](https://github.com/ekhodzitsky/gigastt).
No Rust, Cargo, Python or Git is needed to run the archive.

Download the archive, checksum file, signature and public key from the
[versioned release](https://github.com/ekhodzitsky/agent-youtube/releases/tag/v0.10.1).
For example, using curl and GnuPG as installation tools:

```sh
base=https://github.com/ekhodzitsky/agent-youtube/releases/download/v0.10.1
curl -fLO "$base/agent-youtube-0.10.1-x86_64-unknown-linux-gnu.tar.gz"
curl -fLO "$base/SHA256SUMS"
curl -fLO "$base/SHA256SUMS.asc"
curl -fLO "$base/release-signing-key.asc"
gpg --show-keys --with-fingerprint release-signing-key.asc
```

Check the key fingerprint against an independently trusted record of the
maintainer key before importing. The release uses the existing owner key:
**A230 007D A524 E68F 89D2 F829 ADD5 69E3 8E7A 4CE1**.
This page and a key downloaded from the same GitHub account are not independent
identity verification. A valid signature detects changed bytes; it does not
establish source safety, YouTube availability or production-security approval.

```sh
gpg --import release-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf agent-youtube-0.10.1-x86_64-unknown-linux-gnu.tar.gz
cd agent-youtube-0.10.1
./agent-youtube --version
./agent-youtube setup --model-variant ml_ctc
./agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --work-dir runs/first-job
```

Stop if signature or checksum verification fails. The checksum command skips
undownloaded assets but must report the downloaded archive as OK. The archive
contains LICENSE, NOTICE, third-party texts and producer.json with the exact
source revision and executable digest. Models and GigaSTT are downloaded separately
by setup. To use the command without `./`, install only the executable into a
PATH directory; keep its notices with the distribution. Runtime and job directories
are relative to your working directory unless explicitly configured.

## Build from source

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

FFmpeg and FFprobe are not required. Patched snapshots of **syom 0.9.0 source candidate** and
**bandlimit 0.1.7** are included under `vendor/`; **ryf 0.7.3** comes from
crates.io with its version and checksum locked and only SIMD enabled. Builds do
not depend on sibling checkouts; offline builds require registry packages in the
Cargo cache. All three codec runtime dependency tables are empty. Provenance and
local patches are in [vendor/README.md](../vendor/README.md).
The syom candidate is pinned to an exact upstream revision; it is not advertised
as a published 0.9.0 registry release. From source 0.15.0, its distinct v3 media
backend refuses older jobs without mutating them. Resume those jobs with their
original implementation or use a new work directory. Existing transcripts remain
readable; see the [upgrade qualification](syom-upgrade.md).

### Managed Linux setup

Run from a writable working directory. No separate recognizer or model installation is
needed for the normal online path:

```sh
agent-youtube setup
agent-youtube doctor
```

The default downloads the Russian e2e_rnnt model and pinned GigaSTT 2.22.0 Linux
x86_64 GNU archive into ignored `tools-local/runtime`. Select a multilingual model:

```sh
agent-youtube setup --runtime-dir tools-local/ml_ctc-runtime --model-variant ml_ctc
agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --runtime-dir tools-local/ml_ctc-runtime --work-dir runs/english
```

The installer uses Rust HTTPS, ranged streaming transfers and pinned SHA-256
verification. Repeat the same command to resume partial downloads. Completed
files are reused; modified files, foreign partials and incompatible prefixes are
refused. Partial data/state remain owned by the selected runtime root. Server
Retry-After guidance is persisted; rerunning setup does not clear a cooldown.
No gh, curl, GigaSTT download command or shell extractor is needed.

Online model bundles are 326,991,430 bytes (e2e_rnnt), 224,762,597 (ml_ctc) or
591,645,175 (ml_ctc_large), plus the 21,416,334-byte runtime archive. Downloads do
not load entire files into memory. Allow extra disk/RAM for extracted files,
model initialization, optimized caches and transcription jobs. GitHub hosts the
runtime/default model release; pinned Hugging Face revisions host CTC bundles.
Model/runtime licenses remain separate from this project's source license.

To use existing local models and a previously downloaded official archive:

```sh
agent-youtube setup --runtime-dir tools-local/offline-runtime \
  --model-dir "$HOME/.gigastt/models" \
  --archive /absolute/gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz --offline
agent-youtube doctor --runtime-dir tools-local/offline-runtime
```

An explicit --model-dir always selects local files; setup never downloads into
that directory. --offline prohibits network access, including archive fetching.
It can also validate/reuse an already prepared root or publish a fully downloaded,
verified owned cache file. Offline failures identify missing assets in JSON.
Online model downloads enforce the compiled upstream hashes; explicitly supplied
local model sets retain actual-byte fingerprinting for compatibility with existing
installations. A local archive must still match the compiled release hash.

Models are stored under the runtime's models directory when --model-dir is omitted.
The installer does not search or modify ~/.gigastt/models in this mode. The manifest
is published last after all checks. Existing manifests are immutable and cannot
silently switch model selection. Setup/doctor do not prove inference or YouTube
availability; those checks remain explicitly not_checked until a real job.

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

The following steps are optional manual provisioning. Normal setup fetches the
selected pinned bundle automatically.

The GigaSTT engine also supports Russian, English, Kazakh, Kyrgyz and Uzbek through
`ml_ctc` / `ml_ctc_large`; see its [model selection](https://github.com/ekhodzitsky/gigastt#choose-a-model).
Select the model explicitly with `--model-variant`; setup stores the selection
and transcribe/doctor inherit it from a prepared runtime. Resume uses the saved
job selection. Use a new runtime/work directory to change model identity.

The default is **GigaAM v3 `e2e_rnnt` INT8**. It supplies punctuation
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

## Multilingual models

Normal setup downloads the selected INT8 bundle automatically; no separately
installed GigaSTT is required. The following filenames and optional manual commands
are useful for offline provisioning or explicitly managed model directories.

| Variant | Required files | Model bytes |
| --- | --- | ---: |
| ml_ctc | multilingual_ctc.int8.onnx, multilingual_vocab.txt | 224,762,597 |
| ml_ctc_large | multilingual_large_ctc.int8.onnx, multilingual_vocab.txt | 591,645,175 |

Both support Russian, English, Kazakh, Kyrgyz and Uzbek; output has no built-in
punctuation/casing/ITN. Accuracy depends on the language and recording. Models are
provisioned from the upstream Hugging Face repositories and verified against the
[pinned GigaSTT model hashes](https://github.com/ekhodzitsky/gigastt/blob/v2.22.0/crates/gigastt-core/src/model/variant.rs).
Disk size is not a RAM requirement. Allow runtime, inference and job storage.

```sh
agent-youtube setup --runtime-dir tools-local/ml_ctc-runtime --model-variant ml_ctc
agent-youtube doctor --runtime-dir tools-local/ml_ctc-runtime
agent-youtube transcribe 'https://www.youtube.com/watch?v=jNQXAC9IVRw' \
  --runtime-dir tools-local/ml_ctc-runtime --work-dir runs/english
```

Use ml_ctc_large and another runtime directory for the larger model. To prepare
models manually with an existing GigaSTT 2.22.0 executable:

```sh
./tools-local/gigastt-2.22.0/gigastt download --model-variant ml_ctc \
  --model-dir models/ml_ctc --skip-diarization
python3 tools/check_models.py --model-dir models/ml_ctc --model-variant ml_ctc
agent-youtube setup --runtime-dir tools-local/local-model-runtime \
  --model-dir "$PWD/models/ml_ctc" --model-variant ml_ctc
```

With explicit --gigastt, pass --model-dir and --model-variant to transcribe/doctor.
A variant conflicting with a prepared runtime is rejected. Repeated setup uses
the same model selection flags as the original invocation; model changes need a
new runtime root. The executable remains pinned to 2.22.0.

JSON uses recognizer_language=mul for either multilingual model and ru for the
Russian model. This is configured model scope, not detected speech language;
source_language remains null. Old jobs and runtime manifests without a variant
retain e2e_rnnt. Existing raw ASR JSON and transcripts are never rewritten.
Rust Options literals now require model_variant (ModelVariant::default() preserves
the original model). Job/checkpoint schema versions remain unchanged; the new
selection field defaults when reading older state. Older binaries may reject new
state, so keep the producing binary for unfinished jobs.
