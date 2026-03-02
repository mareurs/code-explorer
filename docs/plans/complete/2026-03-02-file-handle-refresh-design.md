# Design: Auto-Refreshing `@file_*` Handles on mtime Change

**Date:** 2026-03-02
**Status:** Approved

## Problem

`read_file` on large files (> `FILE_BUFFER_THRESHOLD` lines) snapshots file content into
an `OutputBuffer` entry under a `@file_*` handle. The handle is valid for the session and
is queried via `run_command("grep/sed/awk @file_abc12345")`.

If the agent then edits the file (`edit_file`, `create_file`, or any external tool —
git checkout, other MCP server), subsequent queries through the handle silently return
stale content. The agent may then make decisions based on outdated data, producing
incorrect edits.

The `BufferEntry.command` field already stores the source file path for `@file_*` entries,
giving us everything needed for a fix without additional write-side coordination.

## Decisions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Refresh trigger | Lazy mtime check in `get()` | Single choke point; catches all mutations (internal and external) |
| File deleted at refresh | Return `None` (cache miss) | Clean failure; existing error message guides agent |
| mtime unavailable (rare FS) | Use cached content (silent degradation) | Safe fallback for unusual filesystems |
| Coupling to write tools | None | No changes to `edit_file`, `create_file`, or any other tool |

## Design

### `BufferEntry` Schema Change

Add `source_path: Option<PathBuf>` to `BufferEntry`:

```rust
pub struct BufferEntry {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timestamp: u64,
    pub source_path: Option<PathBuf>,  // NEW: set only for @file_* entries
}
```

- `store_file()` sets `source_path = Some(PathBuf::from(&path))`
- `store()` and `store_tool()` set `source_path = None`
- `source_path` is the typed, explicit intent marker; `command` retains its existing
  dual-semantic role (command text for `@cmd_*`, file path string for `@file_*`)

### `get()` Behavior

```
get(id: &str) -> Option<BufferEntry>

  If entry.source_path.is_some():
    stat the file
    if not found / unreadable → return None
    if mtime > entry.timestamp:
      re-read file content
      update entry.stdout and entry.timestamp in-place (under the mutex)
    if mtime unavailable → fall through, return cached entry
  
  Return cloned entry (refreshed or cached)
```

The refresh happens inside the existing `Mutex<BufferInner>` lock, so it is
automatically serialized against concurrent `get()` calls.

`@cmd_*` and `@tool_*` entries (`source_path = None`) are unaffected.

### Files Changed

- `src/tools/output_buffer.rs` — only file touched:
  - `BufferEntry`: add `source_path` field
  - `store_file()`: set `source_path`
  - `store()`, `store_tool()`: set `source_path: None` 
  - `get()`: add mtime-refresh logic for `source_path.is_some()` entries

### Testing

**TDD — test before implementation.**

Three-step sandwich for the core regression test:

1. `store_file(path, old_content)` → get handle
2. Overwrite file on disk with `new_content` (bypass normal path)
3. `get(handle)` → assert content is still old (proves staleness exists)
4. (No explicit invalidation step — mtime check is automatic)
5. `get(handle)` again after disk write mtime advances → assert content is fresh

Additional cases:
- `get()` on deleted file → `None`
- `get()` on unmodified file → returns cached entry (no re-read)
- `@cmd_*` and `@tool_*` handles unaffected by new logic

## Out of Scope

- Refreshing `@cmd_*` handles (would require re-executing the command)
- Refreshing `@tool_*` handles (would require re-calling the tool)
- Push-based invalidation from write tools (Option B/C — rejected for simplicity)
- Surfacing staleness as a warning annotation (Option 3 — rejected for simplicity)
