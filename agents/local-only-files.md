## Local-only files

Never commit, force-add, or publish local agent configuration, memories,
sessions, caches, credentials, or private task-tracker data, even to a
private repository. Keep these private-state paths ignored (patterns apply at any depth):

```gitignore
.serena
.aider
.aider.*
.claude-memory-compiler
.codebase-memory
.codebase-memory-*
backlog
.backlog
backlog.config.yml
```

These patterns cover directories, files, and symlinks with those names.
Other tool-generated indexes, databases, logs, session transcripts, and
machine-specific MCP configs are local-only too: identify their actual
paths before creating them and add precise ignore rules.

Real `.env` files, credentials, private keys, and tokens must stay local.
Keep sanitized examples only when the project explicitly uses them.
Do not print secret-bearing file contents while checking.

Classify mixed client directories (`.grok`, `.claude`, `.codex`, `.cursor`,
`.kimi`, `.kimi-code`, `.agents`, `.omc`, `.omx`, `.superpowers`, `.gstack`)
and `.mcp.json` by content before writes. Sessions, credentials, caches,
and machine-specific config are always local-only. Identify their precise
paths and protect them, including future generated state, before tool init.
Do not print secret-bearing values while inspecting configuration.

Portable, sanitized team instructions and shared config may be tracked when
project policy explicitly identifies them as shared. Unknown content is
unclassified, not automatically safe: inspect or report a conflict. Do not
unignore existing directories or untrack shared files automatically.
`AGENTS.md`, `CLAUDE.md`, `agents/`, product specifications, and accepted
sanitized decisions are normally shared. Keep private task drafts separate.

Before staging, inspect `git status --short` and `git diff --cached
--name-only`, then check `git ls-files` for forbidden paths. Ignore rules
do not remove files already tracked. Report tracked violations by path;
never delete local data or rewrite history as part of setup. Removing
tracked files from the index needs an explicit cleanup scope. Do not
commit or publish while violations remain.
