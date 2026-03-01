code-explorer MCP server: high-performance semantic code intelligence.
Provides file operations, symbol navigation (LSP), AST analysis (tree-sitter),
git blame, semantic search (embeddings), and project memory.

**Subagents and spawned agents SHOULD use code-explorer too.** If you spawn a subagent or delegate to another agent, instruct it to use code-explorer tools for all code navigation — do not fall back to native Read/Grep/Glob on source files.

## How to Choose the Right Tool

### Navigate code

**You know the name → structure-aware tools:**
- `find_symbol(pattern)` — locate by name substring. Also accepts `name_path` (e.g. 'MyStruct/my_method').
  Pass `kind` to narrow: `"function"`, `"class"`, `"struct"`, `"interface"`, `"type"`, `"enum"`, `"module"`, `"constant"`.
- `list_symbols(path)` — symbol tree for file/dir/glob. Single-file mode caps at 100 top-level symbols.
- `find_references(name_path, path)` — find all usages
- `goto_definition(path, line)` — jump to a symbol's definition via LSP. Auto-discovers libraries.
- `hover(path, line)` — get type info and documentation for a symbol at a given position. Complements find_symbol (name lookup) and goto_definition (navigation).
- `list_functions(path)` — quick function/method signatures (tree-sitter, no LSP)
- `list_docs(path)` — extract all docstrings and doc comments from a file (tree-sitter)

**You know the concept → semantic search:**
- `semantic_search(query)` → then drill down with `list_symbols` / `find_symbol(include_body=true)`

**You know nothing → start with the map:**
1. `list_dir(path)` → 2. `list_symbols(file)` → 3. `semantic_search("what does this do")`

**Search by text or filename:**
- `search_pattern(pattern)` — regex search across files. Pass `context_lines` for merged context blocks around matches. Scope with `path=<file_or_dir>`, limit with `max_results` (default 50).
- `find_file(pattern)` — glob-based file search (e.g. `**/*.rs`, `src/**/mod.rs`). Scope with `path=<dir>`, limit with `max_results` (default 100).

**Non-source files & history:**
- `read_file(path)` — read a file. Returns content directly for short files,
  smart summary + `@file_*` ref for large files (>200 lines). For source code
  files the summary includes top-level symbols. Prefer `list_symbols` /
  `find_symbol` for source code navigation — they are more structured and
  token-efficient.
- `git_blame(path)` — who last changed each line and in which commit

**List directory contents:**
- `list_dir(path)` — list files and directories. Pass `recursive=true` for a full tree.

**Run shell commands:**
- `run_command(command)` — execute a shell command. Run freely even if output
  might be large; the buffer handles it. Returns content directly for short
  output, smart summary + `@cmd_*` ref for large output.
  **Runs from the project root automatically** — no `cd` prefix needed.
  Stderr is captured automatically — no `2>&1` needed.
  - `cwd` — run from a subdirectory (relative to project root)
  - `acknowledge_risk` — bypass safety check for destructive commands
  - `timeout_secs` — max execution time (default 30)

Anti-patterns (never do these):
- ❌ `cd /home/user/proj && cargo test` → ✅ `cargo test`
- ❌ `cargo test 2>&1 | tail -20` → ✅ `cargo test` then `tail -20 @cmd_id`
- ❌ `cargo test 2>&1 | grep FAILED | head -20` → ✅ `cargo test` then `grep FAILED @cmd_id`
- ❌ `cat src/main.rs` → ✅ `read_file("src/main.rs", start_line, end_line)` or `list_symbols("src/main.rs")`
- ❌ `head -20 lib.py` → ✅ `read_file("lib.py", start_line=1, end_line=20)`
- ❌ `sed -n '1,50p' main.ts` → ✅ `read_file("main.ts", start_line=1, end_line=50)`
- ❌ `awk '{print}' server.go` → ✅ `search_pattern(regex)` or `find_symbol(name)`

Shell access to source files (`.rs`, `.py`, `.ts`, `.go`, etc.) is blocked — use code-explorer symbol tools instead. Pass `acknowledge_risk: true` to bypass if absolutely necessary.

### Edit code

- `replace_symbol(name_path, path, new_body)` — replace entire symbol body (preferred for code)
- `insert_code(name_path, path, code, position)` — insert before or after a named symbol
- `edit_file(path, old_string, new_string, replace_all?)` — find-and-replace: locates old_string in the file and replaces it with new_string. Must match exactly (whitespace-sensitive). Fails if not found; fails if multiple matches unless replace_all is true. Empty new_string deletes the match. Alternatively, use insert: "prepend" or "append" to add text at the start or end of the file without a string match.
- `remove_symbol(name_path, path)` — delete a symbol entirely, including its doc comments and attributes
- `create_file(path, content)` — create or overwrite a file

**Prefer symbol tools over `edit_file` for source code:**
| ❌ `edit_file` for… | ✅ Use instead |
|---|---|
| Replacing a function/method/struct body | `replace_symbol(name_path, path, new_body)` |
| Inserting code before or after a symbol | `insert_code(name_path, path, code, position)` |
| Deleting a function, struct, or impl | `remove_symbol(name_path, path)` |
| Renaming a symbol across the codebase | `rename_symbol(name_path, path, new_name)` |

`edit_file` is for non-structural changes only: imports, string literals, comments, config values.
Multi-line edits on source files (`.rs`, `.py`, `.ts`, `.go`, etc.) are blocked — the tool will tell you which symbol tool to use.

### Refactor

- `rename_symbol(name_path, path, new_name)` — rename across the entire codebase via LSP. Sweeps for remaining textual occurrences (comments, docs, strings) that LSP missed. **Warning:** LSP rename may corrupt string literals or macro arguments that contain the old name — always verify changed files compile after use.

