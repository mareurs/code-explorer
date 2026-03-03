# Tool Restructure Design — 32 → 23 Tools

**Date:** 2026-03-03  
**Status:** Approved  
**Approach:** Cuts + naming pass (Approach B)

## Goal

Reduce cognitive overhead for LLMs navigating the tool surface. Eliminate redundant tools, merge related operations under shared entry points, and rename the one tool whose scope changed substantially. No file-tool surgery — the file I/O group stays intact.

---

## Final Tool Inventory (23 tools)

| Group | Tools |
|---|---|
| **File I/O** | `read_file`, `list_dir`, `search_pattern`, `create_file`, `find_file`, `edit_file` |
| **Workflow** | `run_command`, `onboarding` |
| **Symbol/LSP** | `find_symbol`, `list_symbols`, `find_references`, `goto_definition`, `hover`, `replace_symbol`, `remove_symbol`, `insert_code`, `rename_symbol` |
| **Semantic** | `semantic_search`, `index_project` |
| **Memory** | `memory` |
| **Config/Nav** | `activate_project`, `project_status`, `list_libraries` |

---

## Removals & Migrations

| Removed Tool | Replacement | Rationale |
|---|---|---|
| `list_functions` | `list_symbols` | LSP covers everything tree-sitter did, plus hierarchy |
| `list_docs` | `list_symbols(include_docs: true)` | Docstrings are symbol metadata, not a separate concern |
| `index_library` | `index_project(scope: "lib:name")` | Identical pipeline; scope param already exists on sibling tools |
| `write_memory` | `memory(action: "write", …)` | 4 CRUD tools → 1 dispatched tool |
| `read_memory` | `memory(action: "read", …)` | |
| `list_memories` | `memory(action: "list")` | |
| `delete_memory` | `memory(action: "delete", …)` | |
| `git_blame` | `run_command("git blame …")` | OutputBuffer makes raw git output queryable; structured JSON not worth a dedicated slot |
| `index_status` | `project_status` | Index health belongs in the project state snapshot |
| `get_usage_stats` | `project_status` | Telemetry belongs in the project state snapshot |
| `get_config` | → **renamed** `project_status` | Scope grew substantially; name must reflect new breadth |

---

## Parameter Changes

### `list_symbols` — new params

```
include_docs: bool   (default false)
    When true, include docstrings for each symbol.
    Replaces list_docs.
    Default output stays compact (progressive disclosure).
```

Description update: *"List symbols in a file or directory; pass `include_docs: true` for docstrings (replaces `list_docs`) — signatures included by default (replaces `list_functions`)."*

### `index_project` — new param

```
scope: string   (default "project")
    "project"      — index the active project (existing behavior)
    "lib:<name>"   — index a registered library (replaces index_library)
```

Description update: *"Build or update the semantic search index; use `scope: 'lib:name'` to index a library (replaces `index_library`)."*

### `memory` — new tool (action dispatch)

```
action:  "read" | "write" | "list" | "delete"   (required)
topic:   string    (required for read / write / delete)
content: string    (required for write)
private: bool      (default false)
```

Description: *"Persistent project memory — `action`: `read`, `write`, `list`, `delete`."*

Internally: the four existing impl blocks stay intact, dispatched from a single `call()` entry point.

### `project_status` — renamed from `get_config`, enriched output

No new input params. Output gains three new sections:

```json
{
  "project_root": "…",
  "config": { … },
  "index": {
    "chunks": N,
    "files": N,
    "last_updated": "…",
    "drift_files": N
  },
  "usage": {
    "total_calls": N,
    "top_tools": [{ "name": "…", "calls": N }]
  },
  "libraries": {
    "count": N,
    "indexed": M
  }
}
```

`libraries` is a summary only — use `list_libraries` for the full registry detail.

### `list_libraries` — description update only

*"List registered libraries and their index status. Use `scope: 'lib:name'` in `semantic_search`, `find_symbol`, or `index_project` to target a library."*

This closes the loop between discovery and usage.

---

## Implementation Order

### Phase 1 — Additive (no breakage)
1. Add `include_docs` param to `list_symbols` — fold `list_docs` tree-sitter logic in
2. Add `scope` param to `index_project` — fold `index_library` dispatch in
3. Add `memory` tool as dispatch wrapper over existing 4 impl blocks

### Phase 2 — Removals (after Phase 1 passes tests)
4. Remove from `server.rs`: `list_functions`, `list_docs`, `index_library`, `write_memory`, `read_memory`, `list_memories`, `delete_memory`, `git_blame`
5. Remove from path security gates (`check_tool_access`)
6. Update `server_registers_all_tools` test to expect 23 tools

### Phase 3 — Rename + enrich
7. Rename `get_config` → `project_status`
8. Fold `index_status` output into `project_status` response
9. Fold `get_usage_stats` output into `project_status` response
10. Update `list_libraries` description
11. Update `src/prompts/server_instructions.md` navigation guide — remove old tool refs, add `memory(action:)` entry

---

## Testing

| Change | Test coverage needed |
|---|---|
| `list_symbols(include_docs: true)` | 1 new case: verify docstrings appear in output |
| `index_project(scope: "lib:name")` | Migrate existing `index_library` tests to new param |
| `memory(action: …)` | 4 cases (one per action), reuse existing memory fixtures |
| `project_status` | Verify all 4 sections present: config, index, usage, libraries |
| Removed tools | `server_registers_all_tools` now expects 23 — implicit regression coverage |

**No migration shims.** This is an internal tool server, not a versioned public API. Removed tools are deleted cleanly.

---

## Naming Convention Note

`project_status` follows the `noun_verb/noun_noun` pattern used elsewhere (`git_blame`, `index_project`, `semantic_search`). The rename signals the tool's new scope without introducing a new convention.
