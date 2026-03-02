# Progressive Discoverability for Symbol Tools

**Date:** 2026-02-28
**Status:** Approved, pending implementation
**Scope:** `find_symbol`, `list_symbols`, `src/tools/output.rs`, `src/prompts/server_instructions.md`

---

## Problem

### Observed Failure — Real Session Trace

The following trace was captured from a Claude agent working in a TypeScript React Native project
(`eduplanner-mobile`) while brainstorming a new UX feature. Three tool calls failed or produced
excessive output:

```
● find_symbol("Approval")
  ⎿ 49 results across many files — agent had to manually filter noise

● list_symbols("src/screens/approval/TeacherApprovalScreen.tsx", depth: 2)
  ⎿ Error: result (94,490 characters) exceeds maximum allowed tokens.

● find_symbol("WeeklyGrid", path: "src/components")
  ⎿ ⚠ Large MCP response (~14.6k tokens)   [+401 lines]
```

All three reflect the same underlying gap: **tools return as much as they can, but don't guide the
agent toward a narrower follow-up call when the output is too large.**

### Root Causes

**`list_symbols` single-file — no output cap at all.**
When given a single file path, the tool returns every symbol unconditionally. With `depth: 2`, the
LSP symbol tree for a large React component includes every state hook, nested helper, JSX subtree,
and local variable — easily 200+ entries serialized to 94KB. There is no `cap_items` call in
single-file mode.

**`find_symbol` in directory — `collect_matching` recurses into all children.**
When a directory path is given, the tool calls `document_symbols` on each file, then recursively
walks the entire symbol tree looking for substring matches. For TypeScript, LSP `document_symbols`
includes local variables, parameters, and closures. A search for "WeeklyGrid" in `src/components`
matches every `const weeklyGrid = ...` and every callback parameter named `grid` in addition to
the actual component definition. The `early_cap` fires at 201 items but only *between files*, so
all matches from files already processed are kept.

**Overflow hint is too vague.**
The current overflow block includes `hint: "Restrict with a file path or glob pattern"`. This
tells the agent *what kind of thing* to do but not *the exact call* to make. The agent has to
infer the syntax from context.

**No visibility into result distribution.**
When 401 symbols match across many files, the agent gets the first 50 (or 200) with no idea which
files the rest came from. The correct narrowing step — re-call with a specific file — is obvious
in hindsight but requires guesswork without a file-level breakdown.

---

## Design

### Principle: Progressive Discoverability

Tools should never silently discard data. When a result set is too large to return fully:

1. Return the first N results (as now)
2. Tell the agent the total count
3. Show **where** the remaining results live (file distribution)
4. Show **how** to get them (concrete, copy-paste-ready tool call examples)

This is an extension of the existing progressive disclosure principle (exploring → focused modes)
applied one level up: **discoverability of what exists** before **reading its content**.

---

### Change 1 — Extended `OverflowInfo`

Add an optional `by_file` map to `OverflowInfo` in `src/tools/output.rs`:

```rust
pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,          // existing
    pub next_offset: Option<usize>,  // existing
    pub by_file: Option<IndexMap<String, usize>>,  // NEW
}
```

`by_file` is populated by callers that have file-scoped result sets. It maps relative file path →
count of matched symbols across the **full** result set (before truncation). Serialized into the
overflow JSON:

```json
{
  "overflow": {
    "shown": 50,
    "total": 401,
    "hint": "Showing 50 of 401. To narrow down:\n• paginate:       add offset=50, limit=50\n• filter by file: add path=\"src/components/WeeklyGrid.tsx\"\n• filter by kind: add kind=\"function\" (also: class, interface, type, enum)",
    "by_file": {
      "src/components/WeeklyGrid.tsx": 3,
      "src/components/WeeklyGridItem.tsx": 1,
      "src/screens/HomeScreen.tsx": 12
    }
  }
}
```

