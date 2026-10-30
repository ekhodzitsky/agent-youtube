# Prepared codec patches

These are review artifacts for upstream maintainers, not production dependencies.
The agent-youtube runtime continues to use the existing vendored codecs. No sibling
repository is modified and no upstream changes have been published.

| Patch | Exact upstream base | Changes |
| --- | --- | --- |
| `syom.patch` | `c458e8a15d57612f4c2b92cbb0347d05c8f893c3` (0.8.1) | Qualified AAC-LC checkpoint state, strict indexed-presentation completion with bounded movie-clock rounding, two regression tests |
| `bandlimit.patch` | `0559b94be9f55cdbf2bf7c87a600ee66c6b2e761` (0.1.7 source) | Write-mode checkpoint state and a restoration regression at seven input rates |

The patches preserve newer upstream PNS seek history, allocation fixes and
resampler kernels. The old Linux slice-move/import changes are unnecessary at
the selected bandlimit base. Do not replace upstream files with our older copies.

From agent-youtube, with read-only sibling source repositories available:

```sh
python3 tools/qualify_vendors.py --prepared-patches --upstream-root /path/to/projects
```

The runner archives those exact commits into ignored `runs/`, checks and applies
the patches there, then runs library tests and the new integration tests. It uses
upstream development dependencies, separate from agent-youtube's runtime graph. Review
individual changes with `git apply --stat tools/upstream-codecs/syom.patch`.

Verified 2026-10-29 on Linux x86_64: syom 1,125 library tests and two new tests
passed (seven existing library tests ignored); bandlimit 142 library tests and
two new tests passed. The new checks compare restored versus continuous output,
including PNS, fractional sample rates and malformed checkpoint rejection.

## Finalization contract and migration requirements

The earlier claim that a 22,063-sample 22,050 Hz input exposes a finalization
bug was incorrect. `output_len` explicitly rejects a non-integral rational
output length with `Overflow`; that input violates the length precondition for
16,000 Hz output. Both our fork and unmodified upstream enforce this contract.

The agent-youtube finalizer already pads input to the reduced-ratio grid, flushes the
filter, and retains exactly `ceil(input_samples * 16000 / input_rate)` samples.
No runtime fix or backend identity change was necessary. The new production
regression checks 1-, 13-, 511-, 512- and rate+13-sample inputs at seven rates,
with checkpoints at the start, middle and EOF. Complete PCM16 output matches
padded one-shot conversion, including the final partial sample, and restored
output matches uninterrupted output. Existing encoded-media resume tests cover
both indexed and fragmented containers.

[The standalone contract regression](bandlimit-finish-contract.rs) reproduces
the expected rejection and then verifies successful padded finalization and
bit-identical one-shot output. Copy it to an isolated upstream checkout's
`tests/upstream_finish_tail.rs` and run
`cargo +stable test --locked --test upstream_finish_tail`. It passes on the
unmodified named upstream base. The prepared checkpoint test now requires
successful padded finalization and exact lengths; matching two errors is no
longer accepted as evidence of completion. This removes the incorrectly
reported finalization blocker, not the migration requirements below.

Checkpoint bytes remain caller-owned implementation state (`YTLC0001`/`YTBL0001`),
not a portable cross-version format. Bind them to the exact input, options and
codec implementation; a source mismatch must refuse restoration. New upstream
kernels may change PCM, so adopting a published patched version requires a new
media-backend identity, cross-version migration decisions and agent-youtube end-to-end
recovery tests. Same-version restoration passing is not cross-version equivalence.
The library-level restore API does not itself authenticate the input file.

No public API stability or release readiness is implied. The reviewed scope is
AAC-LC mono output from supported mono/stereo input and the listed rates; other
profiles retain replay fallback. Full media validation remains the caller's duty.

Strict current-stable Clippy checks of the upstream packages also fail in
unchanged upstream code (slice iteration, initialization and arithmetic style).
Newly introduced lint findings were corrected in the prepared additions, but
this bundle does not claim upstream `clippy -D warnings` passes. agent-youtube's own
required Clippy gate passes; unrelated upstream cleanup is not mixed into these
patches.

## Application qualification

[The isolated agent-youtube comparison](migration.md) records complete PCM, duration,
process-restart recovery, memory and old-job refusal. Production dependencies
remain unchanged; passing library tests alone does not authorize migration.
