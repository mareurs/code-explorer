codescout MCP server: high-performance semantic code intelligence.
Provides file operations, symbol navigation (LSP), AST analysis (tree-sitter),
semantic search (embeddings), and project memory.

**Subagents and spawned agents SHOULD use codescout too.** If you spawn a subagent
or delegate to another agent, instruct it to use codescout tools for all code
navigation — do not fall back to native Read/Grep/Glob on source files.

## Iron Laws

These are non-negotiable. Violating the letter IS violating the spirit.

1. **NO `read_file` ON SOURCE CODE.** Use `list_symbols` + `find_symbol(include_body=true)`.
   `read_file` on source returns a summary, not raw content. Symbol tools give you
   structured, token-efficient navigation. `read_file` is for config, markdown, and data files.

2. **NO `edit_file` FOR STRUCTURAL CODE CHANGES.** Use `replace_symbol`, `insert_code`,
   `remove_symbol`, or `rename_symbol`. `edit_file` is for imports, literals, comments, config.
   Multi-line edits on source files are blocked — the tool tells you which symbol tool to use.

3. **NO PIPING `run_command` OUTPUT.** Run the command bare, then query the `@ref` buffer
   in a follow-up: `cargo test` → `grep FAILED @cmd_id`. Never `cargo test 2>&1 | grep FAILED`.
   The buffer system exists to save your context window — use it.

## How to Choose the Right Tool

### By knowledge level

| You know… | Start with | Then drill with |
|---|---|---|
| **The name** (function, type, symbol) | `find_symbol(pattern)` or `list_symbols(path)` | `find_symbol(name_path, include_body=true)` |
| **The concept** ("how does auth work?") | `semantic_search(query)` | `list_symbols` / `find_symbol` on results |
| **Nothing** (new codebase) | `list_dir(path)` → `list_symbols(file)` | `semantic_search("what does this do")` |
| **A text pattern** (regex, error message) | `search_pattern(pattern)` | `find_symbol` on matched files |
| **A filename** (glob pattern) | `find_file(pattern)` | `read_file` or `list_symbols` on result |

### By task

| Task | Tool | NOT this |
|---|---|---|
| Read a function body | `find_symbol(name, include_body=true)` | ~~`read_file("src/foo.rs")`~~ |
| See file structure | `list_symbols(path)` | ~~`read_file` entire file~~ |
| Get docstrings | `list_symbols(path, include_docs=true)` | ~~removed `list_docs`~~ |
| Get function signatures | `list_symbols(path)` | ~~removed `list_functions`~~ |
| Find all usages | `find_references(name_path, path)` | ~~`search_pattern`~~ |
| Jump to definition | `goto_definition(path, line)` | — |
| Type info / docs | `hover(path, line)` | — |
| Replace a function body | `replace_symbol(name_path, path, new_body)` | ~~`edit_file` with old_string~~ |
| Insert code near a symbol | `insert_code(name_path, path, code, position)` | ~~`edit_file`~~ |
| Delete a symbol | `remove_symbol(name_path, path)` | ~~`edit_file` with empty string~~ |
| Rename across codebase | `rename_symbol(name_path, path, new_name)` | ~~manual find/replace~~ |
| Change an import/literal/comment | `edit_file(path, old_string, new_string)` | — (correct tool) |
| Read config/markdown/data | `read_file(path)` | — (correct tool) |
| Run a shell command | `run_command(command)` | ~~piped commands~~ |
| Index a library | `index_project(scope="lib:name")` | ~~removed `index_library`~~ |
| Project health check | `project_status` | ~~removed `get_config`, `index_status`, `get_usage_stats`~~ |
| Persistent notes | `memory(action="read\|write\|list\|delete")` | ~~`write_memory`, `read_memory`, `list_memories`, `delete_memory`~~ |
| Search memories by meaning | `memory(action="recall", query="...")` | — |
| Store knowledge for later | `memory(action="remember", content="...")` | — |
| Git line history | `run_command("git blame file")` | ~~removed `git_blame` tool~~ |
| GitHub identity / teams | `github_identity(method, ...)` | — |
| GitHub issues | `github_issue(method, owner, repo, ...)` | — |
| GitHub pull requests | `github_pr(method, owner, repo, ...)` | — |
| GitHub file contents / writes | `github_file(method, owner, repo, path, ...)` | — |
| GitHub repo / branches / releases | `github_repo(method, ...)` | — |

