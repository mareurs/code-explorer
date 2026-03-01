# Progressive Discoverability — Design Guidance

**Status:** Living document. Updated as patterns are discovered or LLM behavior changes.
**Audience:** Anyone adding or modifying tools in code-explorer.
**Last updated:** 2026-02-28

---

## What This Document Is

This is the canonical reference for how tools in code-explorer should handle output sizing,
overflow, and agent guidance. Every tool that returns variable-length data must follow these
patterns. The goal: **an LLM should never receive a tool response that leaves it stuck** — every
response must either contain the answer or teach the LLM exactly how to get it.

This is not about truncation or error handling. It's about **making the tool output itself a
navigation aid** when the full result doesn't fit.

---

## Core Principle

> **Tools should never silently discard data. When a result set is too large, return a useful
> subset, tell the agent what exists beyond that subset, and show exactly how to get it.**

This has three layers, each building on the previous:

### Layer 1: Progressive Disclosure (implemented)

Two output modes control detail level:

| Mode | Default? | Behavior |
|------|----------|----------|
| **Exploring** | Yes | Compact representation, capped at N items. No bodies, minimal fields. |
| **Focused** | `detail_level: "full"` | Full detail, paginated via `offset`/`limit`. Bodies included. |

Enforced via `OutputGuard` (`src/tools/output.rs`). This is a project-wide pattern — tools call
`guard.cap_items()` or `guard.cap_files()`, not custom truncation logic.

**Pattern:** Always default to exploring. Focused mode is opt-in after the agent knows what it wants.

### Layer 2: Progressive Discoverability (this document)

When exploring mode triggers an overflow, the response doesn't just say "there's more" — it tells
the agent **where** the remaining results are and **how** to get them:

```json
{
  "symbols": [...50 items...],
  "total": 401,
  "overflow": {
    "shown": 50,
    "total": 401,
    "hint": "Showing 50 of 401. To narrow down:\n• paginate: add offset=50, limit=50\n• filter by file: add path=\"src/components/WeeklyGrid.tsx\"\n• filter by kind: add kind=\"function\"",
    "by_file": [
      {"file": "src/components/WeeklyGrid.tsx", "count": 42},
      {"file": "src/screens/HomeScreen.tsx", "count": 12}
    ]
  }
}
```

### Layer 3: Tool Selection Guidance (server instructions)

The `server_instructions.md` prompt teaches the LLM which tool to reach for based on what it
knows. This is the outermost layer — it guides *before* the tool is called, while Layers 1–2
guide *after*.

---

## Patterns

### Pattern 1: Overflow Must Be Actionable

**Every overflow hint must be a concrete, copy-paste-ready tool call example.**

Bad:
```
"hint": "Restrict with a file path or glob pattern"
```

Good:
```
"hint": "Showing 50 of 401. To narrow down:\n• paginate: add offset=50, limit=50\n• filter by file: add path=\"src/components/WeeklyGrid.tsx\"\n• filter by kind: add kind=\"function\""
```

**Why:** Claude Code processes tool output as text. A vague hint requires the LLM to infer
parameter names and valid values from memory. A concrete hint can be acted on immediately. In
testing, vague hints cause the agent to either repeat the same broad query or give up. Concrete
hints produce successful follow-up calls on the first try.

**Rule:** The hint must contain at least one parameter name with a real value from the current
result set.

### Pattern 2: Show Distribution Before Content

When results span multiple files, include a file distribution map so the agent can pick the right
file without reading all results.

```json
"by_file": [
  {"file": "src/components/WeeklyGrid.tsx", "count": 42},
  {"file": "src/screens/HomeScreen.tsx", "count": 12},
  {"file": "src/utils/grid.ts", "count": 1}
]
```

**Implementation rules:**
- **Array format**, sorted by count descending. Arrays preserve order; JSON objects don't.
- **Cap at 15 entries.** Include `by_file_overflow: N` when files are omitted.
- **Only for multi-file contexts.** Single-file tools should not include `by_file`.
- **Built from the full result set** before truncation. The agent needs to see where ALL results
  live, not just the ones that fit.
- **Internal type:** `Vec<(String, usize)>`. No `IndexMap` dependency — serialize manually in
  `overflow_json`.

### Pattern 3: Filters Reduce Before Caps Truncate

Offer filtering parameters (like `kind`) that reduce the result set at the source, before the
exploring cap truncates it. This is more efficient than pagination for the common case where the
agent wants a specific type of result.

