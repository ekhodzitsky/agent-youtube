# Independent source migration audit — 0.7.0

Scope: publishable source, package/binary/library identity, developer tools,
job/runtime ownership, machine contract, Git references and repository settings.
This audit does not requalify YouTube availability or recognition accuracy.

| Finding | Resolution | Verification |
| --- | --- | --- |
| Executable and library still used predecessor identity | Package/binary agent-youtube; Rust imports agent_youtube; help, examples, release preparer and research callers agree | CLI regression plus all-target Rust checks |
| Renamed locks could permit unsafe mixed-application jobs | Required checkpoint application identity checked before journal replay/recovery; independent locks, scratch and runtime ownership | Missing/foreign identity rejection leaves checkpoint bytes unchanged; existing recovery suite |
| Evidence manifest exposed a product-specific version key | Manifest schema2 uses evidence.application_version | Emitted artifact schema-conformance tests |
| Cache and test/build flags carried previous names | Independent cache directory and AGENT_YOUTUBE environment namespace | Request pacing, worker, process and fixture tests |
| Signed test certificates/messages contained earlier branding | Public fixtures regenerated with OpenSSL; private generation keys stay ignored | RSA strength, signature, trust/name/expiry and handshake tests |
| Source report labels could imply current builds produced old measurements | Historical summaries use normalized/neutral identities; original values/digests and unedited local backups retained | Recorded source hash inventory; historical ASR/transcripts not rewritten |
| Public tags/history exposed the previous identity | Owner-authorized independent root and new version tag replace earlier public refs | Cold-clone refs, commit count and tracked text/binary scan |
| Operational state could leak through source publication | Local tracker, runtime, logs, models, media and backups remain ignored; source distribution excludes them | Tracked inventory, ignore regression tests and source archive checks |
| Repository visibility or hosted CI could drift | Active source public and independent; predecessor privately archived; Actions disabled, workflow dispatch-only | Repository API and workflow-run inventory |

## Integration and storage boundary

Invoke agent-youtube and use agent_youtube for Rust imports. Start a new work and
managed-runtime directory. Earlier unfinished jobs belong to their original
binary; do not edit ownership markers or rename lock files to adopt them. Offline
export of completed canonical transcripts does not require acquisition or ASR.
Exact upstream ASR and canonical transcript bytes remain private and unchanged.

Other response/artifact schema versions remain independent of manifest schema2.
The new source identity changes build fingerprints; old binaries and historical
reports cannot serve as clean producer identity for a new release artifact.

## Evidence and limits

Detailed local commands, scanner reports, removed-object inventory, previous
source/history backups and repository API receipts remain in the ignored local
tracker and runs directories. Portable reports retain historical commit/digest
identifiers for provenance; these are not promises that old commits are reachable
in the independent public branch. Current source has no dependency on the private
archive or its local filesystem location.

Ref replacement removes old history from advertised branches/tags and fresh
clones. It cannot certify erasure of copies already downloaded or provider-side
cached objects. No production-security approval, new ASR accuracy score or
unconditional YouTube access guarantee follows from the migration.
