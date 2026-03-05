# Prompt Refactoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite server_instructions.md with superpowers-style forcing patterns, update all tool references across onboarding files to match the 23-tool post-restructure surface.

**Architecture:** Three files change: `server_instructions.md` (deep rewrite), `onboarding_prompt.md` (ref swaps), Rust code in `workflow.rs` + `prompts/mod.rs` (ref swaps + test fixes). Design doc: `docs/plans/2026-03-03-prompt-refactoring-design.md`.

**Tech Stack:** Markdown (prompt files), Rust (workflow.rs, prompts/mod.rs)

---

## Task 1: Rewrite `server_instructions.md`

**Files:**
- Overwrite: `src/prompts/server_instructions.md`

**Step 1: Write the new server_instructions.md**

Replace the entire file with the new Layered Iron Laws structure. The file must contain these sections in order:

1. **Opening** — tagline + subagent reminder (keep existing text)
2. **Iron Laws** — 3 non-negotiable rules
3. **How to Choose the Right Tool** — two tables (by knowledge level + by task)
4. **Anti-Patterns** — consolidated ❌/✅ table + rationalization callout
5. **Tool Reference** — 23 tools in 7 groups (File I/O, Symbol Navigation, Symbol Editing, Semantic Search, Workflow, Memory, Project & Libraries)
6. **Output System** — modes + buffer ref table
7. **Project Management** — worktrees + customization
8. **Rules** — 9 numbered one-liners

Full content below:

````markdown
code-explorer MCP server: high-performance semantic code intelligence.
Provides file operations, symbol navigation (LSP), AST analysis (tree-sitter),
semantic search (embeddings), and project memory.

**Subagents and spawned agents SHOULD use code-explorer too.** If you spawn a subagent
or delegate to another agent, instruct it to use code-explorer tools for all code
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
| Persistent notes | `memory(action="read\|write\|list\|delete")` | ~~removed 4 separate memory tools~~ |

## Anti-Patterns — STOP if you catch yourself doing these

| ❌ Never do this | ✅ Do this instead | Why |
|---|---|---|
| `read_file("src/main.rs")` to read source | `list_symbols("src/main.rs")` then `find_symbol(name, include_body=true)` | Symbol tools are structured + token-efficient |
| `read_file` then scan for a function | `find_symbol("function_name")` directly | Skip the file, go straight to the symbol |
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
  (>200 lines) return a smart summary + `@file_*` ref. Source code summaries include
  top-level symbols. Use `start_line`/`end_line` for targeted excerpts.
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

### Project & Libraries

- `activate_project(path)` — switch active project root. Required after `EnterWorktree`.
- `project_status` — project state: config, semantic index health, usage telemetry,
  library summary. Pass `threshold` for drift scores, `window` for usage time range.
- `list_libraries` — registered libraries and index status. Use `scope="lib:<name>"` in
  `semantic_search`, `find_symbol`, or `index_project` to target a library.

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

Large content is stored in an `OutputBuffer` — you get a smart summary + `@ref` handle.
The full content costs nothing to hold. Query via `run_command` + Unix tools:

| Ref pattern | Source | Example query |
|---|---|---|
| `@cmd_*` | `run_command` output | `grep FAILED @cmd_a1b2c3` |
| `@file_*` | Large file reads (>200 lines) | `sed -n '42,80p' @file_abc123` |
| `@tool_*` | Large tool responses (>10 KB) | `jq '.symbols[].name' @tool_abc12345` |

Buffer queries return ≤ 100 lines inline. Truncation hints show the exact `sed` command
to continue. Do NOT pipe buffer queries (`grep @ref | head`) — run targeted commands directly.

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
````

**Step 2: Verify section headings used by tests**

The test `static_instructions_contain_key_sections` in `src/prompts/mod.rs:145` checks:
- `"## How to Choose the Right Tool"` — ✅ present in new file
- `"## Output Modes"` — ❌ renamed to `"## Output System"` with `### Modes` subsection
- `"## Rules"` — ✅ present in new file

This test needs updating in Task 3.

**Step 3: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "feat: rewrite server_instructions.md with Iron Laws forcing patterns

Restructured for the 23-tool post-restructure surface:
- Iron Laws at top for non-negotiable compliance anchors
- Decision matrix tables replace prose (by knowledge level + by task)
- Consolidated anti-pattern wall covers all 4 pain points
- Tool reference updated: memory dispatch, project_status, include_docs, scope param
- Output system compressed (buffer ref table vs prose)
- Rules section as quick-reference checklist"
```

---

## Task 2: Update `onboarding_prompt.md` tool references

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`

**Step 1: Apply all ref swaps**

8 spots need updating (line numbers from current file):