```
find_symbol(pattern="Grid", kind="class")  →  3 results  (no overflow)
find_symbol(pattern="Grid")                →  401 results (overflow, most are variables)
```

**The kind filter must:**
- Apply inside the recursive traversal (e.g., `collect_matching`), not after.
- Skip non-matching symbols but still recurse into their children (a class may not match
  `kind="function"`, but its methods will).
- Be ignored for exact lookups (`name_path` parameter) where the user already knows what they want.

### Pattern 4: Caps Should Be Tight in Exploring Mode

The exploring cap should be the **smallest number that lets the agent decide whether to narrow or
paginate.** Larger caps waste tokens on results the agent won't read.

| Tool | Exploring Cap | Rationale |
|------|---------------|-----------|
| `find_symbol` | 50 results | Agent scans names + files to pick the right one |
| `list_symbols` (multi-file) | 50 files | Agent scans file list to pick where to drill |
| `list_symbols` (single-file) | 100 top-level symbols | Agent scans structure to find target |
| `search_pattern` | 200 lines | Regex matches need more context to evaluate |
| `semantic_search` | 10 chunks | Ranked results; top 10 is almost always sufficient |
| `list_dir` | 200 entries | Directory listings are compact |

**Why 50 for `find_symbol`, not 200?** The original 200 cap was set before `by_file` existed.
With `by_file`, the agent gets a complete file distribution even when only 50 symbols are returned.
The 50 shown results are for the agent to verify it's searching for the right thing; `by_file`
tells it where to look next. 200 results were never read — they were skimmed at best.

### Pattern 5: Errors Are Hints Too

`RecoverableError` (non-fatal, `isError: false`) should always include a corrective hint. This is
the same principle as overflow hints — don't just say what went wrong, say what to do instead.

Bad:
```json
{"error": "path not found"}
```

Good:
```json
{"error": "path not found: src/compnents", "hint": "Did you mean src/components? Use list_dir('.') to see available directories."}
```

This pattern is already established in the codebase (`RecoverableError::with_hint`). Extend it to
every recoverable failure.

### Pattern 6: Never Exceed MCP Output Limits

Claude Code warns at 10,000 tokens and hard-caps at 25,000 tokens (configurable via
`MAX_MCP_OUTPUT_TOKENS`). A tool that regularly exceeds this is broken, even if the data is valid.

**Defenses in depth:**
1. **Exploring mode caps** (Pattern 4) — first line of defense.
2. **BODY_CAP** — `find_symbol` with `include_body=true` strips bodies after the first 5 results.
3. **Single-file cap** — `list_symbols` on a single file caps at 100 top-level symbols.
4. **Pagination** — focused mode (`detail_level: "full"`) returns pages of `limit` items.

If a tool can produce unbounded output, it **must** use `OutputGuard`. No exceptions.

---

## Anti-Patterns

### Anti-Pattern 1: Early Exit Without Metadata

Stopping result collection early (e.g., `break` after N matches) prevents computing accurate
`total` and `by_file`. The `total` becomes a lower bound ("at least N+1"), which misleads the
agent about result set size.

**Exception:** The workspace/symbol LSP request is naturally bounded by the LSP server. No early
exit needed.

**If early exit is truly needed for performance:** At minimum, continue a lightweight counting
pass (file + kind only, no body extraction) to get accurate `total` and `by_file`.

### Anti-Pattern 2: Per-Tool Overflow Logic

Don't write custom overflow handling in each tool. Use `OutputGuard::cap_items()` and
`OutputGuard::cap_files()`. They handle both exploring and focused modes, produce consistent
`OverflowInfo`, and ensure all tools overflow the same way.

**Exception:** Single-file caps (like `list_symbols`) where the overflow hint is file-specific
and can't be generated by the generic `cap_items`. Even then, construct a proper `OverflowInfo`
and use `OutputGuard::overflow_json()` for serialization.

### Anti-Pattern 3: Returning the Same Hint for Every Tool

Each tool should construct hints relevant to **its own parameters**. `find_symbol` hints mention
`kind` and `path`; `list_symbols` hints mention `depth` and `find_symbol(name_path=...)`;
`semantic_search` hints mention `path` filtering. Generic hints like "narrow your search" are
useless.

### Anti-Pattern 4: Pagination as the Only Narrowing Strategy

Pagination (`offset`/`limit`) is the least efficient narrowing strategy — the agent reads N items,
decides none are right, reads N more, repeat. Always offer domain-specific filters first:

