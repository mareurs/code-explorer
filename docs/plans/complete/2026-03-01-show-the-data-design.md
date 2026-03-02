# "Show the Data" — Human-Facing Tool Output Design

**Date:** 2026-03-01  
**Status:** Approved  
**Scope:** Focused cleanup pass — 9 hollow tools + PROGRESSIVE_DISCOVERABILITY.md update

---

## Problem

The human-facing output (`format_for_user()`) for 9 tools shows metadata — counts, file
names, character counts — instead of the actual data the tool fetched. When a developer
watches Claude work in Claude Code, these outputs give no signal about whether the tool
found what it was looking for.

**Tier 3 — Hollow one-liners (the problem):**

| Tool | Current human output | What's missing |
|------|---------------------|----------------|
| `find_references` | `12 refs` | Any locations at all |
| `git_blame` | `src/main.rs · 42 lines · 5 authors` | The author breakdown |
| `list_functions` | `src/main.rs → 12 functions` | The function signatures |
| `list_docs` | `src/lib.rs → 8 docstrings` | The docstring content |
| `list_memories` | `7 topics` | The topic names |
| `list_libraries` | `5 libraries` | The library names + index status |
| `read_memory` | `topic · 1234 chars` | The actual content |
| `get_usage_stats` | raw JSON dump | A formatted summary |
| `index_status` | `42 files · 1234 chunks` | Model name, timestamp, drift |

---

## The Rule

> **If a tool fetches data, its `format_for_user()` must show at least a compact preview
> of that data — not just a count.**

**Corollary:** Counts are metadata, not content. `"7 topics"` is metadata. The topic
names are data. If the LLM receives the names, the human should see them too.

This rule is added to `docs/PROGRESSIVE_DISCOVERABILITY.md` as a new section alongside
the existing LLM-facing patterns, and becomes a checklist item for all new tools.

---

## New Formats

### `list_memories` — was: `"7 topics"`

