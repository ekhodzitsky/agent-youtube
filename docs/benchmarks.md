# Benchmarks and evidence boundaries

[Project overview](../README.md) · [Documentation](README.md)

Historical measurements, rechecked against retained local JSON on 2026-10-27.
This documentation edit performed no fresh network or ASR trials. These figures
are not a current-release performance certificate or availability SLA.

## Russian reference quality

Measured 2026-10-24 with GigaSTT 2.21.0 and GigaAM v3 `e2e_rnnt` INT8.
Five Russian FLEURS read-speech clips contain 103 normalized reference words.
The three modes use the same sources and model; lower word error rate is better.

| Boundaries | Clips | Word errors / reference words | Orthographic WER |
|---|---:|---:|---:|
| Full context | 5 | 11 / 103 | 10.68% |
| Fixed 10-second cuts | 5 | 11 / 103 | 10.68% |
| Quiet 10-second cuts | 5 | 8 / 103 | 7.77% |

Two additional full-context trials tested seeded synthetic noise and ten seconds
of silence. They are excluded from the five-clip denominators. The noise case
had zero word errors; silence had zero inserted words and undefined WER.
One observation per case/mode supplies no statistical confidence interval.

### Configuration and data

- Pinned [reference manifest](../tests/fixtures/asr-references.json), FLEURS revision
  `70bb2e84b976b7e960aa89f1c648e09c59f894dd`, archive/member hashes, sample counts,
  published references, and CC-BY-4.0 attribution.
- Two encoder threads, one engine per trial, pool size 1, observed RSS budget
  1 GiB and inactivity budget 120 seconds. Full context uses 600-second chunks;
  fixed/quiet modes use 10 seconds. Exact windows and model/binary fingerprints
  are retained in the results. No hardware-independent throughput is claimed.
- [Derived numerical results](benchmark-results.json) contain all 17 trial
  metrics, configuration fingerprints, and the original report's SHA-256.
  They contain no ASR text, audio, model data, or absolute machine paths.
- Original raw/canonical/coverage outputs stay local-only under
  `runs/agent-fixes-20261024/quality-final/`. Its `quality.json` records per-trial
  paths; coverage records identify producer revision/source fingerprint.
  These artifacts are not included in a clean clone.
- The [evaluator](../src/pipeline/evaluation.rs) reuses production recognition,
  checkpoints, publication, and cleanup.

Metrics lowercase Unicode, normalize the Russian yo/e distinction, and compare
alphanumeric tokens. Numeric spellings are not expanded. Published references
were not independently corrected. Orthographic WER is not a calibrated probability
of correctness. These tiny read-speech samples do not qualify music, spontaneous
or overlapping conversation, other languages, or word alignment. GigaSTT 2.22.0,
recommended for setup, was not used in these historical measurements.

### Reproduce

Download the archive in the manifest, verify its SHA-256, and extract only the
five named WAV members into a dedicated directory. Provision GigaSTT and the
matching models separately. From the repository root:

```sh
AGENT_YOUTUBE_REFERENCE_AUDIO=/absolute/reference-wavs \
AGENT_YOUTUBE_QUALITY_OUTPUT="$PWD/runs/quality-new" \
AGENT_YOUTUBE_QUALITY_MODELS="$HOME/.gigastt/models" \
AGENT_YOUTUBE_QUALITY_GIGASTT="$HOME/.local/bin/gigastt" \
cargo test --locked --lib reference_corpus_quality -- --ignored
```

Use a new output directory for a new experiment. Exact historical reproduction
also needs the original producer source/build and GigaSTT 2.21.0. The current
checkout produces a new measurement. Inspect `quality.json` and trial evidence;
sum errors and reference words over the five clean cases per mode, excluding
`synthetic-noise-full` and `silence-full`.

## Acquisition comparison

Historical 2026-10-24 Linux same-host release trials: agent-youtube baseline `700fd67`
and yt-dlp 2026.10.21. Two recordings, one trial per tool/recording; both tools
completed both recordings with full decoding validation.

