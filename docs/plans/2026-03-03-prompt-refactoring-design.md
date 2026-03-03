# Prompt Refactoring Design — server_instructions + onboarding

**Date:** 2026-03-03
**Status:** Approved
**Approach:** Layered Iron Laws (Approach A)

## Goal

Rewrite `server_instructions.md` with superpowers-style forcing patterns to improve LLM
compliance with tool selection. Update `onboarding_prompt.md` and `build_system_prompt_draft()`
to reflect the 23-tool post-restructure surface. Minimize token cost in server instructions
(injected every request) while allowing onboarding to be thorough (one-time cost).

## Context

- Tool restructure landed: 32 → 23 tools (commit `238b8ad`)
- Four main LLM compliance failures: read_file overuse on source, edit_file instead of symbol
  tools, piping run_command output, ignoring overflow hints
- Superpowers research identified effective forcing patterns: Iron Laws, anti-rationalization
  tables, decision matrices, gate functions

## Addendum: Context-Aware `read_file` Navigation

The context-aware `read_file` design (`docs/plans/2026-03-03-context-aware-read-file-design.md`)
introduces format-specific navigation parameters that replace the `run_command("jq ...")` pattern:

- **Markdown:** `read_file(path, heading="## Section")` — jump to a heading
- **JSON:** `read_file(path, json_path="$.key")` — extract a JSON subtree
- **TOML/YAML:** `read_file(path, toml_key="section")` — extract a config section

These are documented in `server_instructions.md` now (all 3 types). The onboarding system
prompt template includes a "File Navigation" section with project-specific examples.
`build_system_prompt_draft()` generates hints based on detected config files.

## Token Budget

| File | Current | Target | Rationale |
|---|---|---|---|
| `server_instructions.md` | ~4.5K tokens (168 lines) | ~5-6K tokens | Modest growth for forcing patterns |
| `onboarding_prompt.md` | ~6K tokens (235 lines) | No hard limit | One-time cost, thoroughness matters |
| `build_system_prompt_draft()` | N/A (Rust fn) | N/A | 2 spots need ref updates |

---

## File 1: `server_instructions.md` — Deep Rewrite

### Structure

```
1. Opening (tagline + subagent reminder)           ~50 tokens
2. Iron Laws (3 non-negotiable rules)              ~200 tokens  ← NEW
3. Tool Selection (decision matrix tables)         ~500 tokens  ← restructured
4. Anti-Pattern Wall (❌/✅ consolidated table)     ~400 tokens  ← NEW
5. Tool Reference (grouped bullet descriptions)    ~2,200 tokens ← updated for 23 tools
6. Output System (modes + buffers)                 ~800 tokens  ← tightened
7. Project Management (worktrees, customization)   ~500 tokens  ← updated
8. Rules (numbered quick-reference)                ~350 tokens  ← compressed
                                              ─────────────────
                                              Total: ~5,000 tokens
```

### Section 1: Opening

Unchanged from current — tagline + subagent reminder. ~50 tokens.

### Section 2: Iron Laws (NEW)

Three non-negotiable rules at the very top, using superpowers-style authority language.
Front-loaded position creates mental anchors for all subsequent tool selection.

1. **NO `read_file` ON SOURCE CODE.** Use `list_symbols` + `find_symbol(include_body=true)`.
2. **NO `edit_file` FOR STRUCTURAL CODE CHANGES.** Use `replace_symbol`, `insert_code`,
   `remove_symbol`, or `rename_symbol`.
3. **NO PIPING `run_command` OUTPUT.** Run bare, then query `@ref` buffers.

Includes "Violating the letter IS violating the spirit" — blocks workaround thinking.

### Section 3: Tool Selection (restructured)

Two tables replacing the current prose sections:

**By knowledge level** — "You know the name / concept / nothing / text pattern / filename"
→ maps to starting tool + drill-down tool.

**By task** — "Read a function body / See file structure / Replace a function / ..."
→ maps to correct tool + ~~strikethrough wrong tool~~. Strikethrough serves as inline
anti-pattern without needing a separate section.

Includes migration hints for removed tools (list_functions, list_docs, index_library,
git_blame, 4 memory tools, index_status, get_usage_stats, get_config).

### Section 4: Anti-Pattern Wall (NEW)

Consolidated ❌/✅ table covering all 4 pain points in one scannable reference:
- read_file on source code (3 rows)
- edit_file for structural changes (3 rows)
- piping run_command (3 rows)
- ignoring overflow hints (2 rows)

Closes with a rationalization callout: quoted inner-monologue examples that trigger
self-checking ("I'll just quickly read the file"...).

### Section 5: Tool Reference (updated)

Grouped bullets for all 23 tools, organized by category:

| Group | Tools | Count |
|---|---|---|
| File I/O | read_file, list_dir, search_pattern, create_file, find_file, edit_file | 6 |
| Symbol Navigation | find_symbol, list_symbols, find_references, goto_definition, hover | 5 |
| Symbol Editing | replace_symbol, remove_symbol, insert_code, rename_symbol | 4 |
| Semantic Search | semantic_search, index_project | 2 |
| Workflow | run_command, onboarding | 2 |
| Memory | memory (action dispatch) | 1 |
| Project & Libraries | activate_project, project_status, list_libraries | 3 |

