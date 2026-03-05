# Changelog

All notable changes to codescout are documented here.

## [0.2.0] — codescout

> **TL;DR:** The project was renamed from `code-explorer` to `codescout`. If you're
> migrating, update your MCP config and any scripts that reference the old binary name.
> [Full story and migration guide →](docs/manual/src/history.md)

### Breaking changes

- **Binary renamed:** `code-explorer` → `codescout`
- **MCP server ID renamed** — update `.mcp.json` or Claude Code settings accordingly
- **Tool renames** (API consistency):

| Old name | New name |
|---|---|
| `get_symbols_overview` | `list_symbols` |
| `find_referencing_symbols` | `find_references` |
| `replace_symbol_body` | `replace_symbol` |
| `insert_before_symbol` + `insert_after_symbol` | `insert_code` (+ `position` param) |
| `execute_shell_command` | `run_command` |
| `create_text_file` | `create_file` |
| `search_for_pattern` | `search_pattern` |
| `search_code` | `semantic_search` |
| `index_stats` | `index_status` |
| `get_current_config` | `get_config` |
| `check_onboarding_performed` | `onboarding` |

- **Tool consolidations** — `insert_before_symbol` + `insert_after_symbol` merged into
  `insert_code(position)`, `is_onboarded` folded into `onboarding(force)`

### Added

#### New tools
- `goto_definition` — LSP-backed jump to symbol definition; auto-discovers libraries
- `hover` — LSP type info and doc comments at a given position
- `edit_lines` — line-based splice edit (replace/insert/delete by 1-indexed position)
- `insert_code` — insert code before or after a named symbol (replaces the separate
  `insert_before_symbol` / `insert_after_symbol` tools via a `position` parameter)
- `list_libraries` — list all registered external libraries and their index status
- `index_library` — build embedding index for a registered library

#### Library search
- Symbol tools (`list_symbols`, `find_symbol`, `find_references`, `goto_definition`,
  `hover`) and `semantic_search` now accept a `scope` parameter: `"project"` (default),
  `"libraries"`, `"all"`, or `"lib:<name>"` for a specific library
- `LibraryRegistry` — persistent registry; libraries auto-registered via `goto_definition`
  when definitions resolve outside the project
- Manifest discovery auto-registers `Cargo.toml` / `package.json` / `go.mod` paths as
  named libraries

#### Semantic search improvements
- Incremental index: hash-based change detection (git diff → mtime → SHA-256 fallback);
  only changed files are re-indexed
- Semantic drift detection in `index_status` — surfaces files whose content has drifted
  significantly from their indexed embeddings
- sqlite-vec extension replaces hand-rolled Rust cosine loop for distance computation
- AST-aware chunker splits files by symbol boundaries before embedding
- `local-embed` feature flag: fastembed-rs LocalEmbedder for CPU-only inference,
  no Ollama required
- CPU fallback: automatically switches to local model when Ollama is unreachable
- Concurrent embedding with single-transaction writes for faster indexing
- `.cjs` and `.mjs` files now indexed as JavaScript

#### Progressive disclosure
- `OutputGuard` module enforces two output modes across all list/search tools:
  - **Exploring** (default): compact, capped at 200 items, overflow hint included
  - **Focused**: full detail with `detail_level: "full"` + `offset`/`limit` pagination
- `read_file` capped at 200 lines in exploring mode; explicit `start_line`/`end_line`
  bypasses the cap
- `next_offset` field in overflow JSON for seamless pagination

#### Robustness & DX
- Recoverable errors (`RecoverableError`) return `isError: false` with a `hint` field —
  sibling parallel tool calls are not aborted when one tool returns an expected error
- Dynamic server instructions injected into the MCP `initialize` response so Claude
  sees guidance before the first tool call
- `system_prompt` field in `.codescout/project.toml` for project-specific guidance
- Auto-detect project root from the server's working directory on startup
- Configurable per-language LSP init timeout via `lsp_init_timeout_secs`
- `text_sweep` helper: after `rename_symbol`, scans for residual textual occurrences
  (comments, strings, docs) that LSP rename cannot reach
- JetBrains official `kotlin-lsp` replaces community `kotlin-language-server`
- TSX/JSX LSP support via `typescript-language-server`
- tree-sitter support for Java, Kotlin, and TSX
- E2E test fixture projects with TOML-driven data harness
- Windows support: path separators, home directory in security checks, cmd.exe shell

#### Security
- Path sandboxing: all reads/writes validated against project root
- Tool category access controls (read, write, git, index, shell) configurable per-project
- Platform-specific deny-list (SSH keys, `/etc/passwd`, Windows credential stores)

### Changed

- `read_file` now rejects source code files (`.rs`, `.py`, `.ts`, etc.) — forces use of
  symbol tools; pass `start_line`/`end_line` only on non-source files
- `onboarding` redesigned: produces richer project context and memory-creation guidance
- Tool count: 33 → 30 (consolidation of insert tools, removal of git_log/git_diff)
- Default embedding model: `ollama:mxbai-embed-large`