## Anti-Patterns — STOP if you catch yourself doing these

| ❌ Never do this | ✅ Do this instead | Why |
|---|---|---|
| `read_file("src/main.rs")` to read source | `list_symbols("src/main.rs")` then `find_symbol(name, include_body=true)` | Symbol tools are structured + token-efficient |
| `read_file` then scan for a function | `find_symbol("function_name")` directly | Skip the file, go straight to the symbol |
| `run_command("jq '.key' @file_ref")` to query JSON | `read_file(path, json_path="$.key")` | Navigation params > shell buffer queries |
| `edit_file` with multi-line old_string on `.rs`/`.py`/`.ts` | `replace_symbol(name_path, path, new_body)` | Structural edits > fragile string matching |
| `edit_file` to delete a function | `remove_symbol(name_path, path)` | LSP knows the exact range |
| `edit_file` to add code after a function | `insert_code(name_path, path, code, "after")` | Position-aware, no string matching |
| `run_command("cargo test 2>&1 \| grep FAIL")` | `run_command("cargo test")` then `grep FAIL @cmd_id` | Buffer saves context; pipes waste it |
| `run_command("cd /abs/path && cmd")` | `run_command("cmd")` — already in project root | Use `cwd` param for subdirectories |
| `run_command("cat src/lib.rs")` | `list_symbols("src/lib.rs")` or `read_file` with line range | Shell reads on source are blocked |
| Repeat a broad `find_symbol` after overflow | Narrow with `path=`, `kind=`, or more specific pattern | Follow the overflow hint |
| Ignore `by_file` in overflow response | Use top file from `by_file` as `path=` filter | The hint tells you exactly where to look |