| Line | Old | New |
|---|---|---|
| 1 | `write_memory(topic, content)` | `memory(action="write", topic=..., content=...)` |
| 8 | `list_functions` for API surface | `list_symbols` for API surface |
| 10 | `write_memory(topic, content, private=true)` | `memory(action="write", topic=..., content=..., private=true)` |
| 10 | `Standard write_memory creates` | `Standard memory(action="write") creates` |
| 214 | `read_memory(topic)` | `memory(action="read", topic=...)` |
| 215 | `write_memory(topic, content)` | `memory(action="write", topic=..., content=...)` |
| 217 | `read_memory("architecture")` | `memory(action="read", topic="architecture")` |
| 233 | `write_memory(topic, content, private=true)` | `memory(action="write", topic=..., content=..., private=true)` |

**Step 2: Add tool surface list to System Prompt template section (Memory 7)**

After the "What NOT to include" section (~line 179), add:

```markdown
**Current tool surface (23 tools):** read_file, list_dir, search_pattern, create_file,
find_file, edit_file, run_command, onboarding, find_symbol, list_symbols, find_references,
goto_definition, hover, replace_symbol, remove_symbol, insert_code, rename_symbol,
semantic_search, index_project, memory, activate_project, project_status, list_libraries.

Do NOT reference removed tools: list_functions, list_docs, index_library, git_blame,
index_status, get_usage_stats, get_config, write_memory, read_memory, list_memories,
delete_memory.
```

**Step 3: Commit**

```bash
git add src/prompts/onboarding_prompt.md
git commit -m "feat: update onboarding_prompt.md for 23-tool surface

Swap all write_memory/read_memory/list_memories refs to memory(action=...) syntax.
Replace list_functions ref with list_symbols. Add tool surface inventory to system
prompt template section."
```

---

## Task 3: Update Rust code — `prompts/mod.rs` + test assertions

**Files:**
- Modify: `src/prompts/mod.rs`

**Step 1: Update `read_memory` reference in `build_server_instructions`**

Line 29 — change:
```rust
"- **Available shared memories:** {} — use `read_memory(topic)` to read relevant ones as needed for your current task\n",
```
to:
```rust
"- **Available shared memories:** {} — use `memory(action=\"read\", topic=...)` to read relevant ones as needed for your current task\n",
```

**Step 2: Update test assertion for renamed heading**

Line 147 — in `static_instructions_contain_key_sections`, change:
```rust
assert!(SERVER_INSTRUCTIONS.contains("## Output Modes"));
```
to:
```rust
assert!(SERVER_INSTRUCTIONS.contains("## Output System"));
```

**Step 3: Run tests to verify**

```bash
cargo test --lib prompts
```

Expected: all prompts tests pass.

**Step 4: Commit**

```bash
git add src/prompts/mod.rs
git commit -m "fix: update prompts/mod.rs refs and test assertions for new tool surface"
```

---

## Task 4: Update Rust code — `workflow.rs` tool references

**Files:**
- Modify: `src/tools/workflow.rs`

**Step 1: Update `build_system_prompt_draft` function**

6 spots need updating:

| Line | Old | New |
|---|---|---|
| 228 | `read_memory("architecture")` | `memory(action="read", topic="architecture")` |
| 249 | `write_memory(topic, content, private=true)` | `memory(action="write", topic=..., content=..., private=true)` |
| 254 | `write_memory(topic, content)` | `memory(action="write", topic=..., content=...)` |
| 257 | `list_memories(include_private=true)` | `memory(action="list", include_private=true)` |
| 310 | `read_memory(topic)` | `memory(action="read", topic=...)` |
| 316 | `read_memory(topic, private=true)` | `memory(action="read", topic=..., private=true)` |

**Step 2: Run tests to verify**

```bash
cargo test --lib tools::workflow
```

Expected: all workflow tests pass.

**Step 3: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "fix: update workflow.rs memory tool refs to dispatch syntax"
```

---

## Task 5: Final verification

**Step 1: Run full test suite**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Expected: all 926 tests pass, no clippy warnings, formatting clean.

**Step 2: Verify token budget**

Rough check — the new `server_instructions.md` should be ~170-190 lines. If significantly
over 200 lines, review for compression opportunities.

```bash
wc -l src/prompts/server_instructions.md
```

**Step 3: Verify no stale tool references remain**

```bash
# Should return 0 matches in prompt files (excluding plan docs):
grep -rn 'read_memory\|write_memory\|list_memories\|delete_memory\|list_functions\|list_docs\b\|index_library\|git_blame\|index_status\|get_usage_stats\|get_config' src/prompts/ src/tools/workflow.rs
```

Note: `get_config` may still appear in Rust type names (`ProjectConfig`) or test helpers — that's fine.
Only prompt-facing strings (in `.md` files and `push_str` calls) should be clean.