Show all topic names on separate lines (they're typically compact):

```
7 topics

  architecture
  conventions
  debugging/async-patterns
  domain-glossary
  gotchas
  onboarding
  project-overview
```

Cap at 20 entries if more. No overflow hint needed for memories (there won't be 20+).

---

### `list_libraries` — was: `"5 libraries"`

Show library names with index status:

```
5 libraries

  serde          indexed
  tokio          indexed
  anyhow         not indexed
  clap           not indexed
  rusqlite       indexed
```

Two columns: name (left) + status (right). Status is `indexed` or `not indexed`.

---

### `find_references` — was: `"12 refs"`

Show first 5 locations, then a `… +N more` trailer:

```
12 refs

  src/tools/symbol.rs:142
  src/tools/symbol.rs:198
  src/server.rs:87
  src/agent.rs:210
  src/main.rs:45
  … +7 more
```

Cap at 5 visible locations. If ≤5 refs total, no trailer.

---

### `read_memory` — was: `"topic · 1234 chars"`

Show topic name followed by full content (it's already fetched; hiding it helps nobody):

```
architecture

  ## Layer Structure
  
  Agent → Server → Tools → LSP/Embed/Git
  
  ...
```

No cap — memory entries are short by design. Indent content 2 spaces.

---

### `list_functions` — was: `"src/main.rs → 12 functions"`

Show first 8 signatures per file, then `… +N more`:

```
src/tools/symbol.rs — 12 functions

  fn collect_matching(...)
  fn build_by_file(...)
  fn matches_kind_filter(...)
  async fn find_symbol_lsp(...)
  fn list_symbols_single_file(...)
  fn list_symbols_multi_file(...)
  async fn goto_definition_lsp(...)
  fn format_find_symbol(...)
  … +4 more
```

For multi-file results: group by file with the same structure.
Cap at 8 per file; 5 files max in compact mode.

---

### `list_docs` — was: `"src/lib.rs → 8 docstrings"`

Show first 3 docstrings per file (first line of each), then `… +N more`:

```
src/tools/output.rs — 8 docstrings

  OutputGuard: enforces progressive disclosure across all tools
  cap_items: truncate to exploring-mode limit and produce OverflowInfo
  cap_files: file-level capping for multi-file result sets
  … +5 more
```

First line only (truncated to 80 chars if longer). Cap at 3 per file.

---

### `git_blame` — was: `"src/main.rs · 42 lines · 5 authors"`

Keep summary line, add author breakdown with line counts:

```
src/main.rs · 42 lines

  marius        31 lines
  dependabot     8 lines
  other-author   3 lines
```

Sort by line count descending. Cap at 5 authors. If 1 author, omit the breakdown.

---

### `get_usage_stats` — was: raw JSON

Formatted table with tool call stats. Show tools with >0 calls, sorted by call count:

```
usage · last 1h

  tool                 calls   errors   p50ms
  ─────────────────────────────────────────
  find_symbol             47        0      12
  list_symbols            23        0       8
  run_command             18        2     340
  semantic_search         12        0      95
  read_file                9        0       4
  … +6 more tools
```

Header: window duration. Columns: name, calls, errors, p50 latency. Cap at 10 rows.

---

### `index_status` — was: `"42 files · 1234 chunks"`

Add model name, last-indexed timestamp, and drift info:

```
42 files · 1234 chunks · text-embedding-3-small · 2026-03-01 14:22
```

If stale:
```
42 files · 1234 chunks · text-embedding-3-small · 2026-03-01 14:22 · 5 commits behind
```

If not indexed:
```
not indexed
```

One line is enough — the summary line is already compact.

---

## What Does NOT Change

All Tier 1 and Tier 2 tools are excluded. Their human output is already showing data:

- `read_file`, `find_symbol`, `list_symbols`, `search_pattern` — rich multi-line output
- `edit_file`, `replace_symbol`, `remove_symbol`, `insert_code` — ANSI colored diffs
- `rename_symbol`, `run_command`, `goto_definition`, `hover` — compact but informative
- `semantic_search`, `list_dir`, `find_file` — already show actual results
- `onboarding`, `create_file`, `activate_project`, `get_config` — status-only is correct
- `write_memory`, `delete_memory`, `index_project`, `index_library` — status-only is correct

---

## Documentation Update

Add a new section to `docs/PROGRESSIVE_DISCOVERABILITY.md`:

### "Human-Facing Output" Section

```markdown
## Human-Facing Output

The `format_for_user()` method produces output shown to the human watching Claude work.
This is separate from the JSON returned to the LLM.

**The rule:** If a tool fetches data, its `format_for_user()` must show at least a compact
preview of that data — not just a count.

**Why:** Counts are metadata. The human cannot tell from `"7 topics"` whether Claude found
the right topic, or from `"12 refs"` where those references are. Showing data lets the
human verify Claude is on the right track without inspecting the LLM's full context.

**What "compact preview" means:**
- Collections: first 5–8 items, then `… +N more`
- Memory content: full content (it's already fetched)
- Stats: formatted table capped at 10 rows
- Blame: author breakdown capped at 5 entries

**Checklist for new tools:**
- [ ] Does `format_for_user()` show actual data, not just a count?
- [ ] Is the preview capped (5–8 items) to avoid verbosity?
- [ ] Is there a `… +N more` trailer when items are omitted?
```

---

## Implementation Plan Summary

9 files to change, all in `src/tools/`:

| File | Tools to update |
|------|----------------|
| `src/tools/memory.rs` | `list_memories`, `read_memory` |
| `src/tools/library.rs` | `list_libraries` |
| `src/tools/symbol.rs` | `find_references` |
| `src/tools/ast.rs` | `list_functions`, `list_docs` |
| `src/tools/git.rs` | `git_blame` |
| `src/tools/config.rs` | `get_usage_stats`, `index_status` |
| `src/tools/user_format.rs` | New helper functions for each |
| `docs/PROGRESSIVE_DISCOVERABILITY.md` | New "Human-Facing Output" section |

All changes are in `format_for_user()` only — no changes to `call()`, the LLM-facing
JSON, or the `OutputGuard` system.

---

## Success Criteria

- A developer watching Claude use any of these 9 tools can see *what* was found, not just *how many*
- No tool's human output exceeds ~20 lines for typical inputs
- The rule is documented and in the new-tool checklist
