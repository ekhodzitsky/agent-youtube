# Stored data and resume

[Documentation index](README.md) · [Project overview](../README.md)

Moved from the root README on 2026-10-27. Commands assume the repository root.
Historical verification statements retain their original dates and scope; they are
not new measurements of the current version.

## Stored data and resume

```text
runs/example/
  source.json          YouTube player response (API or parsed watch-page bootstrap)
  state.json           versioned checkpoint snapshot (internal schema4)
  job.json             saved invocation (internal schema1)
  job-retry.json       durable retry policy/schedule (private schema2; legacy schema1)
  job-control.json     current generation (internal schema1)
  job-cancel.json      optional generation-scoped cancel request
  state.PID.TIME.SEQ.journal/
    events             framed, hash-chained checkpoint changes
    head.json          atomically committed sequence, byte length and hash
  transcript.json      canonical versioned transcript
  media/source.audio   temporary original AAC/M4A audio-only stream
  media/audio.json     temporary receipt of fully validated decoded sample count
  media/audio.wav      temporary full PCM WAV in legacy unfinished runs
  chunks/000000.wav    temporary bounded audio chunks
  download.json       retained exact source digest and selected representation
  manifest.json       retained engine/source/transcript/raw evidence mapping
  coverage.json       retained exact input sample coverage and producing build identity
  raw/000000.json      exact original gigastt result per chunk, retained
  chunks/000000.resume.json  temporary exact codec/resampler restart state
  logs/                bounded stdout/stderr capture for every subprocess
  batch.PID.TIME.SEQ/  owned temporary hard links and unpublished ASR results
  .agent-youtube.lock         OS lock inode; its continued presence is normal
```

The owned downloader obtains YouTube player metadata and selects audio-only AAC/MP4.
When track metadata is present, it requires a unique track explicitly labeled original
in the English player response; default tracks never outrank the original. Automatic
dubs and descriptive alternatives are excluded. Missing or conflicting original-track
identity fails explicitly. Legacy responses without any track metadata remain supported,
although original status cannot be independently established from those responses.
The selected track ID participates in download receipt identity and resume validation,
and is sent explicitly in SABR requests.
The downloader supports direct media links and SABR.
Private, restricted, live, DRM, authorization-required and unsupported/cipher-only
sources fail explicitly; there is no video, WebM/Opus or external downloader fallback.
The WEB watch page supplies audio metadata and its client context. After a bot
check, unsupported audio, route-level HTTP403 or typed temporary bootstrap/transport
failure, selection can try another qualified anonymous client. MWEB uses its own
matching client version, context and User-Agent, and can recover a bot-denied watch
response through its validated same-page player API context. If Web clients fail,
a qualified Samsung TV route bootstraps a fresh anonymous visitor and signature
timestamp, then requests direct original AAC using its matching client context,
Origin and User-Agent. Anonymous cookies stay in memory; account cookies are not
imported. Prepared VISIONOS and embedded clients offer additional direct-AAC routes.
Embedded requests obtain the video's fresh embed visitor and encrypted host flags;
no third-party site is contacted. These direct clients solve n when present, do not
mint Web PO tokens, and reject unqualified streaming protocols. Typed attestation or
player challenge failures can select another compatible client; cooldowns stop recovery.
Explicit private/deleted/account restrictions, HTTP401/404, invalid metadata and
identity failures stop selection. Any error response with Retry-After stops client
switching. The initial chain has at most four passes through five qualified clients;
all-route HTTP403 stops after one pass. More clients do not provide independent
networks or guarantee access when every available route is denied.

The availability contract is bounded, recoverable acquisition of a completed
recording through a compatible accessible AAC/MP4 route. It requires working local
storage, sufficient resources, supported source metadata and a reachable authorized
source during the retry window. Success requires verified complete bytes, full
decoding and consistent declared duration within two seconds; metadata, a signed
URL or an initial byte range is insufficient. These checks establish transport and
decoded coverage, not the truth of publisher metadata or ASR word accuracy.
A complete retained source with valid receipt/evidence can be reused without a live
YouTube link; partial sources still need a compatible reachable source. Owned media
is temporary and is removed after durable validated transcript publication, so this
is not a permanent audio archive. Deleted unseen recordings, absent permission and
total denial of every reachable byte source have no unconditional recovery guarantee.
The final availability audit (local historical record)
separates real full downloads, controlled faults, hard limits and untested infrastructure.

