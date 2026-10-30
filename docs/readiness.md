# Current readiness and support — 0.8.0

This is the current decision entry point. Dated qualification reports are evidence
for their exact source, binary, model, network and scenario; they are not silently
upgraded to new-release measurements. See the [changelog](../CHANGELOG.md).

| Use | Disposition |
| --- | --- |
| Controlled private Linux x86_64 GNU jobs | Qualified functional candidate within the support boundary below |
| Agent integration | JSON-only CLI; closed machine schema and executable conformance checks |
| Unrestricted production-security approval | Not granted: independent review of the selected TLS path remains outstanding |
| Public source distribution | Experimental source in the independent agent-youtube repository; CLI is agent-youtube |
| Crates.io / prebuilt release uploads | Not enabled; publish=false and local-only artifact preparation |

## Support boundary

| Area | Supported/qualified scope | Outside the claim |
| --- | --- | --- |
| Transcription input | One completed accessible YouTube recording, audio-only AAC/MP4, unique original track when metadata exists | Playlists, live recording, captions, WebM/Opus-only, DRM, account/age/region authorization |
| Metadata | Author-description timestamp/title lines from one accessible completed recording; no media selection or ASR | Automatic chapters, linked/prose timestamps and title-before-time forms |
| Output | One result JSON; stderr JSONL; canonical transcript and exact raw ASR; compact/metadata/full modes and offline exports | Word accuracy, source-language detection, diarization or complete recognition of every utterance |
| Runtime | Local pinned GigaSTT 2.22.0, GigaAM v3 e2e_rnnt INT8; syom/bandlimit/ryf media path | Entire ASR stack being pure Rust: ONNX Runtime has native components |
| Platform | Linux x86_64 GNU on the tested host/network; local filesystem with locks/hard links/fsync | macOS Web/Windows/other architectures, all Linux distributions/filesystems/proxies/CDNs |
| Duration | No fixed recording-duration/file-size cap; disk-streamed media and bounded chunks | Infinite resources: index/checkpoint metadata, retained disk, integer/filesystem and per-JSON-unit limits remain |
| Recovery | Typed temporary acquisition failures, source/token refresh, compatible-prefix proof, durable retry state and explicit process resume | Resurrecting deleted/inaccessible bytes, bypassing authorization or restarting a dead process without an agent |
| Scheduling | New CLI job waits budgeted at 3600 seconds; active work excluded; old saved policies inherited | Overall one-hour runtime cap, an always-on daemon or coordination across unrelated users/hosts |

## Current fixes and evidence

