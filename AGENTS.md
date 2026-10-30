agent-dev-kb: 0.10.0
# agent-youtube

Core instructions: agent-dev-kb revision `d92256be2cb2c92c686bd5c1de2341a61ba1a0cb`.
The stamp verifies core instructions, not optional MCP connectivity.

Public source repository: agent-youtube. Keep operational data, local tracker state,
credentials and original private-history backups private. Do not publish the crate
or upload release artifacts without an explicit owner request. No changes to other projects.
Use English for code and project records. Preserve exact ASR JSON and transcripts. Download audio only; source/decoded media and chunks are temporary. Clean only owned media after durable validated transcript publication; retain media on failed recognition for resume.

## Backlog.md
This repo uses Backlog.md (not GitHub Issues, not a second spec tool).
Run `backlog instructions overview` before work. Mutate via the CLI
(`--plain` / `--json`). Keep each task focused and record its verification.
Docs → `doc create`; conclusions → `decision create`; work → `task create`.
Labels: `audit`, `research`. `remoteOperations` and `autoCommit` stay false.

## Development branch
Use `main` as the single development branch. Commit and push completed work directly
to `main`. Create a feature branch or pull request only when the owner explicitly asks.
Run development checks locally. Do not trigger GitHub Actions unless the owner explicitly
asks to resume hosted CI. Commits and pushes remain authorized; workflows stay manual-only.

## Project constraints
Use Backlog.md as the only work queue. Keep changes surgical; do not remove unrelated
code or introduce speculative features. No `unwrap` / `expect` on production paths.
Write a failing regression test first when the affected area is already tested.
Skip unnecessary ceremony for trivial changes; preserve all required checks.
Keep internal tracker identifiers out of code, commits, and shared documentation.

## Local-only files
Read [local-only rules](agents/local-only-files.md) before creating or staging artifacts.
Backlog data and `.grok/config.toml` are local-only. Accepted sanitized architectural
records may be shared in project docs; raw research and draft decisions remain local.

## Verification
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
Commit messages follow Lore: intent first, useful native trailers.

## Coding principles

[Coding principles](agents/coding-principles.md) — read before acting.