A fresh homepage
challenge and its matching page configuration produce video-bound PO tokens in the
corrected Boa Rust interpreter. Player n-functions are prepared by the native Rust AST
parser and executed in that interpreter. This uses the original downloaded interpreter
and player literals; it does not rewrite the BotGuard program. YouTube changes, bot
checks and region/account restrictions can still prevent downloading public videos.

The private Web worker has an independent 90-second deadline per request, a 4 GiB
Unix address-space limit, a 64 MiB VM thread stack, VM loop/recursion limits,
and bounded IPC/HTTP messages. JavaScript receives no host filesystem, process or
network API. Tokens and VM state stay in memory; private IPC is not an application
log or public command result. The player runtime is cached by script SHA-256 and
the integrity minter is reused before its estimated expiry minus a safety margin.
Tokens are minted for the requested video and refreshed before expiry or on stream
rejection/stall. The worker is stopped after download or when an operation fails or
is cancelled. See the Web verification record (local historical record).
Temporary player-script and token-mint transport/server failures retry those idempotent
operations independently, at most three times with cancellable 5/10/20-second backoff.
The n transform is applied once after successful script retrieval; token retry does not
transform an already prepared URL again. Preparation cooldowns return immediately.

Direct media uses 1 MiB HTTP Range requests. Status 206, Content-Range, declared body
length, final file length and strong ETag (when available) are checked. SABR enables only
audio; its required video format reference is marked fully buffered and any video payload
is rejected. UMP parsing bounds part/segment sizes and validates the audio identity,
byte offsets, segment lengths, sequence order and media-end markers. Protocol references
and MIT notices are in [NOTICE](../NOTICE).

syom stream-decodes the seekable M4A and mixes to mono, bandlimit resamples to 16 kHz,
and ryf writes bounded PCM16 WAV chunks directly, without a duplicate full WAV.
One configured chunk is buffered; resampler history continues across boundaries.
Chunk boundaries use integer sample counts; legacy full-WAV runs copy their payload
without another conversion. The final fractional output
sample is retained by rounding its count upward. Duration comes from PCM sample counts.
AAC priming and tail trimming follow syom's M4A edit-list/gapless handling.
An unfulfilled priming skip or bounded presentation window fails before ASR or final
publication, retaining source media; open-ended fragmented presentations remain supported.
Parser/workspace validation stays enabled. Decoder index/metadata budgets scale with
the actual source size; allocation or format errors retain source media.

Reusing an existing manifest also rechecks the retained source metadata, download
evidence and raw ASR files against their checkpoint hashes. Missing or modified
evidence fails before cleanup; recognition is not silently repeated to replace it.

After durable, validated `transcript.json` publication, the program removes the owned
compressed source, decoded-length receipt, legacy decoded WAV, WAV chunks and registered
temporary files. Raw ASR JSON,
metadata, state, logs and exports remain. `media/` and `chunks/` are normally empty after
success. Completed legacy runs can still finish cleanup, including receipt-backed
`media/video.mkv`, without recognition or the old media tools.

Legacy WAV chunk copies and validated raw ASR results update the publication journal in
groups of at most 32 files. Scratch ownership is durable before writing; all publication
intents are durable before linking destinations, and each file retains its integrity and
fsync checks. A crash can leave a partially published group, which resume completes.
Fresh decoded chunks are published individually as they finish. Qualified AAC-LC runs
also publish a versioned exact codec/resampler state after each full chunk. Resume
verifies the source, state and completed-prefix chunks, then decodes only the remaining
suffix. Qualification covers 1024-sample mono/stereo LC, native rates 8/16/22.05/24/32/
44.1/48 kHz, and an edit list or absence of an iTunSMPB trim tag. Unqualified profiles,
rates, stateful extensions and legacy runs without state retain exact replay from zero,
reusing verified chunk files. Recorded corrupt/incompatible state fails with
`decode_resume_invalid`; it is never silently accepted. State files remain temporary
owned media and are removed only after validated transcript/evidence publication.
The decoded-length receipt is published only after complete source validation; ASR never
starts from an incomplete decode. An interruption before legacy group publication may
repeat up to 32 chunk preparations; owned temporary files and original ASR outputs remain
preserved until successful cleanup.
Checkpoint changes append to a framed SHA-256 journal whose small atomic head identifies
the committed prefix. Full snapshots are published when accumulated journal bytes reach
the preceding snapshot size (at least 64 KiB), and at selected phase boundaries. Metadata
writes now grow with recorded changes instead of repeatedly rewriting the growing map.
The artifact map still uses memory proportional to the number of chunks.