| Recording duration | agent-youtube acquisition + native decode | yt-dlp acquisition only |
|---|---:|---:|
| Approximately 53 seconds | 21.359 s | 26.779 s |
| Approximately 853 seconds | 32.090 s | 31.218 s |

**These columns measure different work and do not establish a speed ranking.**
The tools selected different valid AAC representations. yt-dlp used Node 22.19.0,
default clients, no account/proxy/PO-provider plugin, zero retries, and five-second
request pacing. Its later FFmpeg validation is excluded from the timing. Order
was sequential, not a repeated randomized study. Track identity was not qualified
by listening. No ASR comparison or representative reliability estimate follows.

Local source: `runs/competitor-comparison-20261024/trial-summary.json`.
agent-youtube release SHA-256:
`f74b50211500ca406648fd0f002b25495975b456438e37f735212bbd52480d2b`.
yt-dlp executable SHA-256:
`b7af77f686e95f7845ce8d09d21957718febb8b48b30555adb9dd2292ad7bf14`.
The earlier debug run is excluded from this release table.

The [qualification harness](../experiments/qualification/README.md) explains
new acquisition trials, pinned inputs, interruption/resume checks, and failed
or skipped outcomes. It now supports a broader corpus and dated campaigns.
Those later experiments must not be pooled into these two-recording denominators.
Honor source cooldowns; success cannot be guaranteed by repeating requests.

## Agent response size

A separate 2026-10-24 run produced 46 passages from 852.7993125 seconds of audio.
Its compact response was 25,443 bytes versus 357,734 bytes of canonical JSON;
exact segment text was preserved. This measures bytes, not tokens or ASR quality.
Local source: `runs/agent-fixes-20261024/youtube-verification.json`.
Contiguous input coverage proves processed samples, not correct recognition.

## Job recovery qualification

The root's recovery table measures agent-youtube's acquisition/publication contract,
not GigaSTT accuracy. It uses retained 2026-10-28 Linux x86_64 observations from
0.3.10 development build SHA-256
`b2db95fecbd691c7fb922b68ff0a3c43c993660b35b1410416306b7618b22e3c`.
[Sanitized numerical evidence](recovery-results.json) records exact sample counts,
input IDs, checkpoint settings, source/transcript hashes and build provenance.
The original local report hash is included; no audio or transcript text is copied.

- `of4B3aw7MXo`: 847,436 decoded 16 kHz frames (52.96475 seconds), one raw ASR
  result, default 300-second checkpoint setting, validated transcript and cleanup.
- `bedsdFbhqhw`: 16,861,403 frames (1053.8376875 seconds), 36 raw results with
  30-second checkpoints, two encoder threads, GigaSTT 2.21.0. The process was
  killed after 163,880 source bytes were durably saved; `status` reported resumable,
  and `resume` reached validated publication and cleanup. SIGKILL produced no
  final JSON, as documented by the agent contract.

One observation per scenario is not a reliability estimate. These are different
inputs and checkpoint sizes, so the table makes no throughput comparison; timing
variability is not measured. Both runs used local models and the current network,
without paid services. The exact dirty-source/build identities are retained in
the numerical extract; neither run is presented as a pristine current release.
Original artifacts are local-only under `runs/agent-acceptance-20261028/`.

For a new measurement, run the root quick start with a fresh work directory and
verify `status`, the transcript, raw hashes, manifest frame coverage and removal
of owned media. To reproduce forced restart, start the long input with
`--chunk-seconds 30 --encoder-threads 2`, retain the CLI PID, wait for a durable
acquisition-progress event, terminate that PID with SIGKILL, then run `status`
and `resume` on the same directory. Record the prefix, exit codes and new evidence;
YouTube may refuse a new acquisition. Never compare partial-byte acquisition with
these fully decoded/published outcomes. The automated [CLI recovery harness](../experiments/qualification/cli_resume.py)
also supports explicit cancellation trials; cancellation and SIGKILL are distinct
cases. Historical ASR accuracy measurements above remain separate.
