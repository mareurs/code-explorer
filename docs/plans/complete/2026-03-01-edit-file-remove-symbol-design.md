# Design: `edit_file` + `remove_symbol`

**Date:** 2026-03-01
**Status:** Approved
**Replaces:** `edit_lines` (to be deleted entirely)

## Motivation

`edit_lines` is a line-number-based splice tool that forces LLMs to think in
line numbers — a mental model they don't naturally have. This leads to three
recurring failure modes:

1. **Wrong line** — off-by-one errors, stale line numbers after earlier edits
2. **Wrong `delete_count`** — agent wants to insert before a line but uses
   `delete_count: 1` instead of `0`, destroying the anchor (e.g. deleting a
   closing `}`)
3. **No self-verification** — without `expected_content` (removed for token
   efficiency), edits are blind

Every major LLM coding tool (Claude Code's `Edit`, Cursor, Aider) has converged
on **old_string → new_string** as the primary edit mechanism. It matches how LLMs
reason: "I see X in the file, make it Y."

## Tool 1: `edit_file`

### Identity

- **Name:** `edit_file`
- **Description:** "Find and replace text in a file. Requires old_string to
  match exactly — include enough context to be unique."
- **Replaces:** `edit_lines`

### Schema

```json
{
  "type": "object",
  "required": ["path", "old_string", "new_string"],
  "properties": {
    "path": {
      "type": "string",
      "description": "File path"
    },
    "old_string": {
      "type": "string",
      "description": "Exact text to find (must match file content including whitespace and indentation)"
    },
    "new_string": {
      "type": "string",
      "description": "Replacement text. Empty string deletes the match."
    },
    "replace_all": {
      "type": "boolean",
      "default": false,
      "description": "Replace all occurrences (default: require unique match)"
    }
  }
}
```

### Behavior

| Scenario | `replace_all: false` (default) | `replace_all: true` |
|---|---|---|
| 0 matches | `RecoverableError` — "not found" + hint | Same |
| 1 match | Replace, return `"ok"` | Replace, return `"ok"` |
| N matches | `RecoverableError` — "found N, include more context or use replace_all" | Replace all, return `"ok"` |

### Error messages

- **Not found:** "old_string not found in {path}. Check whitespace and
  indentation — they must match exactly. Use search_pattern to verify."
- **Multiple matches:** "old_string found {N} times (lines {X}, {Y}, {Z}).
  Include more surrounding context to make it unique, or pass replace_all: true."

### Implementation

- `str::find` / `str::matches().count()` — no regex
- Preserves file trailing newline (read, check `ends_with('\n')`, restore)
- `validate_write_path` security gate
- `guard_worktree_write` check
- `notify_file_changed` after write
- Returns `json!("ok")` — no echo (per project convention)

### Why this works

- **Self-verifying:** if old_string doesn't exist, edit fails safely
- **No line numbers:** immune to stale/shifted line numbers
- **Matches LLM mental model:** "I see this, change it to that"
- **Uniqueness constraint is a feature:** forces enough context to be unambiguous
- **Token cost is acceptable:** LLM already has the content in context from
  earlier reads; the old_string is a subset, not the whole file

## Tool 2: `remove_symbol`

### Identity

- **Name:** `remove_symbol`
- **Description:** "Delete a symbol (function, struct, impl block, test, etc.)
  by name. Removes the entire declaration including doc comments."
- **New tool** — completes the symbol CRUD set alongside `replace_symbol`,
  `insert_code`, `rename_symbol`

### Schema

```json
{
  "type": "object",
  "required": ["name_path", "path"],
  "properties": {
    "name_path": {
      "type": "string",
      "description": "Symbol name path (e.g. 'MyStruct/my_method', 'tests/old_test')"
    },
    "path": {
      "type": "string",
      "description": "File path"
    }
  }
}
```

### Behavior

1. Resolve symbol via LSP `documentSymbol` (same as `replace_symbol`)
2. Scan backwards from `start_line` to include contiguous doc comments
   (`///`, `/** */`) and `#[...]` attributes — stop at first non-doc,
   non-attribute, non-blank line
3. Apply `trim_symbol_start` to avoid eating preceding symbol's closing brace
4. Delete lines `start..=end` (inclusive)
5. Collapse 3+ consecutive blank lines down to 1 (cleanup)
6. `notify_file_changed`
7. Return `json!("ok")`

### Error cases

- **Symbol not found:** `RecoverableError` — "symbol not found: {name_path}.
  Use list_symbols(path) to see available symbols."
- Same resolution path as `replace_symbol` via `find_symbol_by_name_path`

### Doc comment inclusion

When removing `my_func`, also remove its doc comment:

```rust
/// This helper does X.       ← included in removal
/// It handles Y and Z.       ← included in removal
#[inline]                      ← included in removal
fn my_func() {                 ← symbol start_line
    // ...
}                              ← symbol end_line
```

Scan backwards from `start_line`, skipping blank lines, collecting lines that
start with `///`, `//!`, `#[`, or are inside `/** */` blocks. Stop at the first
line that doesn't match these patterns.

### Shared infrastructure with `replace_symbol`

- `find_symbol_by_name_path` — symbol resolution
- `trim_symbol_start` — skip preceding closing braces
- `write_lines` — write with trailing newline preservation
- `validate_write_path` — security gate
- `guard_worktree_write` — worktree check

`remove_symbol` is essentially `replace_symbol` with `new_body = ""` plus
blank-line cleanup and doc-comment inclusion.

## Migration: deleting `edit_lines`

### Code to remove

- `EditLines` struct and `impl Tool for EditLines` in `src/tools/file.rs`
- `Arc::new(EditLines)` registration in `src/server.rs`
- `EditLines` import in `src/server.rs`
- All `edit_lines_*` test functions in `src/tools/file.rs`

### References to update

- `src/prompts/server_instructions.md` — replace `edit_lines` mention with
  `edit_file` in the "Edit code" section
- `CLAUDE.md` — update tool list in project structure section
- `docs/TODO-tool-misbehaviors.md` — mark BUG-001 as superseded

### Existing tools that stay

- `replace_symbol` — symbol-level replacement (preferred for code)
- `insert_code` — symbol-relative insertion
- `rename_symbol` — cross-codebase rename via LSP
- `create_file` — create or overwrite entire file

### Updated write tool hierarchy

| Tool | When to use |
|---|---|
| `replace_symbol` | Replace a known symbol's body |
| `insert_code` | Insert before/after a known symbol |
| `remove_symbol` | Delete a known symbol entirely |
| `rename_symbol` | Rename across the codebase |
| `edit_file` | General text replacement (configs, non-symbol edits, cross-boundary edits) |
| `create_file` | New files or full rewrites |

Server instructions should guide: **prefer symbol tools for code, use
`edit_file` when symbol tools don't fit.**

## Testing plan

### `edit_file` tests

- Single match: replace, verify content
- Single match: empty `new_string` deletes text
- No match: returns `RecoverableError` with hint
- Multiple matches + `replace_all: false`: returns error with count + line numbers
- Multiple matches + `replace_all: true`: replaces all, returns `"ok"`
- Whitespace sensitivity: old_string with wrong indentation fails
- Trailing newline preservation
- Security: rejects paths outside project root
- Response is plain `"ok"` string, not object

### `remove_symbol` tests

- Remove a function: body + doc comments deleted
- Remove a method: `MyStruct/my_method`
- Remove with attributes: `#[test]`, `#[inline]` included
- Symbol not found: `RecoverableError`
- Blank line cleanup: no triple blanks left behind
- Preceding symbol's closing brace preserved (reuses `trim_symbol_start`)
- Security: rejects paths outside project root
- Response is plain `"ok"` string