Keep `state.json` and its referenced journal directory together: the snapshot alone can
be stale during a run. Resume replays only the committed suffix after its snapshot anchor;
confirmed truncation, checksum/sequence errors and linked journal files fail without
discarding evidence. Bytes beyond the durable head are uncommitted and remain untouched
on load; the next append can replace that tail. A failed writer must be reopened.
Journal history remains retained metadata even after a snapshot advances the replay
anchor. Schema1/2/3 runs migrate to schema4 by snapshot on checkpoint mutation; old
journals remain readable. Schema4 tracks retired acquisition namespaces so failed
attempts survive restart and are cleaned after publication. Older binaries reject
schema4 rather than lose ownership information. Canonical transcripts remain schema1;
compact responses remain schema2. See the
journal measurement and recovery record (local historical record).

Fresh unfinished runs store about 115 MB/hour of PCM chunks plus compressed source audio,
small per-chunk codec state and checkpoint metadata,
instead of keeping both a full WAV and chunk copies (about 230 MB/hour in legacy runs).
All owned media remain until durable validated transcript publication. Chunking therefore
halves PCM storage but does not make retained disk use independent of recording length.
Before decoding, a known YouTube duration estimates the allocation of missing PCM chunks;
actual sample counts remain authoritative. Each new decoded chunk also checks capacity
before writing, including when duration is unknown or underestimated. Saved verified chunks
are excluded from the estimate. Legacy full-WAV runs retain their chunk-copy preflight.
Checks use Unix `statvfs`, including WAV headers and per-file allocation rounding.
`insufficient_disk_space` reports required/available bytes and allocation unit for preflight,
with `stage: "decode_audio"` or `"prepare_chunks"`. Late disk/quota write failures also use
that code and stage, without claiming capacity figures that are no longer known.
Free space and resume the same work directory; source and published chunks remain intact.
A failed capacity query returns `disk_space_unavailable`. Checks do not reserve space or predict per-user quota and competing writers. Downloads
preflight known remaining compressed bytes with a small metadata allowance. Unix atomic
byte publication checks its known payload allocation; publication and batch preparation
check free inodes when the filesystem reports an inode limit. Actual ENOSPC/EDQUOT from
owned download, checkpoint, publication, pipeline and subprocess-log writes becomes
`insufficient_disk_space` with stage/path details. Raw ASR size remains unknown before
recognition; an external engine failure reports `process_failed` with exit code/signal
and log path without guessing its cause from prose. Other platforms report an unsupported
audio capacity query rather than skipping it. Portable exports retain their existing behavior.
Linux allocation planning uses one quarter of detected headroom from MemAvailable,
cgroup v2 limits (including ancestors) and a finite RLIMIT_AS. Decoder metadata/index
budgets are clamped to that capacity; large chunk, checkpoint snapshot/replay and
state-load allocations are checked before allocation. `resource_limit` identifies
`path`, `stage` and preserved evidence; planning failures include required/available/
budget bytes and can be retried after resources improve. Codec-specific budget failures
use unknown retryability because some limits require a configuration/code change.
Pending ASR indices also grow through fallible geometric reservation, with a conservative
old-plus-new buffer memory check before each growth. Refusal uses `resource_limit` with
stage `prepare_recognition`, before creating the ASR batch, and preserves previous raw/media.
These are best-effort checks, not a hard RSS reservation; other platforms keep codec
fences without Linux memory discovery. External SIGKILL/kernel OOM cannot emit JSON.

Scheduling checks on Unix reuse a digest only while device/inode, size, link count,
mode and nanosecond modification/change times match a prior strict check or owned
publication. Source validation, ASR input, canonical raw consumption and cleanup still
hash independently. This memo does not replace evidence validation at those boundaries.