**If you catch yourself rationalizing** ("I'll just quickly read the file", "this edit is
too small for replace_symbol", "one pipe won't hurt") — that's the signal to stop and
use the right tool. Small shortcuts compound into large context waste.

## Tool Reference

### File I/O

- `read_file(path)` — read a file. Short files return content directly; large files
  (>200 lines) return a structural summary + `@file_*` ref with line ranges.
  Navigate into large files by format instead of querying buffers:
  - **Markdown:** summary shows heading tree → `read_file(path, heading="## Section")`
  - **JSON:** summary shows key schema → `read_file(path, json_path="$.key")`
  - **TOML/YAML:** summary shows table structure → `read_file(path, toml_key="section")`
  - Source code summaries include top-level symbols. Use `start_line`/`end_line` for excerpts.
  Prefer `list_symbols` / `find_symbol` over `read_file` for source code navigation.
- `list_dir(path)` — list files and directories. Pass `recursive=true` for a full tree.
- `search_pattern(pattern)` — regex search across files. Pass `context_lines` for
  merged context blocks. Scope with `path=`, limit with `max_results` (default 50).
- `find_file(pattern)` — glob-based file search (e.g. `**/*.rs`, `src/**/mod.rs`).
- `create_file(path, content)` — create or overwrite a file.
- `edit_file(path, old_string, new_string)` — exact string replacement. Whitespace-sensitive.
  `replace_all=true` for all occurrences. `insert="prepend"|"append"` to add at file
  boundaries. For imports, literals, comments, config — NOT structural code changes.

### Symbol Navigation (LSP)

- `find_symbol(pattern)` — locate by name substring. Accepts `name_path` (e.g.
  `MyStruct/my_method`). Filter with `kind`: function, class, struct, interface, type,
  enum, module, constant. Pass `include_body=true` to read the implementation.
- `list_symbols(path)` — symbol tree for file/dir/glob. Pass `include_docs=true` for
  docstrings. Signatures always included. Single-file mode caps at 100 top-level symbols.
- `find_references(name_path, path)` — find all usages of a symbol.
- `goto_definition(path, line)` — jump to definition via LSP. Auto-discovers libraries.
- `hover(path, line)` — type info and documentation for a symbol at a position.

### Symbol Editing (LSP)

- `replace_symbol(name_path, path, new_body)` — replace entire symbol body.
- `insert_code(name_path, path, code, position)` — insert before or after a named symbol.
- `remove_symbol(name_path, path)` — delete a symbol (removes lines covered by LSP range).
- `rename_symbol(name_path, path, new_name)` — rename across the codebase via LSP.
  Sweeps for textual remainders in comments/docs/strings. **Warning:** may corrupt string
  literals containing the old name — verify compilation after use.

### Semantic Search

- `semantic_search(query)` — find code by natural language or snippet. Returns ranked
  chunks with similarity scores. Use `scope="lib:<name>"` for library code.
- `index_project` — build or update the semantic index. Use `scope="lib:<name>"` to
  index a registered library. Pass `force=true` to rebuild from scratch.

### Workflow

- `run_command(command)` — execute a shell command from the project root. Large output
  stored as `@cmd_*` buffer ref. Stderr captured automatically.
  - `cwd` — run from a subdirectory (relative to project root)
  - `acknowledge_risk` — bypass safety check for destructive commands
  - `timeout_secs` — max execution time (default 30)
- `onboarding` — project discovery: detect languages, read key files, generate system
  prompt draft. Use `force=true` to re-scan.

### Memory

- `memory(action, ...)` — persistent project knowledge.
  - `action="write"` — requires `topic`, `content`. Pass `private=true` for gitignored store.
  - `action="read"` — requires `topic`. Pass `private=true` for private store.
  - `action="list"` — pass `include_private=true` to see both shared and private topics.
  - `action="delete"` — requires `topic`. Pass `private=true` for private store.
  - `action="remember"` — store a semantic memory. Requires `content`. Optional `title`, `bucket` (code/system/preferences/unstructured — auto-classified if omitted).
  - `action="recall"` — search memories by meaning. Requires `query`. Optional `bucket` filter, `limit`.
  - `action="forget"` — delete a semantic memory. Requires `id` (from recall results).

### Project & Libraries

- `activate_project(path)` — switch active project root. Required after `EnterWorktree`.
- `project_status` — project state: config, semantic index health, usage telemetry,
  library summary. Pass `threshold` for drift scores, `window` for usage time range.
- `list_libraries` — registered libraries and index status. Use `scope="lib:<name>"` in
  `semantic_search`, `find_symbol`, or `index_project` to target a library.

### GitHub

- `github_identity(method)` — authenticated user profile, team membership, user search.
  - `method`: `get_me` | `search_users` (query required) | `get_teams` | `get_team_members` (org + team_slug required)
- `github_issue(method, owner, repo, ...)` — issue read/write operations.
  - Read: `list` | `search` | `get` | `get_comments` | `get_labels` | `get_sub_issues`
  - Write: `create` (title required) | `update` | `add_comment` | `add_sub_issue` | `remove_sub_issue`
  - `limit` defaults to 30 for list/search.
- `github_pr(method, owner, repo, ...)` — pull request read/write operations.
  - Read: `list` | `search` | `get` | `get_diff` | `get_files` | `get_comments` | `get_reviews` | `get_review_comments` | `get_status`
  - Write: `create` | `update` | `merge` | `update_branch` | `create_review` | `submit_review` | `delete_review` | `add_review_comment` | `add_reply_to_comment`
  - `get_diff` always returns a `@tool` buffer handle (diffs are large).
- `github_file(method, owner, repo, path, ...)` — file contents and writes via GitHub API.
  - `get` — fetch file at optional ref/branch (returns `@buffer` handle).
  - `create_or_update` — create or update a single file (`sha` required when updating).
  - `delete` — delete a file (`sha` required).
  - `push_files` — push multiple files in a single commit.
- `github_repo(method, ...)` — repository, branch, commit, release, and code search.
  - Repo: `search` | `create` | `fork`
  - Branches: `list_branches` | `create_branch`
  - Commits: `list_commits` | `get_commit` (returns `@buffer` handle)
  - Releases: `list_releases` | `get_latest_release` | `get_release_by_tag`
  - Tags: `list_tags` | `get_tag`
  - Code: `search_code` (returns `@buffer` handle)

## Output System

**File paths in tool output are relative to the project root** (e.g. `src/tools/mod.rs`,
not `/home/user/project/src/tools/mod.rs`). Pass them as-is to other tools.

### Modes

Default: **exploring** — compact, capped at 200 items.
Pass `detail_level: "full"` for focused mode with `offset`/`limit` pagination.
Only switch to focused AFTER identifying targets.

Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "...", "by_file": [...] } }`
— **follow the hint.** Narrow with `path=`, `kind=`, or a more specific `pattern`.
`by_file` shows per-file match counts; use the top file as your `path=` filter.

### Output Buffers

Large content is stored in an `OutputBuffer`. When a result is buffered you receive an
`output_id` field (or `file_id` for large file reads) containing a `@ref` handle.
The full content costs nothing to hold — query it on demand.

#### How to recognise which ref type you have

| Signal in tool result | Ref type | Content |
|---|---|---|
| `"output_id": "@cmd_abc"` in `run_command` result | `@cmd_*` | **plain text** — raw command stdout |
| `"file_id": "@file_abc"` in `read_file` result | `@file_*` | **plain text** — raw file content |
| `"output_id": "@tool_abc"` in any other tool result | `@tool_*` | **structured JSON** — pretty-printed tool result |

#### Access patterns by ref type

| Ref | Content type | Primary access | Secondary access |
|---|---|---|---|
| `@file_*` | plain text | `run_command("grep pattern @file_abc")` | `read_file("@file_abc", start_line=N, end_line=M)` |
| `@cmd_*` | plain text | `run_command("grep pattern @cmd_abc")` | `read_file("@cmd_abc", start_line=N, end_line=M)` |
| `@tool_*` | JSON object | `read_file("@tool_abc", json_path="$.field")` | `read_file("@tool_abc", start_line=N, end_line=M)` |

**Key distinction:** `@file_*` and `@cmd_*` contain plain text — grep/sed are the natural
tools. `@tool_*` contains a JSON object (pretty-printed, multi-line) — use `json_path` to
extract a specific field, or `start_line`/`end_line` to browse sections.

**`@tool_*` json_path examples:**
- `find_symbol` result → `json_path="$.symbols[0].body"` extracts the function body as plain text
- `list_symbols` result → `json_path="$.files[0].symbols"` extracts the symbol list
- Any result → browse with `start_line=1, end_line=50` to see the structure first

**Buffer queries via `run_command`** return ≤ 100 lines inline. Truncation hints show the
exact `sed` command to continue. Do NOT pipe buffer queries — run targeted commands directly.

**Never grep a `@tool_*` ref for code content** — function bodies are JSON string values
(`"body": "fn foo() {\n..."`); grep on code inside them will not work. Use
`json_path="$.symbols[N].body"` instead, or read the source file directly using
`start_line`/`end_line` from the symbol metadata.

## Project Management

### Worktrees

After `EnterWorktree`, call `activate_project` with the worktree path — write tools are
NOT automatically coupled to the shell's working directory. If you forget, writes silently
modify the main repo. To clean up: `git worktree prune` from the main repo root.

### Project Customization

If `.codescout/system-prompt.md` exists, its contents appear below as "Custom
Instructions" — project-specific guidance. Edit the file to customize AI behavior.

## Rules

1. **Symbol tools over `read_file` for source code.** `list_symbols` + `find_symbol(include_body=true)` beats reading entire files.
2. **Symbol edits over `edit_file` for code.** `replace_symbol`, `insert_code`, `remove_symbol` for structural changes. `edit_file` for imports, literals, comments.
3. **Run commands bare, query buffers after.** `cargo test` → `grep FAILED @cmd_id`. Never pipe.
4. **Exploring mode first.** Only `detail_level: "full"` after you know what you need.
5. **Follow overflow hints.** Narrow with `path=`, `kind=`, or a more specific pattern — don't repeat broad queries.
6. **`run_command` is already in the project root.** Never prefix with `cd /abs/path &&`. Use `cwd` for subdirectories.
7. **Buffer queries: run targeted, don't pipe.** `grep pattern @cmd_id` — never `grep @ref | head`.
8. **Check `features_md` from `onboarding` before suggesting features.** Don't propose work that's already done.
9. **Semantic search for "how does X work?"** Then drill into results with symbol tools.