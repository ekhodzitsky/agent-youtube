<!-- agent-dev-kb:coding-principles sha256=812b4079cb88a1e24ce24ff6107bfe123f099ad2f3a994a705c3fec8a5745815 -->
These principles apply throughout implementation, review, and verification.
Use the project's existing task workflow; Backlog.md applies when configured.
Do not introduce a competing task or spec framework.

### Continuous application

- Read applicable repository instructions, linked principles, and local
  constraints before changing code. Reload after context loss, a change of
  repository or scope, or an update to those instructions.
- Identify the affected contracts, invariants, failure modes, and checks.
  Keep this proportional to risk; a small change needs no separate document.
- Preserve these constraints while implementing. Review the final diff for
  violations, including changes to tests, dependencies, and build settings.
- Never weaken checks, suppress diagnostics, or update expected results merely
  to make a failing change pass. Explain intentional contract changes.
- Keep justified exceptions narrow, explicit, and documented at their use.
  Follow repository approval requirements; do not invent additional gates.
- Verify the final code state. Report skipped checks, pre-existing failures,
  and unresolved risks. Do not claim guarantees beyond the evidence.

### Design and scope

- State material assumptions. Resolve consequential ambiguity before dependent
  work; proceed with reasonable defaults for reversible implementation details.
- Prefer the simplest complete solution. Avoid speculative features, config,
  and abstractions. Optimize for clarity and maintenance, not line count.
- Make the smallest complete change: include necessary callers, tests, and
  documentation. Match local style; avoid unrelated cleanup.
- Encode meaningful invariants in types and visibility where practical.
  Inspect every construction path, including builders and deserialization.
  A validated value does not prove that mutable external state stays valid.
- Keep domain logic independent of I/O where useful. Make time, randomness,
  and scheduling controllable when correctness depends on them. Do not create
  interfaces or crates solely to satisfy an architectural pattern.
- Treat public APIs, CLI behavior, serialized data, and storage formats as
  contracts. Document intentional incompatibility and its migration path.

### Failure and resource handling

- Validate untrusted input at boundaries. Never silently discard errors or
  substitute defaults when that hides invalid input or partial failure.
- Define observable state after errors, cancellation, and partial completion.
  Returning an error does not establish that an external action did not occur.
- Retry only when repeated effects are safe; bound attempts and elapsed time.
  Preserve useful error context without exposing secrets or duplicating logs.
- Bound input sizes, buffering, concurrency, and other resources exposed to
  untrusted or growing workloads. Define behavior when a limit is reached.
- Give resources and background work explicit owners and shutdown behavior.
  Check fallible completion explicitly; cleanup alone cannot report success.
- Distinguish atomic visibility from durability. Test the failure guarantees
  actually promised by the storage or external-service contract.

### Verification and evidence

- Use checks that exercise observable contracts, boundaries, and failure modes.
  Reproduce a bug with a regression test when feasible, then demonstrate the fix.
  Do not add tests that merely restate implementation or trivial edits.
- For critical logic, assess whether tests detect plausible wrong behavior;
  coverage alone is insufficient. Use targeted mutation testing when useful.
- Preserve minimized fuzz inputs, seeds, or event sequences as reproducible
  regressions, preferably in the normal test suite. Reruns are not a flaky-test fix.
- Select additional checks by risk: property/fuzz tests for parsers and state
  machines; fault injection for partial failures; schedule exploration for
  concurrency; Miri for exercised unsafe paths; bounded proofs for suitable
  critical components. Document model limits; none proves the whole system.
- Measure performance changes against a relevant baseline and realistic workload.
  Include correctness and resource costs; do not optimize from intuition alone.
- Run repository gates and checks relevant to affected supported configurations.
  After further edits, rerun affected checks; distinguish passed from not run.
- Justify new dependencies by capability, maintenance, features, transitive cost,
  licensing, security, and toolchain compatibility. Reuse is not automatic trust.
- Keep secrets and external tracker ids out of changes. Use the repository's
  language for comments and commits; otherwise use English.

### Rust

- Prefer safe Rust. Deny `unsafe_code` by default; use `forbid` for crates that
  must remain entirely safe. These settings do not ban unsafe in dependencies.
- Any permitted unsafe must be isolated and necessary, with a local safety
  argument covering its obligations. Document unsafe API caller obligations.
  Require evidence for performance justifications; never bypass ownership or
  thread-safety requirements to make code compile. Use explicit unsafe blocks
  inside unsafe functions and run applicable Miri tests; a pass is not proof.
- Deny Clippy `unwrap_used` and `expect_used` on production code. Test-only use
  may be allowed by the repository. A rare production exception requires a
  locally established invariant and a narrowly scoped lint exception.
- Review other panic paths too: indexing, assertions, arithmetic, and explicit
  panic. Expected input and operational failures belong in `Result`/`Option`.
  Do not hide errors with `.ok()`, discarded results, or fallback defaults.
- Enforce `unused_must_use`; use typed errors at reusable boundaries and useful
  context at application boundaries. Do not use catch-unwind as normal recovery.
- Prefer clear ownership and borrowing. Use `Clone`, `Arc`, and interior
  mutability deliberately; neither ban them nor use them to avoid design work.
- Keep locks short and avoid holding synchronous lock guards across `.await`.
  Choose mutex types from actual contention and suspension needs.
- Make async cancellation, task supervision, backpressure, and shutdown explicit.
  Dropping a handle or timing out does not establish that work stopped; already
  running blocking work may continue. Inspect the APIs' cancellation guarantees.
- Maintain `rust-version` and test affected supported features, targets, and
  MSRV. `--all-features` alone is not a feature matrix; test valid combinations.
  Use API compatibility checks for libraries, plus behavioral contract tests.
- Keep formatting and lint checks reproducible with the repository toolchain.
  Select useful Clippy lints; do not enable all restriction lints indiscriminately.
  Document local lint exceptions and avoid blanket crate-wide allowances.

### Minimal code
<!-- Ladder adapted from ponytail (MIT), github.com/DietrichGebert/ponytail -->
Choose the first suitable option that preserves the required contracts:
1. Is the behavior needed? If not, omit it.
2. Can existing project code supply it clearly? Reuse it.
3. Does the standard library fit the requirements? Prefer it.
4. Does an existing dependency or platform facility fit safely and portably?
   Compare their actual costs and guarantees; avoid duplicating working code.
5. Otherwise, add the smallest clear implementation or justified dependency.
Never cut trust-boundary validation, data-loss handling, security,
accessibility, or required verification for brevity.
<!-- /agent-dev-kb:coding-principles -->