A direct-media HTTP 403 permits one retry of the same source per download call,
with a five-second wait, only when its known signed lifetime exceeds the wait and
existing expiry margin and required attestation remains valid. This consumes the
existing retry budget. Persistent refusal still renews the current client before
exclusion; Retry-After and cancellation take precedence. This does not promise that
403 responses are transient or that every source can be obtained.

Media are streamed from disk rather than loaded wholesale.
Before recognition, decoded frames must agree with a finite positive declared source
duration within two seconds, allowing integer metadata rounding and encoder padding.
Receipts and private leases retain the selected duration, which must remain consistent
with initial metadata. Decoded coverage must satisfy both available declarations;
the tool never chooses a shorter declaration just to fit a partial recording.
Older receipts can use initial metadata only when its exact
representation matches the saved download. Completed-job reuse and idle status apply
the same gate. Old runs lacking this evidence may now fail closed; their exact ASR and
transcripts remain readable/exportable and are never rewritten to pass validation.
Canonical assembly, completed validation and CLI exports process one segment at a time;
the whole transcript is not loaded into RAM. There is no fixed recording-duration,
chunk-count, 64 GiB transfer or 256 MiB transcript limit. Chunk names extend beyond six
digits. Decoder index and checkpoint metadata still require memory proportional to
the recording structure; filesystem/container/integer limits and available disk/RAM
still apply. Individual raw ASR/YouTube metadata JSON remain limited to 16 MiB per unit.
Default checkpoints are 300 seconds (`--chunk-seconds 10..600`). See the
long-recording support record (local historical record).

Run **the exact same command and work directory** after a failure. Completed media and
ASR chunks are verified by streaming SHA-256 and reused. Owned downloads keep a JSON
receipt beside their M4A in the persisted download namespace. A receipt stores the video,
format/track identity, modification timestamp, transfer protocol, expected length and
SHA-256 of committed bytes; SABR also records completed segment progress. Bytes are synced
before the receipt is atomically updated. Resume verifies that prefix and removes only an
uncommitted crash tail from the owned partial file. Truncated, changed, linked or unreceipted
files are preserved and rejected. Expired links are refreshed with bounded retries.
Receipts bind the selected WEB, MWEB, Samsung TV, VISIONOS or embedded client for resume.
When an MWEB watch page reports a bot check, a validated MWEB context from that
same page can authorize one player API request. Invalid context, content restrictions
and server cooldowns do not authorize this fallback. A successful API response still
requires complete media transfer and decoding before acquisition is qualified.
After repeated media HTTP403/410 or explicit refresh refusals, the downloader can
exclude the failing client and select a compatible source from another qualified client.
Video, format, track, modification timestamp, MIME, length and variant must match.
Before adopting another client, the new direct source must reproduce the SHA-256 of
**every committed prefix byte**, using bounded Range reads and the retained strong ETag
when present. This also permits a verified SABR-to-direct transition: the receipt then
records the new transport and clears obsolete SABR sequence state. Sources with different
audio identity are never spliced. Switching to SABR under another client is not qualified.
Cooldowns and content restrictions stop recovery; failing media and receipts remain owned
and retained. This can add prefix-sized network traffic during recovery.
Expired retained sources use the same compatible-client selection. Old binaries cannot
resume receipts naming clients they do not support; resume with the current version.
Replayed SABR segments must match saved bytes.