0.8.0 adds metadata-only chapters: author-description timestamp/title pairs with
explicit source and absence/error distinctions. No ASR, model or audio acquisition
is used. Current local network check on 2026-10-30 returned 35 timestamps for
rfscVS0vtbw (16,012 seconds); it is a single watch-page result, not an availability
rate. Loopback tests cover malformed/absent metadata, HTTP403/429, Retry-After,
wrong video identity and the closed result schema. Historical ASR measurements
below retain their original versions. [Usage](usage.md#published-timestamps-without-audio).

0.7.0 establishes independent crate, CLI, cache and job-ownership namespaces.
Manifest schema2 uses application_version; checkpoints require the
application identity. Start a new work/runtime directory; use original binaries
for unfinished earlier jobs. Historical functional results remain tied to their
original builds and are not new 0.7.0 measurements. Hosted Actions are disabled;
checks run locally. Publication grants no additional availability/security assurance.

0.6.1 corrects the schema for fields already emitted by the CLI: processed-audio
scope, timestamp-gap diagnostics, setup, waiting and retry status. Closed objects
remain closed. Regular local Rust tests validate real CLI output/artifacts/events;
Python checks reject malformed variants. No canonical/ASR bytes or job formats
are migrated. [Contract](agent-contract.md) · [schema](../schemas/contract.schema.json).

The current provider snapshot changes only its manifest's paste-to-pastey macro
dependency and private publication flag. Its Rust source matches upstream revision
70f76c039e587192688af18a80d5d6435dedaf22 byte-for-byte, verified from recorded hashes.
Application TLS protections and cryptographic algorithm versions remain intact.
[Provenance](../vendor/README.md) · [verification record](release-061-results.json).

| Evidence | What it proves | Qualification limit |
| --- | --- | --- |
| [0.6.1 local release checks](release-061-results.json) | Current conformance, source identity, local gates and controlled TLS checks | Not an independent security audit |
| Official-model first run, 2026-10-30, 0.6.0 | Fresh source downloaded/decoded/transcribed: 52.96475 s, exact source/raw/transcript hashes and cleanup | One short source; setup archive was local; no accuracy score |
| [0.6.0 automatic recovery](automatic-recovery.md) | Single invocation survives prefix HTTP429/SABR stall; real GigaSTT; old-job reuse | Controlled faults plus one fresh short network sample |
| [0.5.1 corpus](acquisition-051-qualification.md) | 14/15 full acquisitions, 15.571 hours, longest 4.448 hours, two caller resumes | Selected one-network corpus; no ASR, statistical availability rate or SLA |
| [Multi-hour ASR restart](multi-hour-defaults.md) | 3.82 hours, 46 chunks, exact adoption after SIGKILL and validated cleanup | Historical build/model/host; no new 0.6.1 long ASR result |
| [Quality measurements](benchmarks.md) | Dated small reference metrics and boundary sensitivity | GigaSTT 2.21.0; no general conversation accuracy or superiority claim |

The public model release has encoder digest `16805903...` and totals 311.84 MiB.
The first-run check on 2026-10-30 used a separate prepared runtime and verified all
four official hashes, full 847,436-frame decode and publication in 18.453 seconds.
This timing is a single end-to-end observation, not a throughput benchmark. Earlier
model measurements with encoder `cf51b300...` retain their own identities; neither
model set replaces another in an unfinished job. [Installation](installation.md).

## Security and license disposition

The project now supplies the [MIT license text](../LICENSE), matching its existing
manifest declaration and the owner's instruction to remediate the audit. This
does not authorize uploading prebuilt artifacts or publishing a crate. Runtime,
model and third-party licenses/notices remain distinct; collected text is not an
independent legal clearance. [Local release procedure](private-release.md).

The unmaintained paste build dependency is removed in 0.6.1; the selected lockfile
uses pastey 0.2.3. Raw cargo audit still reports
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) on rsa
0.10.0-rc.19. The application refuses certificate private-key loading and uses
no RSA signing/decryption or RSA key transport. Its RSA operations verify public
certificate/handshake signatures. This reachability assessment does not patch the
RSA package or establish all crypto operations secure. No advisory is suppressed.

The selected [upstream provider warning](https://github.com/RustCrypto/rustls-rustcrypto/blob/70f76c039e587192688af18a80d5d6435dedaf22/README.md)
remains in the retained README. RSA key strength, fallible RNG and finite AES-GCM
record policy have been fixed and tested at the application boundary. Independent
review of the configured TLS/crypto path is still needed for an unrestricted
production-security claim. A local review cannot manufacture that external
assurance. The strict pure-Rust requirement is preserved. No native fallback,
certificate bypass, advisory waiver or production-approval assertion is added.

## Operation and rollback

1. Use the [verified installation sequence](installation.md), then run doctor and
   one full source representative of the intended network. Doctor alone proves
   neither inference nor media availability.
2. Use dedicated private work directories and a local writable shared user cache.
   Monitor typed failure codes, full-download outcomes, waiting budgets, retained
   disk/inodes and ASR resource limits. Shared watch pacing is per user/cache,
   not a multi-host capacity controller.
3. Retain failed jobs for resume, including journals, raw ASR, media and the selected
   runtime/models. No age-based deletion policy is supplied: it would destroy the
   promised recovery evidence. Arrange storage/quota alerts; do not delete owned
   media independently to reclaim space.
4. After a process dies, status is authoritative only as a snapshot; wait for idle
   and handle lock contention on resume. Respect server retry guidance. Follow
   [troubleshooting](usage.md#troubleshooting), not unlimited restart loops.
5. Pin a source version by immutable commit/tag, source digest and optional binary
   digest. Keep the previously working bundle/runtime. Before rollback, inspect
   saved-job compatibility: binaries older than 0.6.0 cannot read new retry schema2;
   older media backends may refuse unfinished jobs. Reuse completed validated text
   or finish an incompatible job with its original binary instead of editing state.

The owner authorized public source distribution in a fresh independent repository,
not opening the original private repository. The public branch starts from a reviewed independent source root. Original private-history backups, local operational records, media
and raw evidence stay outside public Git. The old repository remains a private
archive; its cached historical objects are not part of the new repository network.
YouTube denial and unverified ASR accuracy remain product support limits.
