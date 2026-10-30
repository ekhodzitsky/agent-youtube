# Local full-audio qualification

Run from the repository root; keep qualification outputs local. The harness uses Python's standard library only;
Python, yt-dlp, Node and FFmpeg are research tools, never runtime dependencies of
agent-youtube. Supply installed executable paths explicitly (for a non-executable Python
zipapp, also supply `--yt-dlp-python /absolute/path/to/python3`); nothing is downloaded or
installed by the harness. Build checks run locally; no GitHub operation is used.

For an explicitly fault-injected SABR trial, the ignored
`research_production_client` hook accepts `AGENT_YOUTUBE_RESEARCH_INJECT_SABR_DNS=1`.
It synthesizes one initial media DNS failure after the normal gate checks, then
uses the production fallback path. Verify `metrics.injected_dns_failures == 1`
and complete size/hash/decode evidence; a successful acquisition without the
injection does not qualify failover. Request-attempt counters include the
synthetic failure. The hook and injection are absent from production builds;
keep fault-injected trials separate from ordinary reliability measurements.
`AGENT_YOUTUBE_RESEARCH_DNS_AFTER_BYTES` optionally delays the injection until the fresh
trial has committed that many bytes. Verify the recorded pre-failure prefix
SHA-256 against the final file as well as full decode/length evidence. These
prefix fields describe that process, not an aggregate across process restarts.

The separate stall runner requires a frozen release CLI binary and a release lib-test
executable containing the ignored production hooks:

```sh
python3 experiments/qualification/stall.py \
  --output runs/stall-UNIQUE \
  --binary /absolute/path/to/agent-youtube \
  --probe /absolute/path/to/lib-test-executable
```

It runs three videos with a transient injected stall and one persistent-stall/new-process
resume trial. `--videos`, `--persistent-video` and `--after-bytes` select the finite
corpus and committed-byte threshold. The hook uses `AGENT_YOUTUBE_RESEARCH_STALL=renew|persistent`
and `AGENT_YOUTUBE_RESEARCH_STALL_AFTER_BYTES`. Injection returns successful SABR cycles with
no new bytes **before media dispatch**; it does not manufacture server responses,
shorten real cooldowns or discard existing media. Synthetic cycles are counted separately
from HTTP requests. `metrics.stall` records the pre-stall prefix/representation and
successful stalled-source renewals; the transient injection stops only after that
production renewal succeeds. The persistent mode continues until bounded exhaustion.
All injection and counters are compiled out of the normal CLI.

A passing trial requires exactly one renewal, the bounded cycle count, unchanged
representation and prefix, and full source/PCM verification. A direct source or a video
that completes before injection is **not** a passing stall trial. Persistent trials
require retryable `download_no_progress`, an unchanged partial file and successful
completion by a separate process with injection disabled; no receipt/lease edits are
made. Preserve the JSON report and failed trials. These controlled experiments do not
measure natural YouTube failure rates or guarantee future availability.

Access recovery and the actual CLI/recognizer lifecycle have separate runners:

```sh
python3 experiments/qualification/access.py \
  --output runs/access-UNIQUE --binary /absolute/path/to/agent-youtube \
  --probe /absolute/path/to/lib-test-executable
python3 experiments/qualification/cli_resume.py \
  --output runs/cli-resume-UNIQUE --binary /absolute/path/to/agent-youtube
```

Run them sequentially and honor the shared cooldown. `access.py` exercises a synthetic
SABR 403 failure, expired source deadline, expired caller-side token deadline, and
incompatible track/representation candidates after a nonempty committed prefix.
These opt-in lib-test hooks use `AGENT_YOUTUBE_RESEARCH_ACCESS_FAULT` and
`AGENT_YOUTUBE_RESEARCH_FAULT_AFTER_BYTES`; all are compiled out of the CLI. Candidate mutation
occurs before compatibility checks; 403 injection uses the same typed failure as the
SABR HTTP parser. Neither claims that YouTube sent those responses. Token expiry calls
the real token minting path, while expiration of the worker's cached minter is tested
separately offline. Completion requires measured recovery, exact prefix/identity,
and full decode. Identity faults require a nonretryable refusal with unchanged media,
then completion in a new process with the synthetic fault removed. No receipt/lease
edits are used. Local HTTP tests separately verify that actual 429/Retry-After responses
persist a gate that prevents a second process from making early requests.

`cli_resume.py` uses the normal CLI and installed GigaSTT/models. It cancels a real
acquisition after durable bytes, verifies retained media, resumes in another process,
checks the prefix before cleanup, then validates completed status, raw-ASR and transcript
hashes, cleanup and unchanged completed reuse. Every command must emit one JSON value
and JSONL diagnostics. This is explicit cancellation, not a claim of a naturally
occurring network failure. Canonical raw ASR and transcripts remain unmodified in the
owned job; only diagnostic captures are redacted. Supply `--video` for another recording.

