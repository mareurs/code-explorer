# Design: edit_file Symbol Guidance + Prompt Updates

**Date:** 2026-03-01  
**Status:** Approved

## Problem

Agents default to `edit_file` (or native `Edit`) for source code changes, even when
symbol-aware tools (`replace_symbol`, `insert_code`, `remove_symbol`) would be safer,
more precise, and LSP-backed. Rule 6 in `server_instructions.md` exists but is buried
and not actionable enough.

## Scope

Three coordinated changes:
1. `edit_file` heuristic blocker (Rust code)
2. `server_instructions.md` anti-pattern table + memory section sync
3. `onboarding_prompt.md` private memory sync

---

## Component 1: edit_file Heuristic Blocker

**Files:** `src/tools/file.rs`, `src/util/path_security.rs`

### Blocking condition

In `EditFile::call()`, after param extraction, before the write:

```
if is_source_path(path) && old_string.contains('\n') {
    return Err(RecoverableError::with_hint(message, inferred_hint))
}
```

`is_source_path(path: &str) -> bool` — new public fn in `path_security.rs`, reuses
`SOURCE_EXTENSIONS` regex. Covers all supported languages:
`.rs|.py|.ts|.tsx|.js|.go|.java|.kt|.c|.cpp|.cs|.rb|.swift|.scala|.ex|.hs|.lua|.sh|…`

### Hint inference (cross-language)

Priority order:

| Condition | Hint points to |
|---|---|
| `new_string.is_empty()` | `remove_symbol(name_path, path)` |
| `old_string` matches `\b(fn\|def\|func\|fun\|function\|class\|struct\|impl\|trait\|interface\|enum\|type)\b` | `replace_symbol(name_path, path, new_body)` |
| `new_string.len() > old_string.len()` | `insert_code(name_path, path, code, position)` |
| fallback | generic: list all three tools |

### Error format

```
message: "edit_file cannot replace multi-line source code — use a symbol-aware tool"
hint: "<specific tool>(name_path, path, …) — <one-line description>"
```

Matches existing `RecoverableError::with_hint` style. No escape hatch (unlike
`run_command`'s `acknowledge_risk`) — symbol tools handle all structural cases.

---

## Component 2: server_instructions.md Updates

**File:** `src/prompts/server_instructions.md`

### 2a. Edit code section — add anti-pattern table

Insert after the existing tool list (after `create_file` bullet):

```markdown
**edit_file on source code → prefer symbol tools:**
| ❌ edit_file for… | ✅ Use instead |
|---|---|
| Replacing a function/struct body | `replace_symbol(name_path, path, new_body)` |
| Inserting code before/after a symbol | `insert_code(name_path, path, code, position)` |
| Deleting a function/struct/impl | `remove_symbol(name_path, path)` |
| Renaming across the codebase | `rename_symbol(name_path, path, new_name)` |

`edit_file` is for non-structural changes only: imports, string literals, comments, config values.
```

### 2b. Memory section — add private memory params

After the `write_memory` / `read_memory` bullet list, add:
```markdown
- `write_memory(topic, content, private=true)` — store in project-local private store (not injected into system instructions)
- `list_memories(include_private=true)` — returns both shared and private memories
```

---

## Component 3: onboarding_prompt.md Sync

**File:** `src/prompts/onboarding_prompt.md`

Add a note in the memory-writing rules section (Rule block at top) that private memories
exist and are written with `private=true`. One or two sentences — agents should know the
option exists when generating the initial memory set.

---

## Testing

- Unit test in `src/tools/file.rs`: `edit_file_blocks_multiline_on_source_file` — verify
  `RecoverableError` is returned for `.rs`/`.py`/`.ts` with multi-line `old_string`
- Unit test: `edit_file_allows_singleline_on_source_file` — single-line edits pass through
- Unit test: `edit_file_allows_multiline_on_non_source_file` — `.md`/`.toml` pass through
- Unit test: `edit_file_hint_suggests_remove_symbol_when_new_string_empty`
- Unit test: `edit_file_hint_suggests_replace_symbol_for_fn_def`
- Unit test: `is_source_path` covers all `SOURCE_EXTENSIONS` languages
