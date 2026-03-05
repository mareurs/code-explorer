# Gotchas & Known Issues

## Tool Behavior

- **`find_symbol(include_body=true)` body truncation** — LSP `workspace/symbol` returns the *name position* (single line), causing `start_line == end_line` and a body with only the signature. Workaround: use `list_symbols(path)` first to get correct ranges, then `find_symbol(name_path=..., include_body=true)`. Top-level functions in large files: use `list_symbols` spans directly.

- **`edit_file` blocked on source files** — Multi-line structural edits on `.rs`/`.py`/`.ts` are blocked. Use `replace_symbol`, `insert_code`, `remove_symbol` instead. `edit_file` is for imports, literals, comments, config files.

- **`run_command` piping blocked** — Never `run_command("cmd | grep X")`. Run bare, then query the `@cmd_*` buffer: `run_command("cmd")` → `run_command("grep X @cmd_id")`.

## Plugin / Hooks

- **code-explorer-routing blocks Read/Grep/Glob on source files** — Will get `PreToolUse hook error`. Use codescout MCP tools instead. Non-source files (`.md`, `.toml`, `.json`, `.sh`) are fine.

- **Bash tool fully blocked** — All Bash calls denied by `pre-tool-guard.sh`. Use `mcp__codescout__run_command` exclusively.

- **`block_reads` config requires string "false"** — In `.claude/code-explorer-routing.json`, set `"block_reads": "false"` (string, not boolean). jq's `//` treats boolean `false` as falsy.

## Git / Worktrees

- **`activate_project` required after EnterWorktree** — Write tools silently target the main repo unless you switch the active project. Always call `activate_project(worktree_path)` after `EnterWorktree`.

- **`finishing-a-development-branch` cleanup** — Can fail when Claude's CWD is inside the worktree. Use `git worktree prune` from main repo root instead.

## Embedding

- **Remote embeddings need API/Ollama** — Default build uses `remote-embed` feature. Needs `OPENAI_API_KEY` or Ollama running. Use `--features local-embed` to avoid external dependency.

- **`semantic_search` warns on stale index** — Warning appears when index is behind HEAD; run `index_project` to rebuild. Incremental rebuilds are fast (git diff → mtime → SHA-256).

## Build

- **`panic = "abort"` in release** — Intentional. Panics kill the process immediately so Claude Code triggers a clean reconnect via `/mcp`. Do not remove.

- **E2E tests need real LSP servers** — Gated behind `--features e2e-rust` etc. Excluded from default `cargo test`.
