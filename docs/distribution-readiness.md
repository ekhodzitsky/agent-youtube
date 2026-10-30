# Distribution preparation — 2026-10-29

For current support, dependency/license disposition and release procedure, see
[Current readiness](readiness.md). Dated evidence below retains its original scope.

Dependency/advisory counts below are the original preparation snapshot. The
[0.5.0 TLS update](tls-provider-scope.md) removes the crypto-bigint finding from
the selected graph, retains the RSA finding and does not grant publication or
unrestricted production security approval.

Public distribution remains unapproved. The current private candidate is described
in [release readiness](release-readiness.md); [audit evidence](distribution-audit.json)
records the additional privacy, notice and advisory checks below.

## Privacy and Git history

Gitleaks v8.30.1 scanned all local Git refs (75 commits) and an exported HEAD using
its default rules, full redaction and disabled `gitleaks:allow` comments. The
Linux scanner archive was verified against the official release asset SHA-256.
No scan or repository content was uploaded to a scanning service.

Each scan reports six matches. Three are public attestation API-key constants,
matched against the public [BgUtils source](https://github.com/LuanRT/BgUtils) at
the revision retained in the evidence; three are CSS parser token enumeration
strings in the bundled DOM. Historical matched lines were also checked, without
printing key values. No private credential was confirmed among those matches.
No scanner allowlist was added: scan exit status still reports findings. Pattern
scanning cannot establish absence of all credentials or confidential information.

Git history contains **144 Backlog files and one `.grok/config.toml`**. They are
absent from the current index but remain private operational records. Do not
make this existing Git history public. A separately reviewed source snapshot
without private history is the safer publication candidate; this audit neither
creates a public repository nor rewrites or deletes the existing private history.
Ignored runs, media, transcripts, models and agent state must stay excluded.

## Exact notice collection

```sh
python3 tools/prepare_notices.py --output runs/distribution-review/notices
```

The collector reads the offline locked Cargo graph and emits a package/text
inventory plus `THIRD_PARTY_NOTICES.txt`. Current result: **257 third-party Rust
packages, 182 unique text entries including bundled-asset notice collections,
zero packages missing collected text**. Three supplemental text files are retained
with exact upstream revisions and digests; local Boa notices are reused. A corrupt
supplement is rejected by a verified negative probe. The bundled BotGuard notices
cover its wrapper and 20 JS packages; the EJS-derived algorithm notice is included.

This is a text-collection result, not a legal determination that every licensing
obligation is satisfied. An unconfigured `cargo deny check licenses` rejects the
graph because no allow policy exists; that output is not evidence that all these
licenses are prohibited. License alternatives, attribution packaging and any
intended distribution still require an explicit policy decision.

Cargo declares MIT. A concrete [root license text](../LICENSE)
uses the same owner/year attribution as the owner's existing MIT codec projects.
At this historical snapshot it was a draft. The owner-authorized 0.6.1
remediation supplies it as root LICENSE; repository publication remains prohibited.

The agent-youtube notices collection does not bundle GigaSTT, ONNX Runtime or model
weights. GigaSTT has its own notices; upstream [GigaAM](https://github.com/salute-developers/GigaAM)
and its [v3 model card](https://huggingface.co/ai-sage/GigaAM-v3/blob/ctc/README.md)
declare MIT, but the exact provisioned model/runtime artifacts and their notices
must accompany any decision to redistribute those assets. Optional diarization
and benchmark data have separate terms and are outside this candidate bundle.
The shipped codec fixture is synthetic and has recorded MIT provenance in
[its fixture record](../tests/fixtures/README.md).

## Dependency security disposition

The two advisory entries remain unresolved in the locked graph:

| Finding | Upstream status | Current application boundary |
| --- | --- | --- |
| [crypto-bigint RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | Patched in >=0.7.6; not a compatible leaf replacement for the current curve dependency family | Client key exchange is restricted to checked X25519; affected NIST secret operations are excluded from the configured client path |
| [rsa RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | No patched version listed | Private-key loading/signing is disabled; RSA certificate-signature verification remains |

Source paths and existing regression tests confirm those configured boundaries;
they do not prove the crypto implementation secure. Raw audit still fails.
No advisory suppression or unverified dependency override was introduced.
Changing crypto families or removing certificate algorithms needs separate TLS
compatibility and security qualification; it is not a safe one-line version bump.
The provider remains experimental. See [the implementation record](development.md).

Publication therefore still needs: an explicit public-distribution policy and
root license decision, a history-free reviewed source candidate, completed
redistribution-policy review, and a security decision on the provider/advisories.
Production availability and ASR support limits from the release report remain.