### Library code

`find_symbol` auto-discovers libraries. Use `scope: "lib:<name>"` on symbol/search tools.
- `list_libraries` — show registered libraries and their status
- `index_library(name)` — build embedding index for a library

### Other local repositories

- **Quick peek** (few files): use absolute paths — `list_dir`, `read_file`, `list_functions`, `search_pattern` all work without switching projects
- **Deep dive** (symbols, references, semantic search): `activate_project("/absolute/path")` first, explore, then switch back

## Output Modes

Default: **exploring** — compact, capped at 200 items.
Pass `detail_level: "full"` for focused mode with `offset`/`limit` pagination.
Only switch to focused AFTER identifying targets.

Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "...", "by_file": [{"file":"...","count":N},...] } }` — follow the hint.
`by_file` (on `find_symbol` overflow) shows per-file match counts sorted by count descending; use `path=` to zoom into the top file.

## Output Buffers

Large content — whether from a command, a file read, or a large tool response —
is stored in an `OutputBuffer` rather than dumped into your context. You get a
smart summary and an `@ref` handle. The full content costs you nothing to hold.
Query it via `run_command` + Unix tools:

- **`@cmd_*`** — shell command output (from `run_command`). Stderr captured automatically; no `.err` suffix needed for most cases, but `@cmd_id.err` accesses stderr-only when available.
- **`@file_*`** — large file reads (>200 lines, from `read_file`).
- **`@tool_*`** — large tool responses (>10 KB). When any tool response exceeds 10,000 bytes, it is automatically stored in the buffer and you receive a compact summary + ref handle. Query with `run_command("jq '.field' @tool_abc12345")` or `run_command("grep pattern @tool_abc12345")`. Stored as compact JSON (not pretty-printed). No `.err` suffix variant.

Example queries:

    run_command("grep FAILED @cmd_a1b2c3")
    run_command("sed -n '42,80p' @file_abc123")
    run_command("jq '.symbols[] | .name' @tool_abc12345")
    run_command("diff @cmd_a1b2c3 @file_abc123")

**Be targeted:** extract what you need in one well-crafted query per buffer —
don't probe the same `@ref` multiple times for overlapping information.

## Project Management

- `onboarding` — initial project discovery: detect languages, read key files (README, CLAUDE.md, build file), create config, generate `system_prompt_draft`. Returns `features_md` path if found, or `features_suggestion` if not. Use `force: true` to re-scan.
- `activate_project(path)` — switch the active project root. Required after `EnterWorktree`.
- `get_config` — show active project config and server settings
- `index_project` — build or incrementally update the semantic search index
- `index_status` — index stats, staleness, and drift scores. Pass `threshold` to query drift.
- `get_usage_stats` — per-tool call counts, error rates, latency percentiles

### Memory (persistent per-project knowledge)

- `write_memory(topic, content)` — persist knowledge (topic is path-like, e.g. 'debugging/async-patterns')
- `read_memory(topic)` — retrieve a stored entry
- `list_memories` — list all topics
- `delete_memory(topic)` — remove an entry
- `write_memory(topic, content, private=true)` — store in project-local private store (not surfaced in system instructions; use for sensitive or session-specific notes)
- `list_memories(include_private=true)` — returns both shared and private memories in `{ shared: [...], private: [...] }` shape

## Project Customization

If `.code-explorer/system-prompt.md` exists, its contents appear below as
"Custom Instructions" — project-specific guidance from the user. Edit the file
to customize how the AI navigates and works with your codebase.

## Worktrees

After `EnterWorktree`, call `activate_project` with the worktree path — write tools are NOT automatically coupled to the shell's working directory.
If you forget, write tools will silently modify the main repo instead of the worktree — they will include a `"worktree_hint"` field in their response to alert you. When you see that field, call `activate_project` and redo the write.
To clean up: `git worktree prune` from the main repo root, then start a new session.

## Rules

1. **PREFER symbol tools over `read_file` for source code.** `list_symbols` + `find_symbol(include_body=true)` beats reading entire files. `read_file` on a large source file returns a summary + `@file_*` ref, not raw content — use `start_line` + `end_line` when you need a targeted excerpt.
2. **Check `features_md` from `onboarding` before suggesting features.** If the project has a FEATURES.md, read it first — don't propose work that's already done.
3. **Semantic search for "how does X work?"** Then drill into results with symbol tools.
4. **Exploring mode first.** Only `detail_level: "full"` after you know what you need.
5. **Respect overflow hints.** Narrow with `path=`, `kind=`, or a more specific `pattern` — don't repeat broad queries.
6. **Prefer symbol edits** (`replace_symbol`, `insert_code`, `remove_symbol`, `rename_symbol`) for code. Use `edit_file` when symbol tools don't fit.
7. **`run_command` is already in the project root.** Never prefix with `cd /abs/path &&`. Use `cwd` param for subdirectories only.
8. **Don't inline-pipe `run_command` output.** Run the command bare, then query the buffer in a follow-up: `cargo test` → `grep FAILED @cmd_id`. Never `cargo test 2>&1 | grep FAILED`.
9. **Buffer queries return ≤ 200 lines inline.** When querying a `@ref` (e.g. `grep pattern @cmd_id`, `jq '.field' @tool_abc`), output above 200 lines is truncated — the hint shows the exact next-page `sed` command to continue. Do NOT pipe buffer queries (`grep @ref | head`) — run the targeted command directly. For text/markdown files, prefer `read_file(path, start_line, end_line)` over `run_command("cat file") + buffer queries`.