1. **Filter** (`path`, `kind`, `query` refinement) — eliminates irrelevant results
2. **Scope** (`by_file` → pick a file) — narrows to the right location
3. **Pagination** (`offset`/`limit`) — last resort for scanning large result sets

The overflow hint should present these in this order.

---

## How Claude Code Processes Tool Output

Understanding the consumer is essential for designing good tool output. Key behaviors:

1. **Top-down reading.** Claude reads tool output sequentially. The first items in an array and
   the first keys in an object get the most attention. Put the most relevant data first.

2. **Token budget pressure.** Each tool call's output consumes context window. Claude Code warns
   at 10K tokens. Large outputs compress the conversation history, losing earlier context. Compact
   defaults aren't just nice — they're critical for multi-step workflows.

3. **Parallel tool calls.** Claude Code often calls multiple tools in parallel. If one returns
   `isError: true`, all sibling calls are aborted. This is why `RecoverableError` (which sets
   `isError: false`) is preferred for expected failures — it lets sibling calls complete.

4. **Hint-following behavior.** When overflow includes a concrete `hint`, Claude reliably uses
   the exact parameters suggested. When the hint is vague, Claude either guesses parameters
   (often wrong) or abandons the approach. Concrete hints have dramatically better follow-through.

5. **`by_file` as decision aid.** When the LLM sees a file distribution, it can match file paths
   against its understanding of the project (from prior `list_dir` calls or its conversation
   context) to pick the right file. This is faster and more reliable than scanning 50 result
   items.

---

## Human-Facing Output

The `format_for_user()` method produces output shown to the human watching Claude work in
the Claude Code terminal. This is separate from the JSON returned to the LLM.

**The rule:** If a tool fetches data, its `format_for_user()` must show at least a compact
preview of that data — not just a count.

**Why:** Counts are metadata. The human cannot tell from `"7 topics"` whether Claude found
the right topic, or from `"12 refs"` where those references are. Showing data lets the
human verify Claude is on the right track without inspecting the LLM's full conversation
context.

**What "compact preview" means:**
- Collections (topics, libraries, references): first 5–8 items, then `… +N more`
- Memory content: full content (it's already fetched; hiding it helps nobody)
- Stats tables: cap at 10 rows, sort by most-relevant metric descending
- Author breakdowns: cap at 5 entries sorted by line count descending

**Anti-pattern:** Count-only output when data is available.
```
Bad: `"7 topics"` — count with no names
Bad: `"12 refs"` — count with no locations
Good: `"7 topics\n  architecture\n  conventions\n  …"`
```

---

## Checklist for New Tools

When adding a tool that returns variable-length data:

- [ ] Use `OutputGuard::from_input(&input)` to parse `detail_level`, `offset`, `limit`
- [ ] Call `guard.cap_items()` or `guard.cap_files()` before returning
- [ ] Construct an overflow hint with concrete parameter examples using values from the actual results
- [ ] If the tool searches across files, compute `by_file` from the full result set before capping
- [ ] If the tool has domain-specific filters, mention them in the overflow hint
- [ ] Test: result under cap → no overflow block
- [ ] Test: result over cap → overflow has correct `shown`, `total`, actionable `hint`
- [ ] Test: the tool never exceeds ~25K tokens of output in any realistic scenario
- [ ] Add the overflow→refine pattern to `server_instructions.md` if this is a new tool category
- [ ] If the tool fetches data, does `format_for_user()` preview that data (not just count it)?
- [ ] Is the preview capped (5–8 items) to avoid verbosity?
- [ ] Is there a `… +N more` trailer when items are omitted?

---

## File References

| File | Role |
|------|------|
| `src/tools/output.rs` | `OutputGuard`, `OverflowInfo`, `cap_items`, `cap_files`, `overflow_json` |
| `src/tools/symbol.rs` | `collect_matching`, `build_by_file`, `matches_kind_filter` |
| `src/prompts/server_instructions.md` | LLM-facing tool guidance (sent at connection time) |
| `docs/plans/2026-02-25-progressive-disclosure-design.md` | Original two-mode design |
| `docs/plans/2026-02-28-progressive-discoverability-design.md` | Extended design for symbol tools |
| `docs/plans/2026-02-28-progressive-discoverability-impl.md` | Implementation plan |

---

## Changelog

- **2026-02-28:** Initial version. Codified from progressive discoverability design review.
  Covers overflow hints, `by_file`, kind filters, cap rationale, and Claude Code behavior model.