`by_file` is only populated when the search scope was a directory or project-wide (not a single
file). The map is sorted by count descending so the most relevant file appears first.

---

### Change 2 — `find_symbol`: `kind` filter parameter + richer overflow

**New `kind` parameter:**

```json
"kind": {
  "type": "string",
  "description": "Filter by symbol kind: function, class, interface, type, enum. Omit for all kinds.",
  "enum": ["function", "class", "interface", "type", "enum"]
}
```

When `kind` is provided, `collect_matching` skips symbols whose `SymbolKind` does not match — at
all nesting depths. This allows the agent to say "I want the component definition" and get exactly
that, not every local variable that happens to contain the name.

Internal kind mapping (TypeScript LSP kinds → our filter values):

| Filter value | LSP SymbolKinds matched |
|---|---|
| `function` | Function, Method, Constructor |
| `class` | Class, Struct |
| `interface` | Interface |
| `type` | TypeParameter, Enum (when not filtered separately) |
| `enum` | Enum, EnumMember |

**Cap reduction:** Exploring-mode cap for `find_symbol` drops from 200 → 50. The `by_file` map
compensates by showing where the unshown results live. 50 results is sufficient for the agent to
assess whether it has the right symbol or needs to narrow.

**`by_file` computation:** Collect all matches into a full list (existing behavior), build the
`by_file` count map from the full list, then truncate to 50. No extra file scanning.

**Enriched overflow hint format:**

```
Showing {shown} of {total}. To narrow down:
• paginate:       add offset={shown}, limit=50
• filter by file: add path="<top file from by_file>"
• filter by kind: add kind="function" (also: class, interface, type, enum)
```

The hint uses the top file from `by_file` as the concrete example path, making it directly
actionable.

---

### Change 3 — `list_symbols` single-file cap

Apply `cap_items` to the top-level symbol array for single-file mode. Cap at 100 top-level
entries. Children at `depth: N` are included within those 100 top-level entries (no change to
child depth handling).

When capped, the response shape becomes:

```json
{
  "file": "src/screens/approval/TeacherApprovalScreen.tsx",
  "symbols": [...],
  "total": 247,
  "overflow": {
    "shown": 100,
    "total": 247,
    "hint": "File has 247 symbols. Use depth=1 for top-level overview, or find_symbol(name_path='TeacherApprovalScreen/handleReject', include_body=true) for a specific symbol."
  }
}
```

The hint explicitly teaches the `name_path` pattern — the precise follow-up tool when the agent
already knows the symbol it wants.