```sh
python3 -m unittest discover -s experiments/qualification -v
python3 experiments/qualification/qualify.py \
  --output runs/qualification-UNIQUE \
  --yt-dlp /absolute/path/to/yt-dlp \
  --ffmpeg /absolute/path/to/ffmpeg \
  --node /absolute/path/to/node \
  --video of4B3aw7MXo --video 2teKcIDANI4 --repetitions 2
```

Output must be a new directory under `runs/`. All media and evidence are retained;
there is no ASR publication to authorize automatic media cleanup. Executable hashes,
Cargo.lock, Rust version, source base/patch hash, configuration and randomized trial
order are recorded. Trials for each video/repetition stay adjacent, with randomized
tool order. The Rust release binary and its ignored acquisition test hook
are built once, then reused for trials. JSON results go to stdout; progress goes to
stderr. Exit 1 means setup or qualification failed. Missing/invalid probe output
counts as failure even if its process exits 0. A rate-limit response stops subsequent
network trials; skipped trials have separate counts and are not network failures.
Rerunning requires a new output directory and honoring the server cooldown.

A native success requires matching video/receipt identity, exact declared source
length and whole-file SHA-256, complete native decoding, and a fully read mono
16 kHz PCM payload whose duration matches metadata within two seconds. yt-dlp
success requires the requested video, declared source size, full FFmpeg decoding
with errors fatal, and the same PCM length checks. Metadata duration is an estimate;
these checks prove transport/container completeness within this scope, not that the
publisher's recording contains every intended spoken word. No listening/ASR accuracy
claim follows from them.

To exercise a real native interruption and a new process resuming the committed
prefix, use a sufficiently long video and `--native-resume-after-bytes 1048576`.
The initial process must leave a verified nonempty partial prefix; the resumed file
must retain its exact hash and pass full decoding. A download that finishes before
cancellation is recorded as `interruption_not_exercised`, not a recovery success.
This mode still runs fresh yt-dlp trials: it does not compare recovery latency.
Source representation changes, HTTP 403/429, corrupt evidence and cleanup ordering
are deterministically covered by the Rust test suite; live fault injection is not
performed against YouTube.

Native wall time includes acquisition and native decoding; the reference records
acquisition and decode separately. Formats may differ, so byte equality and naive
wall-time rankings are inappropriate. No account cookies, proxy rotation or paid
services are configured. Inherited proxy environment
presence is recorded without values. A tiny same-host corpus establishes no SLO or
unconditional availability. Future trials must include representative long recordings,
repeat days/networks, and report all causes of failure. Keep the reports private.

For a controlled automatic PO-provider reference, start an inspected bgutil HTTP
server separately on loopback and add:

```sh
--plugin-dir /absolute/path/to/bgutil-checkout \
--po-provider-url http://127.0.0.1:14416 \
--reference-client mweb --native-client mweb
```

The harness never starts or installs a server. Default yt-dlp plugin directories
are disabled; only the supplied checkout is enabled. Its Git revision/dirty state
are recorded. Explicit provider URLs must be loopback HTTP origins with a port and
no credentials. Use matching inspected server/plugin versions and preserve the
server build/dependency hashes alongside the report when supplying an existing
build. The external provider owns its HTTP retry policy: ensure it respects server
Retry-After intervals and stop it after the experiment. The controlled research
driver used for the historical paired comparison instrumented provider Axios
backoff without changing token logic. That local driver is not a product dependency.
Reference extractor/media retries are zero, so the harness can stop on observed
cooldowns; a provider can still have its own outstanding requests.

Verbose reference logs distinguish `provider_loaded`, `provider_token_requested`
and `provider_token_received`. A loaded provider or successful independent issuance
alone never proves that a particular download used a token. A bot rejection before
the GVS stage may legitimately show no token request. PO namespaces in log messages
are preserved while URLs, tokens and credential headers are redacted. Provider
content-binding values, including plain `Generating POT for` messages, are also
redacted. Selected reference language/format notes are recorded when available. Automatic native client
selection remains the default; pin `mweb` only for explicit context comparisons.

## Broader multi-date corpus

`corpus.json` pins15 video IDs and selection provenance. Candidate tags are not
verified ages or track counts; keep unavailable items and failures in the report.
`corpus.py` validates the manifest and aggregates actual completed/attempted calendar
series, refusing mixed corpus/tool hashes. One date repeated three times cannot
qualify three-day coverage. Failed, skipped and unmeasured outcomes remain distinct.

`series.py --config PRIVATE_CONFIG.json` executes a finite local campaign with frozen
native binary/probe, yt-dlp, Node, FFmpeg and harness artifacts. Configuration lists
the exact artifact paths/hashes, manifest/hash, repository/output root under runs/,
allowed dates, timezone, seed, timeout/budget and resume-video IDs. It does not build,
install tools, start a PO provider or enable hosted CI. The current campaign uses
native auto and configured yt-dlp VISIONOS without account cookies or external PO.
This is an explicit route comparison, not a claim about every yt-dlp configuration.
Previously matched MWEB+provider trials remain separate historical evidence;
they are not pooled into a new campaign.

