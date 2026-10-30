# JSON and timestamp semantics

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## JSON and timestamp semantics

The immutable canonical transcript and `--response full` use schema **1**.
The default derived agent view uses schema **2**, described above. Canonical schema1
stores `source` (normalized URL, video ID, optional title),
`language`, audio `duration` in seconds, `engine` identity and `segments`:

```json
{
  "schema_version": 1,
  "source": {"url": "https://www.youtube.com/watch?v=of4B3aw7MXo", "video_id": "of4B3aw7MXo", "title": null},
  "language": "ru",
  "duration": 2.0,
  "engine": {"name": "gigastt", "version": "gigastt 2.22.0", "model_variant": "e2e_rnnt"},
  "segments": [{"start": 0.12, "end": 1.2, "text": "Пример текста.", "timing": "words",
    "words": [{"word": "Пример", "start": 0.12, "end": 0.6, "confidence": null, "speaker": null},
              {"word": "текста.", "start": 0.7, "end": 1.2, "confidence": null, "speaker": null}],
    "speaker": null}]
}
```

The example is illustrative, not a claimed recording. Seconds are absolute relative to
video/audio start; per-chunk offsets are added to actual ASR word times. Segment text is
preserved exactly from the recognizer. Unknown upstream fields survive in `raw/`.
Since 0.3.11, JSON parsing preserves f64 roundtrips for confidence and timestamps.
After validating relative word times, assembly bounds floating-point additions to
the exact sample-derived endpoint to prevent a rounding-only overshoot. Empty-word
segment endpoints also come directly from the integer sample span. Raw bytes and
previously published transcripts are never rewritten by this fix.
Missing word times produce honestly marked `timing: "chunk"` intervals; speaker IDs and
confidence remain absent/null if unavailable. Mono recognition currently does not produce
speaker labels. No diarization or fake word alignment is added. Real gigastt word intervals
can have zero duration and are preserved in JSON. SRT/VTT coalesce instantaneous words
into adjacent real intervals; if a cue has no representable positive duration, subtitle
export reports an error rather than inventing time (JSON/TXT/Markdown remain available).

SRT/VTT use millisecond timestamps and escaped single-line cue text. Short cues are built
from actual words only when their concatenation matches the ASR text; otherwise the exact
segment text is used as a larger cue. Markdown/TXT retain exact segment text with timestamp
prefixes. Speaker IDs are retained when present. Incompatible schema versions, nonfinite,
negative, out-of-bounds or out-of-order times fail validation. Exports reject timestamps
that overflow their integer millisecond representation instead of saturating them.

### Quality limits

ASR makes mistakes, particularly in names, English technical terms, music and overlapping
speech. Word times are ASR estimates, **not forced alignment**. The default external 300-second
checkpoint boundaries are nonoverlapping and may cut a word; gigastt internally uses its
own overlapping recognition windows.

Optional `--boundary-window-ms 1000` searches within one second of each nominal ASR cut
for a quiet 200 ms span (mean-square level approximately below -35 dBFS, 20 ms search
step). Values are 0..2000; zero keeps fixed cuts and hardlinked batch inputs. Continuous
speech without a qualifying pause keeps its nominal cut. The immutable `recognition.json`
plan stores contiguous integer sample boundaries and is retained as evidence. It is hashed
before reuse. Shifted inputs copy exact PCM across decoder chunks; no samples, text or
words are discarded, overlapped or deduplicated. Canonical offsets follow the plan while
raw JSON remains untouched. Changing this option on an unfinished run requires a new work
directory. A backend identity suffix also prevents older binaries from mixing these results.
Fixed decoder chunk/state recovery is unchanged. Shifted batch WAVs require extra temporary
disk, up to another approximately 115 MB/hour if all cuts move. They stay owned until
validated transcript publication; capacity is checked before writing each shifted input.
A real speech cut experiment reduced lexical disagreement with full-context ASR from four
edits to one, but this is not human-labelled WER or a general accuracy guarantee. The recognizer's exact text and all raw JSON results remain
available for correction. Reviewing audio later requires fetching it again after cleanup.