Note: `by_file` is **not** included in single-file overflow (it's a single file by definition).

---

### Change 4 — `server_instructions.md` discoverability guidance

Add a new section to the server instructions that teaches the overflow→refine pattern:

```markdown
## When symbol tools return too many results

All symbol tools use progressive discoverability: when results overflow, the response includes
`overflow.by_file` (where results are distributed) and `overflow.hint` (concrete follow-up calls).

Recommended workflow:
1. Check `overflow.by_file` — pick the file most likely to contain what you want
2. Re-call with `path="that/file.tsx"` to scope the search to one file
3. Or add `kind="function"` to `find_symbol` to skip variables and local declarations
4. For a structural overview of a file: use `list_symbols(depth=1)` instead of `find_symbol`

For `list_symbols` overflow: the file has more top-level symbols than shown. Use
`find_symbol(name_path="ClassName/methodName", include_body=true)` to read a specific one.
```

---

## Tests That Motivated This Design

These test scenarios directly correspond to the failures observed in the trace. They should be
written as unit tests before the implementation (TDD).

### T1 — `find_symbol` overflow includes `by_file` map

```
GIVEN a project with 3 files each containing 30 symbols named "grid"
WHEN find_symbol(pattern="grid") is called without a path
THEN overflow.by_file contains {file_a: 30, file_b: 30, file_c: 30}
AND overflow.shown = 50
AND overflow.total = 90
AND symbols array has exactly 50 entries
```

**Why this matters:** Without `by_file`, the agent sees 50 symbols with no indication that 40
more exist in the same 3 files. It may conclude the search was exhaustive.

### T2 — `find_symbol` kind filter excludes variables

```
GIVEN a file with:
  - class WeeklyGrid { ... }
  - const weeklyGrid = new WeeklyGrid()
  - function renderWeeklyGrid() { ... }
WHEN find_symbol(pattern="weeklygrid", kind="class") is called
THEN symbols contains only "WeeklyGrid" (the class)
AND "weeklyGrid" (the variable) is excluded
AND "renderWeeklyGrid" (the function) is excluded
```

**Why this matters:** The trace showed `find_symbol("WeeklyGrid")` returning 401 results, most of
which were variable declarations and local references, not the component definition the agent needed.

### T3 — `find_symbol` overflow hint is actionable

```
GIVEN find_symbol returns an overflow
WHEN overflow.hint is inspected
THEN it contains a concrete example path from by_file
AND it mentions the kind filter option
AND it shows the next offset value for pagination
```

**Why this matters:** The original hint `"Restrict with a file path or glob pattern"` requires the
agent to infer both the path to use and the parameter syntax. The new hint must be copy-pasteable.

### T4 — `list_symbols` single file applies cap

```
GIVEN a file with 247 top-level symbols
WHEN list_symbols(path="LargeFile.tsx", depth=2) is called
THEN symbols array has exactly 100 entries
AND total = 247
AND overflow is present
AND overflow.hint contains "find_symbol" and "name_path" as refinement guidance
AND overflow does NOT contain by_file (single file context)
```

**Why this matters:** The 94KB crash in the trace was a single-file `list_symbols` call. There
was no cap at all — the test ensures the cap is enforced and the hint teaches the right follow-up.

### T5 — `list_symbols` under cap has no overflow

```
GIVEN a file with 40 top-level symbols
WHEN list_symbols(path="SmallFile.tsx") is called
THEN overflow is absent
AND symbols array has 40 entries
AND total is absent (or equals 40)
```

**Why this matters:** Ensures the cap only activates when needed — small files should not show
spurious overflow messages.

### T6 — `find_symbol` cap drop from 200 to 50

```
GIVEN a project-wide search that matches 80 symbols
WHEN find_symbol(pattern="x") is called in exploring mode
THEN symbols array has exactly 50 entries
AND overflow.shown = 50, overflow.total = 80
```

**Why this matters:** Confirms the reduced cap applies to project-wide searches.

---

## What This Does Not Change

- The `symbols` array is always present in the response, even when overflow fires — no shape changes for agents that ignore overflow.
- `BODY_CAP` (strip bodies after 5 results with `include_body=true`) is unchanged.
- `list_symbols` directory mode already uses `cap_files` — no change needed there.
- Focused mode (`detail_level: "full"`) behavior and pagination are unchanged.
- The `kind` filter is optional; omitting it preserves all current behavior exactly.

---

## Review Findings (2026-02-28)

The following issues were identified during code review against the actual implementation.

### RF1 — Early-cap path prevents accurate `by_file` and `total`

**Problem:** The directory-scoped `find_symbol` path (lines 494–543 of `symbol.rs`) uses an
`early_cap` that breaks out of the file loop once `matches.len() > max_results`. It never collects
the full result set. Two consequences:

1. `by_file` can only reflect files processed before the early exit — not the true distribution.
2. `total` is set to `max_results + 1` (line 536), not the real count.

The design's T1 test expects `overflow.total = 90` (real count), which contradicts the early-exit
optimization.

**Resolution:** Remove the early-cap for directory/glob searches. Instead, collect all matches into
a lightweight `(file_path, SymbolKind, symbol_json)` list, build `by_file` from the full list,
then truncate to cap. The performance cost is acceptable: `collect_matching` is cheap compared to
the per-file LSP `documentSymbol` call that already dominates latency. The workspace/symbol path
(project-wide, no path given) already collects all results before `cap_items`, so no change needed
there.

### RF2 — `by_file` map needs a cap

**Problem:** A project-wide `find_symbol("get")` could match across hundreds of files. The `by_file`
map itself becomes a large payload.

**Resolution:** Cap `by_file` to the top 15 files by count. Add `by_file_overflow: usize` to
indicate how many additional files were omitted. Example:

```json
{
  "by_file": { "top15files": "..." },
  "by_file_overflow": 42
}
```

### RF3 — `kind` enum is too narrow for multi-language use

**Problem:** The enum `["function", "class", "interface", "type", "enum"]` was designed for
TypeScript. Missing from coverage:

- `module` — Rust `mod`, Python `module` (LSP `SymbolKind::Module`)
- `struct` — explicit Rust struct filtering (currently lumped with `class`)
- `constant` — `const`/`static` in Rust (LSP `SymbolKind::Constant`)
- `trait` — Rust traits map to `SymbolKind::Interface`, but the name is confusing

**Resolution:** Expand the enum and mapping:

| Filter value | LSP SymbolKinds matched |
|---|---|
| `function` | Function, Method, Constructor |
| `class` | Class |
| `struct` | Struct |
| `interface` | Interface (includes Rust traits) |
| `type` | TypeParameter |
| `enum` | Enum, EnumMember |
| `module` | Module, Namespace, Package |
| `constant` | Constant |

The description should note that `interface` matches Rust `trait`.

### RF4 — `kind` should be ignored when `name_path` is used

**Problem:** `find_symbol` accepts both `pattern` (substring search) and `name_path` (exact lookup).
When using `name_path`, the user already knows exactly what they want — applying a `kind` filter
could unexpectedly return empty results if the kind doesn't match.

**Resolution:** Skip the `kind` filter when the input contains `name_path`. Only apply `kind`
filtering in the `pattern`-based search paths.

### RF5 — `list_symbols` cap at 100 may still blow up with deep `depth`

**Problem:** Capping at 100 top-level entries with `depth=2` means each entry includes its full
children tree. A file with 100 top-level symbols each having 5 children = 600 serialized objects,
potentially still very large.

**Resolution:** Use the existing `cap_items` on the top-level array (count-based, 100). This is
sufficient for the common case. The `depth` parameter already defaults to 1, and the hint will
guide agents to use `depth=1` or `find_symbol(name_path=...)` for drill-down. A size-based cap
would add complexity for diminishing returns — defer unless real-world traces show 100-top-level
+ deep-depth as a recurring failure.

### RF6 — `cap_items` API doesn't support `by_file`

**Problem:** `cap_items` returns `(Vec<T>, Option<OverflowInfo>)` but has no way to inject
`by_file` into the returned `OverflowInfo`.

**Resolution:** Callers build `by_file` externally before calling `cap_items`, then patch it into
the returned `OverflowInfo` via mutation. This keeps `cap_items` generic (it doesn't know about
symbol-specific metadata) and avoids complicating the shared `OutputGuard` API.

### Additional Tests (from review)

**T7 — `by_file` is capped at 15 entries:**
```
GIVEN find_symbol matches symbols across 30 files
WHEN overflow.by_file is inspected
THEN by_file contains at most 15 entries (sorted by count desc)
AND by_file_overflow = 15
```

**T8 — `kind` filter is ignored with `name_path`:**
```
GIVEN a file with class Foo and function bar
WHEN find_symbol(name_path="Foo", kind="function") is called
THEN symbols contains "Foo" (the class) — kind filter was ignored
```

**T9 — directory-scoped `find_symbol` reports accurate `total`:**
```
GIVEN a directory with 3 files, each containing 30 matching symbols
WHEN find_symbol(pattern="x", path="dir/") is called
THEN overflow.total = 90 (not max_results + 1)
AND overflow.by_file reflects all 3 files with accurate counts
```