One file lock prevents overlapping series. An existing date directory is preserved,
not silently rerun. Each pair is adjacent in a seeded schedule; a selected long
native case is interrupted and resumed in a new process while reference stays fresh.
Those scenarios and acquisition/decode timings are separate; do not infer a speed
winner. Reports/progress are saved after every trial, including failures and skips.
A cooldown or observed reference503 conservatively pauses remaining trials; pause.json
requires review before future dates. Future local timers must remain finite and
must not remove this pause automatically. Host sleep/offline timers cannot fabricate
observations for a missed date. No listening or ASR accuracy claim follows.

Test-only Rust counters measure downloader HTTP attempts (excluding interpreter-worker
HTTP) and verified replayed audio bytes. Requests are classified as watch, player,
script, attestation, media or other without retaining URLs or headers. Resume and
corpus totals expose categories only when every contributing measurement has the
complete fixed set with a matching total; older or inconsistent evidence stays null.
They do not measure partial failed bodies,
TCP retransmissions or all wire traffic. The reference counters and media_wire_bytes
remain null where uninstrumented, never zero inferred from final file size. Frozen
copies prevent later source edits from changing scheduled observations. Each campaign must retain its own artifact provenance, finite schedule, actual
counts and pending coverage in its local configuration and report.


## Independent speech audit (research only)

`speech_audit.py` compares an explicit mono PCM16/16k WAV and gigastt raw JSON
against independent Silero v5.1.2 frame probabilities. It does not use ASR words
to decide whether the audio contains speech. It binds both inputs and the model
by SHA256, verifies duration/alignment, and preserves all original evidence.
The pairing is explicit operator input, not independent authentication that the
transcript belongs to the same video. Only RIFF WAV is supported by this probe;
its ASR JSON input is limited to 16 MiB. These are research-tool limits, not new
production duration limits.

```sh
uv venv runs/speech-audit-env
uv pip install --python runs/speech-audit-env/bin/python -r experiments/qualification/speech-audit-requirements.txt
runs/speech-audit-env/bin/python experiments/qualification/speech_audit.py AUDIO.wav RAW.json /path/to/silero_vad.onnx runs/new-audit --gigastt /path/to/gigastt
python3 -m unittest discover -s experiments/qualification -p test_speech_audit.py
```

Use an existing model with SHA256
`2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f`.
The probe never downloads models. Its isolated environment adds nine Python
packages, including NumPy and the native ONNX Runtime CPU wheel (about 32.5 MiB
combined compressed for those two wheels on the tested platform). This is not a
production dependency; adding a second inference runtime to agent-youtube is not yet
justified. Python 3.13.15 was tested. Installing the research requirements requires
network access; inference and retries run locally/offline.

Inference carries recurrent state and 64 context samples between 512-sample
frames, following the [pinned upstream wrapper](https://github.com/snakers4/silero-vad/blob/v5.1.2/src/silero_vad/utils_vad.py).
`--context 0` deliberately probes the alternative seen in local gigastt source;
it is not the canonical wrapper. The probe uses its own simple positive-run
segmentation, not Silero's full hysteresis/padding algorithm. `frames.jsonl`
contains every probability; `regions.jsonl` contains uninterrupted positive runs
of at least 250 ms. Audio is streamed and these outputs grow on disk, not in RAM.

Candidates require at least a one-second gap after a 200-ms word margin and at
least 500 ms of positive VAD evidence. Gaps are split into at most 20-second
windows. At most the first five candidates are retried with three seconds of
context per side. Exact retry JSON, WAV and original-timeline offsets remain in
separate directories, even on failure. Existing output directories are refused.
Nothing replaces the canonical transcript or authorizes media cleanup.

**These thresholds are not a calibrated omission detector.** Real ASR timestamp
errors produce candidates even where the words are already in the transcript.
On the supplied 17:34 recording, 29 candidates were found; the first five retries
largely reproduced existing text with variations. A deliberately removed 30–40s
word span was found and recovered into a separate result; ten seconds of digital
silence produced no candidates. Neither result establishes precision, recall,
semantic accuracy or completeness on general recordings. See local Backlog
research evidence for hashes and exact trial paths. Production integration needs
an independent VAD region interface, calibrated alignment tolerance, a labeled
speech/music/noise corpus and a safe policy for presenting conflicting retries.

## Publication provenance cleanup

Private tracker references were removed from three corpus provenance strings.
The frozen video IDs, ordering, labels, selection tags and policy are unchanged.
The manifest file digest changes with those descriptive fields; historical
reports retain the original digest and are not silently assigned to a new cohort.