A pinned [FLEURS reference manifest](../tests/fixtures/asr-references.json) supplies five
licensed read-speech references (names, numbers/units and a foreign term). The opt-in
`pipeline::evaluation::reference_corpus_quality` test measures orthographic WER/CER
for full context, fixed 10-second cuts and the production quiet-cut plan, plus a
seeded synthetic-noise case and silence insertions. Twelve additional trials concatenate
the same five recordings in manifest order: unchanged, with two-second pauses,
attenuated by 20 dB, or with seeded noise, each under the three cut modes. These
are synthetic sequential speakers, not conversation, overlapping speech or a
diarization benchmark. The 29 trials reuse production ASR, publication, recovery
and cleanup. They also assert exact raw-to-canonical text, confidence and speaker
preservation, sample-contiguous input spans, word-time offsets within 1 ns and
lossless text reconstruction from compact passages. Published prompts are not independently corrected
here; this small read-speech sample does not qualify spontaneous conversation,
music, multilingual recognition or universal accuracy. Numeric spelling differences
are counted; `ё`/`е`, case and punctuation are normalized only in metrics.

To reproduce, download the pinned archive URL in the manifest, verify its SHA-256,
and extract only the five named WAV members into an isolated reference directory.
The original reference audio remains caller-owned. No models or network are needed
for the normal test suite; the reference test explicitly requires both local assets:

```sh
AGENT_YOUTUBE_REFERENCE_AUDIO=/absolute/reference-wavs \
AGENT_YOUTUBE_QUALITY_OUTPUT="$PWD/runs/quality" \
AGENT_YOUTUBE_QUALITY_MODELS="$HOME/.gigastt/models" \
AGENT_YOUTUBE_QUALITY_GIGASTT="$HOME/.local/bin/gigastt" \
cargo test --lib reference_corpus_quality -- --ignored
```

Read `runs/quality/quality.json` for the JSON evaluation report. Exact raw,
canonical and compact results and their manifests stay in per-trial directories.
Fixture duration evidence comes from the verified PCM frame count; this bypasses
acquisition and does not qualify downloading or container decoding. Failed trials retain owned WAVs
and can resume with the same inputs/settings; successful trials clean owned media
only after validated durable transcript publication. Reference source hashes and
PCM/configuration fingerprints prevent changed inputs from being silently reused.

On 2026-10-28, all 29 trials completed with 0.3.11 and GigaSTT 2.21.0
(`e2e_rnnt` INT8, two encoder threads). Word edit counts below use the same
103 reference words in each row, not independent new speech samples:

| Input condition | No external cuts | Fixed 10 s | Quiet cuts ±1 s |
| --- | ---: | ---: | ---: |
| Five original recordings, scored separately | 11 | 11 | 8 |
| Sequential concatenation | 9 | 15 | 11 |
| Concatenation with two-second pauses | 12 | 9 | 9 |
| Concatenation attenuated by 20 dB | 9 | 15 | 11 |
| Concatenation with seeded noise | 11 | 15 | 10 |

The separate short noisy case had zero edits out of seven words; ten seconds of
silence produced no words. All 93 reference chunks preserved raw text, confidence,
speaker fields and compact text, with timestamp offsets checked within 1 ns and
contiguous sample coverage. A separate historical 17:34 conversation preserved
all 2,017 words across 36 chunks and compact passages; its pre-0.3.11 confidence
values exposed the rounding defect fixed in this patch. Four preselected 20-second
conversation windows, centered at 30/300/600/900 seconds, disagreed lexically with
split recognition in two cases. These are context-sensitivity observations, not
human-labelled error rates. No audio-listening/manual correction was performed.
Quiet cuts are not universally best, and these results do not justify changing
the default or claiming complete speech recognition.