Key changes from current:
- Descriptions focus on "when" and "gotchas", not param descriptions (schema covers those)
- Symbol tools split into Navigation vs Editing (reinforces Iron Law #2)
- `memory` documented as single action-dispatch tool
- `project_status` documented as combined config/index/usage/library tool
- `list_symbols` documents new `include_docs` param
- `index_project` documents new `scope` param for library indexing

### Section 6: Output System (tightened)

- Output modes: exploring vs focused — preserved but compressed
- Output buffers: **table format** replacing prose bullets (~80 tokens vs ~180)
  - `@cmd_*`, `@file_*`, `@tool_*` with example queries in table columns
- Buffer query rules: ≤100 lines inline, no piping, follow truncation hints

### Section 7: Project Management (updated)

- Worktree section: kept minimal (edge case, one critical gotcha)
- Project customization: .code-explorer/system-prompt.md reference
- Removed: standalone memory section (now in Tool Reference)
- Removed: standalone index/config sections (now project_status)

### Section 8: Rules (compressed)

9 rules, one line each. Intentionally redundant with Iron Laws — different mental mode
(checklist scan vs authority anchor). Each rule embeds the correct command inline for
copy-paste.

---

## File 2: `onboarding_prompt.md` — Ref Updates

### Tool reference swaps (mechanical, throughout file)

| Old | New |
|---|---|
| `write_memory(topic, content)` | `memory(action="write", topic=..., content=...)` |
| `write_memory(topic, content, private=true)` | `memory(action="write", topic=..., content=..., private=true)` |
| `read_memory(topic)` | `memory(action="read", topic=...)` |
| `list_memories` | `memory(action="list")` |
| `list_memories(include_private=true)` | `memory(action="list", include_private=true)` |
| `delete_memory(topic)` | `memory(action="delete", topic=...)` |

### System prompt template (Memory 7) — additions

Add a tool surface reference so the onboarding LLM generates correct tool names:

```
**Current tool surface (23 tools):** read_file, list_dir, search_pattern, create_file,
find_file, edit_file, run_command, onboarding, find_symbol, list_symbols, find_references,
goto_definition, hover, replace_symbol, remove_symbol, insert_code, rename_symbol,
semantic_search, index_project, memory, activate_project, project_status, list_libraries.

Do NOT reference removed tools: list_functions, list_docs, index_library, git_blame,
index_status, get_usage_stats, get_config, write_memory, read_memory, list_memories,
delete_memory.
```

### Quick start section — update

```
1. `memory(action="read", topic="architecture")` — orient yourself
2. `list_symbols("src/")` — see the module structure
3. `semantic_search("your concept")` — find relevant code
4. `find_symbol("Name", include_body=true)` — read the implementation
```

### No structural changes

The template sections, anti-patterns, and 6-memory structure are all sound.

---

## File 3: `build_system_prompt_draft()` — Rust Ref Updates

Two spots in `src/tools/workflow.rs`:

### Spot 1: Navigation Strategy (~line 253)

```rust
// Old:
"1. `read_memory(\"architecture\")` — orient yourself\n"
// New:
"1. `memory(action=\"read\", topic=\"architecture\")` — orient yourself\n"
```

Lines 2-4 unchanged (list_symbols, semantic_search, find_symbol still correct).

### Spot 2: Private Memory Rules (~line 257)

Replace all `write_memory(...)` / `list_memories(...)` references with
`memory(action="write", ...)` / `memory(action="list", ...)` syntax.

---

## Design Principles Applied

### From Superpowers Research

| Pattern | Where applied | Rationale |
|---|---|---|
| Iron Laws (front-loaded authority) | Section 2 | Creates mental anchors before tool catalog |
| Anti-rationalization wall | Section 4 | Pre-empts the 4 most common shortcuts |
| Decision matrix tables | Section 3 | Two entry points: by knowledge level + by task |
| ❌/✅ contrast pairs | Sections 3-4 | Visual saliency, 3x more token-efficient than prose |
| "Violating the spirit" clause | Section 2 | Blocks workaround reasoning |
| Rationalization callout | Section 4 | Quoted inner-monologue triggers self-checking |
| Redundant enforcement layers | Sections 2+8 | Same principles in authority mode + checklist mode |
| Compressed reference tables | Section 6 | Buffer types as table vs prose: 80 vs 180 tokens |

### Token Efficiency

- Tool descriptions lean on JSON schema (LLMs already see it) — focus on "when" not "what"
- Tables replace prose throughout (~3x compression)
- Strikethrough in task table doubles as inline anti-pattern
- git_blame migration row included for LLMs with old tool surface memorized

---

## Implementation Order

1. Write new `server_instructions.md` content
2. Update `onboarding_prompt.md` tool references
3. Update `build_system_prompt_draft()` in workflow.rs
4. Run `cargo fmt` + `cargo clippy` + `cargo test`
5. Verify token count of server_instructions.md stays within ~5-6K budget