### Removed
- `git_log` tool — use `run_command` with `git log` for file history
- `git_diff` tool — use `run_command` with `git diff` for diffs
- `replace_content` tool — superseded by `replace_symbol` and `edit_lines`

### Fixed
- Ghost blank lines in `replace_symbol` and `insert_code` when replacement body contains
  a trailing newline (`.push(body)` → `.extend(body.lines())`)
- `write_lines` empty-output guard: no longer writes `"\n"` when result is empty
- 1-indexed line numbers in all symbol/AST tool outputs (`start_line`, `end_line`)
- Concurrent `semantic_search` deadlock when multiple calls hit a cold LSP simultaneously
- LSP thundering-herd race condition on cold start (watch-channel barrier)
- LSP deadlock in waiter-retry path and excessive lock hold during shutdown
- Graceful LSP shutdown prevents orphaned language server processes
- `search_pattern` returns `RecoverableError` for invalid regex (not a hard error)
- Char-safe truncation in drift snippets (prevented panic on multibyte Unicode)
- HTTP timeout wired through to embedding client
- Hidden directories (`.worktrees`, `.claude`) excluded from all file walkers
- `git_blame` reads committed content correctly; better error for dirty files
- `SecuritySection::default()` now enables write/git/indexing tools (was too restrictive)

---

## [0.1.0] — 2026-02-25

### Added

#### Core MCP server
- Rust MCP server (`rmcp` 0.1) with 29 tools across 8 categories
- Stdio and HTTP/SSE dual transport — stdio for Claude Code, HTTP for multi-session use
- Library + binary split (`src/lib.rs`) enabling integration tests and external use
- Release profile: `opt-level 3`, thin LTO, symbol stripping

#### File tools (3)
- `read_file` — read files with optional line range
- `list_dir` — directory listing, recursive mode
- `search_for_pattern` — regex search across project files

#### Workflow tools (3)
- `execute_shell_command` — run shell commands in project root
- `create_text_file` — create or overwrite files
- `find_file` — glob pattern file discovery

#### Symbol tools — LSP-backed (7)
- `get_symbols_overview` — hierarchical symbol tree for a file or directory
- `find_symbol` — workspace-wide symbol search by name pattern
- `find_referencing_symbols` — find all usages of a symbol
- `rename_symbol` — rename across the whole workspace
- `replace_symbol_body` — replace the body of a symbol
- `insert_before_symbol` / `insert_after_symbol` — precise code insertion
- JSON-RPC 2.0 LSP client with async stdio transport, 30s timeout, crash recovery
- Language server configs for 9 languages: Rust, Python, TypeScript/JS, Go, Java,
  Kotlin, C/C++, C#, Ruby

#### AST tools — tree-sitter offline (2)
- `list_functions` — extract all function/method signatures
- `extract_docstrings` — extract doc comments with associated symbol names
- **Rust** (`tree-sitter-rust`): functions, structs, enums, traits, impl methods,
  modules, constants — `///` and `//!` doc comments
- **Python** (`tree-sitter-python`): functions, classes, methods, decorated definitions
  — triple-quoted docstrings
- **Go** (`tree-sitter-go`): functions, methods with receiver type, structs, interfaces
  — `//` and `/* */` comments
- **TypeScript** (`tree-sitter-typescript`): functions, classes, interfaces, enums,
  type aliases, export statements — JSDoc `/** */`
- **TSX** (`tree-sitter-typescript` LANGUAGE_TSX): full JSX grammar — same extraction
  as TypeScript
- **Java** (`tree-sitter-java`): classes, interfaces, enums, records, methods,
  constructors, fields, enum constants — Javadoc `/** */`
- **Kotlin** (`tree-sitter-kotlin-ng`): classes, objects, functions, properties, enums,
  companion objects, type aliases, enum entries — KDoc `/** */`

#### Git tools (3)
- `git_blame` — per-line blame with commit SHA, author, timestamp
- `git_log` — file commit history
- `git_diff` — working tree or commit-range diff

#### Semantic search tools (3)
- `search_code` — vector similarity search over indexed codebase
- `index_project` — build/update embedding index (chunked, content-hashed)
- `index_status` — show index stats and coverage

#### Memory tools (4)
- `write_memory` — store named notes per project
- `read_memory` — retrieve a note by topic
- `list_memories` — list all stored topics
- `delete_memory` — remove a note

#### Config tools (2)
- `activate_project` — switch active project root
- `get_current_config` — show config and project root

#### Onboarding tools (2)
- `onboarding` — project discovery: detect languages, structure, create config
- `check_onboarding_performed` — check if onboarding has run

### Infrastructure
- `.mcp.json` — Claude Code MCP config for using code-explorer on its own source
- 141 tests: 136 unit + 5 end-to-end integration tests
- Integration tests cover: read→search→replace, AST analysis, memory+config roundtrip,
  git history creation, onboarding+explore
