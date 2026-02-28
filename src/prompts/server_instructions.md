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
- `list_functions(path)` — quick function/method signatures (tree-sitter, no LSP)
- `list_docs(path)` — extract all docstrings and doc comments from a file (tree-sitter)

**You know the concept → semantic search:**
- `semantic_search(query)` → then drill down with `list_symbols` / `find_symbol(include_body=true)`

**You know nothing → start with the map:**
1. `list_dir(path)` → 2. `list_symbols(file)` → 3. `semantic_search("what does this do")`

**Search by text or filename:**
- `search_pattern(pattern)` — regex search across files
- `find_file(pattern)` — glob-based file search (e.g. `**/*.rs`, `src/**/mod.rs`)

**Non-source files & history:**
- `read_file(path)` — for README, configs, TOML, JSON, YAML. Rejects source code — use symbol tools instead.
- `git_blame(path)` — who last changed each line and in which commit

### Edit code

- `replace_symbol(name_path, path, new_body)` — replace entire symbol body (preferred for code)
- `insert_code(name_path, path, code, position)` — insert before or after a named symbol
- `edit_lines(path, start_line, delete_count, new_text)` — line-level splice for non-code files or when symbol tools don't fit
- `create_file(path, content)` — create or overwrite a file

### Refactor

- `rename_symbol(name_path, path, new_name)` — rename across the entire codebase via LSP. Sweeps for remaining textual occurrences (comments, docs, strings) that LSP missed.

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

Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "...", "by_file": {...} } }` — follow the hint.
`by_file` (on `find_symbol` overflow) shows per-file match counts; use `path=` to zoom into the top file.

## Project Management

- `onboarding` — initial project discovery: detect languages, read key files, create config. Use `force: true` to re-scan.
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

## Project Customization

If `.code-explorer/system-prompt.md` exists, its contents appear below as
"Custom Instructions" — project-specific guidance from the user. Edit the file
to customize how the AI navigates and works with your codebase.

## Worktrees

After `EnterWorktree`, call `activate_project` with the worktree path — write tools are blocked until you do.
To clean up: `git worktree prune` from the main repo root, then start a new session.

## Rules

1. **PREFER symbol tools over read_file.** `list_symbols` + `find_symbol(include_body=true)` beats reading entire files.
2. **`read_file` rejects source code.** Use symbol tools for `.rs`, `.py`, `.ts`, etc. `read_file` is for README, configs, TOML, JSON, YAML.
3. **Semantic search for "how does X work?"** Then drill into results with symbol tools.
4. **Exploring mode first.** Only `detail_level: "full"` after you know what you need.
5. **Respect overflow hints.** Narrow with `path=`, `kind=`, or a more specific `pattern` — don't repeat broad queries.
6. **Prefer symbol edits** (`replace_symbol`, `insert_code`, `rename_symbol`) over `edit_lines` for code files.
