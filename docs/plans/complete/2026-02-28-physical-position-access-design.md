# Design: Physical Position Access — `context_lines` + Targeted Source Reads

**Date:** 2026-02-28
**Status:** Approved

## Problem

Agents navigating unfamiliar codebases occasionally reach for `Bash(python3 -c "with open(...)")`
to read source files. Two concrete use cases drove this:

1. **Search + extended context** — agent found a string in a `.d.ts` file and needed 600 chars
   of surrounding type definition. `search_pattern` returns only the matching line; there is no
   way to get context around a match without a Bash script.

2. **Targeted line-range read from a source file** — agent knew lines 26–45 of a `.ts` file and
   needed them. `read_file` rejects all source files unconditionally, even when `start_line` +
   `end_line` are provided — the params are in the schema but silently useless for source files.

Both cases share a pattern: the agent knows **where** to look (a line range or an anchor string)
but not the symbol name. Symbol tools require conceptual knowledge (name); these two gaps arise
when the agent has positional knowledge instead.

## Principle

> **Specificity = access.** Physical file access is unlocked by specificity of request.
> `search_pattern(pattern, context_lines=N)` gives content anchored by *what you're looking for*.
> `read_file(path, start_line, end_line)` gives content anchored by *where you're looking*.
> Neither allows a whole-file dump without specifying location.

This is the inverse of the Python bypass pattern, which opens the whole file first and searches
inside it. The MCP forces agents to express intent before receiving content.

## Design

### Change 1: `read_file` — lift source-file block for targeted reads

**File:** `src/tools/file.rs` — `impl Tool for ReadFile`

**Current flow** (bug: line-range params are in schema but never reached for source files):
```
validate path → is source file? → deny  ← blocks here
                                ↓ (only non-source reaches this)
                         extract start_line/end_line → read file
```

**New flow:**
```
validate path
↓
extract start_line, end_line
↓
is source file AND no line range provided? → deny (updated hint: "specify start_line + end_line for targeted reads")
↓
read file → apply line range if set
```

The error message when denying a source file without line range is updated to explicitly say
that targeted reads (with both `start_line` and `end_line`) are allowed.

The tool description changes from:
> "Rejects source code files — use symbol tools for .rs, .py, .ts, etc."

To:
> "Source code files require `start_line` + `end_line` — use symbol tools for whole-file reads."

**Constraint:** Both `start_line` and `end_line` must be present to unlock source reads. A
single bound (e.g. `start_line` only) is not enough — it still forces explicit intent.

---

### Change 2: `search_pattern` — `context_lines` parameter

**File:** `src/tools/file.rs` — `impl Tool for SearchPattern`

**New parameter:** `context_lines: integer` (default 0, max 20)

When `context_lines == 0`: existing behavior unchanged (just the matching line, backward
compatible).

When `context_lines > 0`:

- File lines collected into a `Vec` (currently streamed as iterator — needs `collect()` for
  indexed access before/after the match).
- Adjacent matches whose context windows overlap are merged into a single block. A new match
  starts a new block only if its context_before window doesn't overlap the previous block.
- Each block is emitted as a single object with a flat multiline `content` string — no per-line
  JSON key overhead:

```json
{
  "file": "src/types/api/schema.d.ts",
  "match_line": 42,
  "start_line": 38,
  "content": "  };\n\n  [path: `/api/v1/mobile/teachers/exceptions/${string}`]: {\n    parameters: {\n      query?: never;"
}
```

- `match_line` is the 1-indexed line of the **first** pattern match in this block (for blocks
  that merge multiple matches, it's the first match's line — agents can scan `content` for
  further matches).
- `start_line` is the first line included in the block (i.e., the match line minus
  `context_lines`, clamped to 1).
- `max_results` cap applies to number of **match events** (not output lines). This is
  intentional: the agent requested context; capping on lines would silently truncate it.
- `context_lines` is capped at 20 server-side; values above 20 are silently clamped.

**Schema addition:**
```json
"context_lines": {
  "type": "integer",
  "default": 0,
  "description": "Lines of context before and after each match (max 20). Adjacent matches that share context are merged into one block."
}
```

**Description update** (append):
> "Pass `context_lines` to see surrounding code — adjacent matches sharing context are merged
> into a single block."

---

## Output comparison

Without `context_lines` (today):
```json
{ "matches": [{ "file": "...", "line": 42, "content": "  [path: `/api/v1/...`]: {" }], "total": 1 }
```

With `context_lines=5`:
```json
{
  "matches": [{
    "file": "src/types/api/schema.d.ts",
    "match_line": 42,
    "start_line": 37,
    "content": "          };\n        };\n        [path: `/api/v1/mobile/teachers/exceptions/${string}`]: {\n            parameters: {\n                query?: never;\n                header?: never;"
  }],
  "total": 1
}
```

---

## Files Changed

| File | Change |
|------|--------|
| `src/tools/file.rs` | `ReadFile::call` — reorder source-file block after line-range extraction |
| `src/tools/file.rs` | `ReadFile::description` + error hint — updated wording |
| `src/tools/file.rs` | `SearchPattern::input_schema` — add `context_lines` |
| `src/tools/file.rs` | `SearchPattern::description` — note about context |
| `src/tools/file.rs` | `SearchPattern::call` — collect lines into Vec, merge overlapping blocks |

No new files. No changes to other tools.

## Testing

- `read_file_source_with_line_range_is_allowed` — passes `start_line` + `end_line` on a `.rs`
  file; asserts success and correct content.
- `read_file_source_without_range_is_rejected` — no line range on `.rs`; asserts
  `RecoverableError` with hint mentioning `start_line` + `end_line`.
- `search_pattern_context_lines_single_match` — one match, `context_lines=2`; asserts
  `start_line`, `match_line`, multiline `content`.
- `search_pattern_context_lines_adjacent_matches_merge` — two matches 3 lines apart,
  `context_lines=2`; asserts they collapse into one block.
- `search_pattern_context_lines_non_adjacent_matches_separate` — two matches 20 lines apart;
  asserts two separate blocks.
- `search_pattern_zero_context_lines_backward_compat` — existing format unchanged.