A receipt may reference a private, content-addressed `media/download.*.lease-*`
file containing only the player selection/negotiation fields and required client/script
context. Resume checks the committed prefix, lease hash, trusted hosts and exact
representation before using it. Generated PO tokens and SABR session state are never
saved there; the native worker prepares fresh attestation after restart. Lease expiry
uses the earlier of the signed URL deadline and the reported validity period, with
a 60-second preparation margin. Sources without a usable URL expiry use fresh lookup.
Already complete audio with a verified receipt-bound lease identity and full file hash
can be reused even after URL expiry, without network or attestation. Incomplete media
never uses an expired URL; an expired lease cannot hide a fresh matching bootstrap.
Expired sources, media HTTP403/410 and explicit SABR refresh requests reacquire matching
metadata; temporary transport/body failures retry the current source. Four successful
SABR cycles without committed bytes first refresh attestation when applicable.
If four more cycles still make no progress, recovery renews the source context once
through the same identity checks. Without attestation, renewal follows the first four
stalled cycles. A further four stalled cycles return `download_no_progress` and retain
media. Actual committed progress permits a later renewal; replay alone does not.
Shared cooldown
also blocks token issuance. Leases remain after failure and are removed with owned
media only after validated transcript publication. Corrupt/linked lease files are
preserved and rejected. See the restart verification (local historical record).
Only chunks lacking validated raw JSON are retried, including later successes in a failed batch. For an unfinished run, engine,
model, media backend or chunk-size changes require a new directory, preserving the old results.
Unfinished runs created by the FFmpeg version require a new work directory: their source,
WAV and raw ASR files are preserved, and results from different preprocessing are not mixed. Ambient `GIGASTT_*` and `ORT_*` settings are rejected before starting recognition.
Use `--encoder-threads N` explicitly, or leave it unset for gigastt auto. The explicit
thread setting participates in unfinished-run identity. This prevents hidden VAD,
hotword, runtime-library or destructive batch options from changing a run. Completed
reuse needs neither the engine nor those settings. Older unfinished runs with ambient
settings require a new directory or their original implementation; their data is preserved.
Reported ASR duration may differ by up to 0.2 seconds from its PCM span; the in-memory
assembly uses the actual span and revalidates word bounds. Raw JSON and word timestamps
are never rewritten. Linked or nonregular files in the owned download namespace are rejected
before opening the download files; remove such links explicitly before resuming.
A finished transcript is validated and returned without another recognition run or
requiring the previous binaries/models. Interrupted media cleanup resumes on that path.

Publication intent and digest are saved *before* linking a completed output, allowing
recovery if a process dies between file publication and checkpoint update. Publication
and checkpoint writes are atomic on the same local filesystem; existing unrelated files
are protected. Unsupported filesystems without hard links return an error. Hash mismatch
or conflicting final files are diagnosed and preserved: keep the damaged run for inspection
and start another directory. Do not edit state paths or hashes by hand.

Ctrl-C/SIGTERM interrupt native downloading/decoding/chunk copying or stop the active
child process group and return **130**. On Unix the runner
also tears down surviving group members after an ordinary successful or failed leader exit,
before releasing the run lock. Group signalling uses OS calls; Linux verifies live group
members through byte-safe `/proc` reads and ignores zombies and vanished entries. No
`kill`/`ps` executable is required by the production runner. Other Unix platforms use a
conservative native group-existence check. Teardown and diagnostic drainage share a
five-second userspace budget plus up to 50 ms for reader closure. Child waiting is polled;
Unix diagnostic pipes are nonblocking, and only finished reader threads are joined.
A detached pipe holder produces `process_log_timeout` instead of an indefinite join.
If group shutdown cannot be verified, `process_teardown_failed` has unknown retryability,
leaves staging unadopted and avoids explicitly unlocking the inherited run lock. This code
also remains visible after cancellation. Confirm the worker has stopped before resuming.
Fallible diagnostic thread creation returns `resource_limit` with stage
`subprocess_log_thread`; partial setup is torn down through the same path.
These bounds cover userspace waits, not an OS syscall stuck in the kernel. Non-Unix blocked
reader threads can outlive the returned error until their pipe closes. Tools that deliberately
detach into another process group are outside group termination guarantees. An OS lock prevents
concurrent writers. On Unix, child stdin inherits the locked file description so a worker
that outlives an abruptly killed parent continues to hold the lock; this safeguard assumes
the external tool preserves stdin. Unusual wrappers that close stdin weaken this behavior.
If an orphan is still running, wait for it or stop that specific process before resuming.
If recognition fails, source media and scratch remain for resume. After success, cleanup
uses registered media receipts, registered scratch and a unique persisted download
namespace. It never recursively removes user directories or unknown files. Every cleanup
target is preflighted before deletion; changed media or symlinks produce an error and are
preserved. Unlinks are synced before their receipts are retired, so interruption during
cleanup is recoverable. A cleanup error does not remove the finished transcript.
Historical unregistered scratch from older versions is preserved because its ownership
cannot be established automatically.

