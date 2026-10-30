# Release 0.3.13 readiness

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

[Managed runtime setup in 0.4.0](runtime-setup.md) has separate qualification
evidence; the original release report below retains its dated scope.

For the subsequent artifact incident, corrected executable and TLS classification,
see [release 0.3.14 verification](release-artifact-restoration.md). This page
retains the original 0.3.13 qualification scope.

This is a locally qualified candidate for controlled private operation on Linux
x86_64, using GigaSTT 2.21.0 and the existing GigaAM v3 e2e_rnnt INT8 models.
It is not a declaration of unrestricted YouTube availability or public-release
approval. Repository visibility, `publish = false` and manual-only workflows
remain in force.

## Behavior and compatibility

Unfinished jobs with a different media backend now fail with
`checkpoint_settings_mismatch`, `retryable: false`, before retry policy,
invocation control or recognizer preflight changes saved files. Validation runs
under the job locks. Tests compare saved file paths and hashes and confirm that
GigaSTT was not invoked. A separate cross-build probe exercises an old job against
an isolated patched-upstream candidate and confirms refusal without mutation.

A completed validated transcript remains reusable across media backends without
recognition. Same-backend unfinished jobs retain the existing checkpoint schemas
and audio identity. syom/bandlimit production forks remain pinned: the earlier
[codec comparison](../tools/upstream-codecs/migration.md) found cross-build PCM
differences. The new early-refusal fix closes its bookkeeping-mutation finding;
it does not establish cross-version codec equivalence.

## Qualification — 2026-10-29

The [sanitized evidence](release-results.json) records exact binary/source/output
hashes and outcomes. Raw audio, transcripts, ASR JSON and logs remain local.

| Check | Result / scope |
| --- | --- |
| Local gates | fmt, strict Clippy, 330 debug tests and 330 release tests passed; 14 opt-in tests ignored |
| Minimum Rust | Rust 1.94 locked all-target check passed |
| Live short source | Full 52.96475 s recording downloaded and recognized; canonical and retained ASR hashes verified |
| Hard process death | SIGKILL after a durable 163,880-byte download prefix; separate process resumed a full 1,053.8376875 s recording |
| Publication and cleanup | Both live jobs validated transcript/manifests, retained exact raw ASR and cleaned owned media after publication |
| Multi-hour media | 7,200 s synthetic decode/restart passed with identical PCM hashes; peak test-process RSS 38.3 MiB; no two-hour speech recognition |
| Dependency audit | Two unresolved RustSec findings: crypto-bigint 0.5.5 and rsa 0.9.10; raw audit does not pass |
| License inventory | All 258 resolved package versions declare license expressions; redistribution notices and model terms are not thereby cleared |

The [subsequent 15-video acquisition sweep](acquisition-qualification.md)
verified 14 complete downloads and recorded one source-unavailable refusal.
Those acquisition trials do not run ASR. Two live transcription successes are
observations, not a representative access success rate.
ASR completeness/accuracy is not established by JSON validation. Tests cover
HTTP failures, expiry, recovery and publication boundaries; these do not make
all future upstream failures recoverable. macOS and other platforms are outside
this release qualification.

## Operation

Build with `cargo build --release --locked`, provision the tested recognizer and
models using [installation instructions](installation.md), then run
`target/release/agent-youtube doctor`. Keep failed job directories for `resume`; use
`status` to distinguish resumable failure, active work and validated completion.
Use a new work directory after an incompatible-backend refusal. Retain the old
binary when unfinished old jobs must be completed. Do not treat `doctor` as a
live-access or accuracy test. Monitor typed JSON failures, retained-job disk use
and complete-download outcomes in the actual deployment network.

The two-hour decoder probe is reproducible locally:

```sh
AGENT_YOUTUBE_PROFILE_SECONDS=7200 cargo test --release long_decode_resume_profile -- \
  --ignored --exact audio::tests::long_decode_resume_profile --nocapture
```

## Public distribution prerequisites

Public release remains blocked by the explicit private-repository policy.
Preparing this candidate does not authorize changing visibility, publishing a
crate, or dispatching hosted workflows. Before distributing, settle the root
license grant (Cargo declares MIT but there is no root LICENSE), review all
redistribution notices/model terms, and review Git history and packaged artifacts
for private data. The subsequent [distribution audit](distribution-readiness.md) scanned local Git refs and classified scanner findings; it found private historical records, so the existing history must remain private.

The experimental RustCrypto TLS provider and the two remaining dependency
advisories require explicit security review for the intended deployment.
The [subsequent TLS review](tls-security-review.md) confirms that upstream
explicitly warns against production use; this candidate is not production
security-approved. The tested pure-Rust alternative loses RSA-chain support. Current
configuration disables affected private-key/NIST-secret paths and uses X25519;
that containment is documented in [development notes](development.md), and does
not make the dependency audit clean. Broader acquisition qualification remains
unfinished; there is no production availability SLA. Coverage is unmeasured.
These limitations must accompany any release readiness decision.
