# Local WebM and Opus qualification

Assessment date: 2026-11-03. This is a standalone consumer experiment, not a
released feature. The production downloader and decoder remain AAC/M4A only.
No production dependency, JSON schema, checkpoint or transcription behavior
changes follow from this report.

## Sources and method

The current experiment uses clean committed Unlace 0.2.4 source at
`eef3e147966d86e8d4bab4d863be74b32524cb3c` and Opura 0.1.66 at
`453b714acc96d9fb569b888e34bde24f32d72c6f`. The previous Unlace 0.2.1 trial is
preserved separately. Opura is unchanged between trials. Both libraries are
pure Rust with no third-party Cargo dependencies; they are not added to this
repository's runtime dependencies. Unlace now includes an MIT license, but
its private, unpublished source is not redistributed here.

The prototype reads sequential Unlace events through the final completion event,
decodes Opus packets at 48 kHz, buffers at most one bounded laced block, and
applies initial pre-skip and whole-block discard padding. Decoded PCM16 is
streamed to a rolling hash. Multi-hour PCM is neither retained in memory nor
written as a complete output file.

The fixed-point reference is unchanged scalar libopus 1.6.1 at
`22244de5a79bd1d6d623c32e72bf1954b56235be`. It decodes the same packet records
for exact PCM comparison. FFmpeg 7.0.2 independently extracts encoded payloads
and performs a separate full decode to check sample counts. The reference
packet path shares the prototype's trimming formula; it alone cannot establish
independent agreement on container timing. Reference tools are used only for
research, not by the Rust prototype or production runtime.

Private local evidence includes immutable source and executable hashes, input
sizes and hashes, scripts, packet records, per-case results and process resource
logs. Raw research, media and private source snapshots remain excluded from Git.
A clean clone cannot reproduce these private trials without obtaining their
inputs and library sources separately.

## Results

All 24 outcomes match the earlier Unlace 0.2.1 experiment, including exit status,
decoded byte count, complete PCM hash and first/last spans:

| Check | Result |
| --- | --- |
| Complete decoding | 22 cases match fixed-point libopus PCM byte for byte |
| Independently demuxed encoded payloads | All 22 decoded cases match FFmpeg |
| Independently decoded sample counts | 16 cases agree; six timing cases remain discrepant |
| Excessive padding | Two inputs are rejected because padding exceeds the block duration |
| Full natural speech | 5.3878 hours, 969,807 packets, 931,013,796 mono frames at 48 kHz |
| Long-file memory | Rust child peak RSS 3,176 KiB on the recorded Linux host |
| Address-space ceiling | Complete short YouTube input passes with a 128 MiB child limit |
| Restart replay | Interruption after 100 packets, then replay from zero, reconstructs exact full PCM |
| Short reads | Read caps of 1, 7 and 17 bytes preserve PCM |
| Truncation | Six byte-truncated inputs fail without a final completion event |

Inputs include a previously downloaded YouTube recording, English/Russian/German
speech, mono/stereo, SILK/Hybrid/CELT, Chromium MediaRecorder audio and mixed
containers, and generated lacing cases. The long input is concatenated natural
read speech from the existing corpus, not a newly acquired multi-hour YouTube
conversation. No new download or recognition-quality trial was performed.

Release compilation, formatting and denied-warning Clippy pass on Rust 1.89.
These observations do not establish a cross-platform resource bound or a speed
ranking. The new prepared seek-session APIs are not used by this sequential
consumer and are outside this qualification.

## Remaining integration limits

Six fixed/Xiph/EBML lacing cases have positive or negative 25 ms whole-block
padding across 60 ms of decoded audio. The prototype emits 35 ms; FFmpeg emits
60 ms for positive padding and zero for negative padding. These disagreements
remain visible. An initial production adapter should refuse these combinations
until another independent timing check establishes the intended behavior.

Removing an entire final Cluster from an unknown-size WebM can still produce
normal container EOF. This was reproduced; the separate expected-source-length
check rejects the shortened object. Decoding every available packet therefore
does not prove that the intended audio was completely downloaded. Decoder
evidence must be bound to acquisition completeness, source identity and coverage.

The pilot accepts the first enabled Opus audio track, mapping family zero,
mono/stereo and zero output gain. Nonzero gain and foreign codecs fail explicitly.
Fractional-sample padding, timestamp gaps, other mappings and arbitrary track
selection require additional policy and verification.

Restart recovery currently replays from the beginning and discards already
published frames. It is not a serialized codec checkpoint or random seek.
Production integration still needs source/track/backend isolation, the existing
16 kHz mono chunk pipeline, durable publication and recovery checks. Broader
format availability does not remove YouTube access restrictions.