On Unix, designated work directories must belong to the current effective user. Newly
created directories use 0700; existing owned work directories and the standard work
subdirectories are tightened to 0700. Foreign-owned or linked work directories are
rejected before writing. New owned artifacts, logs, checkpoint/journal files and exports
use 0600 independently of a permissive umask. Only the ASR child receives umask 077, so
its staging outputs are private without changing the host process umask. Existing files
and unknown paths are not recursively chmodded or rewritten; private ancestors protect
legacy contents. Select a dedicated work directory, since its access permissions are
intentionally tightened. Previously opened descriptors or external hard links are not
revoked by directory permissions.

Progress and terminal diagnostics use the JSON contract above. ASR uses one
`gigastt transcribe-batch` process for pending chunks, `--pool-size 1`, no retries,
JSON only and offline mode. Models/sessions are reused across files, with automatic
encoder CPU threading unless explicitly set. Exact per-file results are adopted after
the process group stops because upstream batch writes are not atomic. Linux progress observation uses inotify, retries incomplete
files and reconciles once on event overflow; completed files are not repeatedly listed.
Resource/permission/unsupported-notification failures use polling after validating the
output directory; unrelated setup errors still fail. Fallback polling on Linux and
other platforms runs at the 200 ms progress cadence, including with short inactivity
deadlines. One final directory validation follows
worker teardown, including files created immediately before exit. On timeout or
failure, valid files are checkpointed and incomplete output is preserved for inspection.
A killed parent leaves recorded staging for validation and adoption on the next resume.

`--asr-memory-mib N` optionally monitors the summed RSS of the Linux ASR process group
at 200 ms intervals. Exceeding it stops the group with `resource_limit`, observed/budget
bytes, stage and preservation details. Valid results are adopted after teardown and the
same run can resume with a larger budget; this setting does not change recognition identity.
Peak observed RSS is emitted as a JSON progress event. Shared mappings can be counted more
than once. This is a sampled stop policy, not a hard allocation limit or a cgroup; peaks
between samples and an external OOM kill cannot be prevented or reliably diagnosed.
A monitoring setup/read failure returns `resource_monitor_unavailable`. Without the option,
no process-group RSS scan runs. Non-Linux rejects an explicit budget.

`--asr-timeout-seconds N` defaults to 1800 seconds **without a new validated chunk**;
progress resets this inactivity deadline, so total recording length is not capped.
Dependency version/help probes have a 30-second deadline. Each subprocess diagnostic
stream keeps at most 1 MiB; exceeding it stops the group and reports `diagnostic_limit`.
Individual ASR JSON larger than 16 MiB also stops the batch. Deadlines/limits retain
source and PCM for resume. Batch media and scratch are only cleaned after durable
transcript and evidence publication, and unknown namespace entries are preserved.
Subprocess errors include the executable, exit status and private stderr path;
diagnostic text remains in owned logs and is not copied into the public error.
Network requests use bounded timeouts;
retries and body/segment processing check cancellation. Removing local duration limits
does not remove YouTube access or request-rate limits. This CLI does not import browser
cookies or bypass account restrictions; its PO tokens authorize the anonymous Web
streaming protocol. Live verification and remaining constraints are recorded in
the Web report (local historical record). Source 0.16.0 uses a distinct v4 media
backend for the pinned unmodified syom 0.10.0 candidate and its upstream checkpoint API.
Jobs from another backend, including v1/v2/v3, are refused before mutation. Resume
them with the original implementation or use a new work directory; all their
media and raw results are preserved. The earlier v1 validation could also accept
incomplete decoded audio or an incorrectly selected source track.
Previously completed transcripts are not rewritten or re-recognized automatically.
A partial file from the old yt-dlp
backend has no native receipt and needs a new work directory; it is preserved.

For new downloads, `download.json` retains validated source byte length/SHA-256 and
the actual selected representation, without media URLs, PO tokens or sessions.
Before cleanup, `manifest.json` durably links that receipt, configured model/binary
hashes, backend identity, transcript digest, raw file digests and absolute time spans.
These files are checkpointed and integrity-checked on reuse. Legacy completed runs
return `artifacts.manifest: null` when their retired source evidence cannot be recovered;
no source selection or digest is invented and existing transcripts are not rewritten.