Recorded validation on 2026-10-17: a full 52.93-second Russian video and the full
3h49m17.64s candidate completed, including all four exports and resumed runs.
Those historical full runs used the previous downloader. On 2026-10-19, native HTTP/TLS
reached YouTube, but full native live runs were rejected by bot/account checks. An initial
SABR research probe returned real AAC initialization/media bytes; it does not establish
full native end-to-end live compatibility. Offline native Range/SABR tests cover complete
downloads, real AAC decoding, ASR publication, interruption/resume and cleanup.
On 2026-10-20, a fresh native short run completed download, decode, real gigastt
recognition and cleanup (52.9879375 seconds, 166 words), followed by all four exports
and completed resume. The current Web stack additionally completed a fresh short
recording and a 3:49:17.6838125 long recording with 46 chunks and 31,976 words.
The long run resumed a validated interrupted SABR prefix and completed decode,
recognition, publication and cleanup with an empty PATH and an absolute gigastt
path. All four exports and completed resume with absent tools/models were verified.
The competitor acquisition trial report (local historical record)
records current TV/embedded/VISIONOS experiments, complete byte/decode evidence,
a controlled yt-dlp comparison, watch-context attestation and cross-client resume.
The production client lifecycle report (local historical record)
records prepared VISIONOS/embedded promotion, safe refusal/attestation recovery and native decode trials.
The full acquisition qualification (local historical record)
reports default/client-specific completion denominators, interruption/restart evidence,
supported scope, measured memory and external infrastructure access/cost limits.
Research probes are explicit opt-in tests; they retain owned media when no validated
transcript has been published. Paid API/proxy access was not configured; browser-based provider trials use only
an isolated test browser and are not production dependencies.

See the current Web report (local historical record);
the High follow-up (local historical record)
retains the earlier incomplete baseline.

On 2026-10-22, the native MWEB watch route completed short AAC and a full
3:49:17 recording with native decoding. The long probe resumed an intact 4 MiB
prefix using its still-valid cached source URL and a fresh token; three fresh-page
resume attempts were blocked. Validated source leases now support production restart
without reopening the watch page; a fresh native interrupted/resumed short run also
completed actual ASR, publication and cleanup.
Safari exposed no HLS manifest. The final release controls encountered a shared
rate-limit challenge and returned structured retry guidance. See the
mobile fallback report (local historical record).

The [BotGuard / Boa controls](../experiments/botguard/README.md) identified eval scope
and arguments defects. The application uses a general corrected engine with browser
Annex B semantics; the unsafe diagnostic bypass remains excluded. Classic Create
issuance alone failed media authorization at about 70 seconds. Fresh homepage-flow
Rust tokens receive accepted stream-protection status, and cached minting and forced
refresh have been verified. See the current report (local historical record) for complete-transfer evidence.

See the verification report (local historical record).
Quality was compared automatically against another ASR head on actual audio samples;
human listening was unavailable. The candidate's final sample at **03:49:00** has
substantial recognition disagreement and needs manual review. GitHub CI could not
start because of the account billing/spending limit; the required local checks passed.

The current agent contract and verification (local historical record) records JSON output, reusable ASR, source evidence, measured performance and current TLS triage.

The historical AI-agent workflow audit (local historical record)
records the audited baseline, dependency triage and the proposed machine-result/evidence
contract. The historical reproductions describe the pre-fix baseline; subsequent fixes and verification are recorded locally.

The earlier detailed audit (local historical record)
records reproduced reliability defects, regression evidence and the targeted refactoring.
Checkpoint publication/recovery lives in `src/checkpoint.rs`; `src/pipeline.rs` coordinates
stages, `src/download/` owns extraction and transfer, `src/audio.rs` handles native media
processing, `src/web/` owns the private Rust Web runtime, and `src/process.rs` owns
ASR subprocess lifetime.
