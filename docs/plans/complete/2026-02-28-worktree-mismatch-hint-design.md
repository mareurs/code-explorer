# Design: Worktree Mismatch Hint in Write Tools

**Date:** 2026-02-28
**Status:** Approved

## Problem

When the code-explorer MCP server is started with `--project /main/repo` and an agent
is working in a git worktree (e.g. `.worktrees/feature-branch`), all write tools silently
operate on the **main repo**. The agent passes a relative path like
`src/prompts/server_instructions.md`, which resolves to `<main-repo>/src/...` rather
than the intended `<worktree>/src/...`. No error is raised; the file exists and is under
the project root, so it passes security validation.

The fix in `server_instructions.md` describes this as "HARD-BLOCKED", which is false —
the block is documentation-only and the bug still exists in code.

## Goal

After every successful write, if git linked worktrees exist under the active project root,
include an advisory `"worktree_hint"` field in the response JSON. The agent can inspect
this hint and call `activate_project(worktree_path)` before retrying if needed.

## Non-Goals

- Blocking writes (writes to the main repo are sometimes intentional)
- Detecting which specific worktree the agent is "in" (unknowable from MCP server)
- Changing the security model

## Approach

**Option chosen:** Separate helper called from each write tool.

Keep `validate_write_path` signature unchanged. Add a standalone
`worktree_hint(project_root: &Path) -> Option<String>` in `src/util/path_security.rs`.
Each of the 5 write tools calls it after resolving `root`, and merges the hint into the
success JSON if `Some`. Zero overhead in the common case (no worktrees).

### Detection

Read `.git/worktrees/` under `project_root`. Each subdirectory corresponds to one linked
worktree. Parse `<entry>/gitdir` to reconstruct the absolute worktree root path. Pure
filesystem I/O; no git2 dependency.

```
<project_root>/.git/worktrees/<name>/gitdir
```
File content: `/abs/path/to/worktree/.git\n`
Worktree root: parent of that path.

Returns `None` (fast path) if `.git/worktrees/` does not exist or is empty.

### Response field

```json
{
  "status": "ok",
  "path": "/home/user/repo/src/tools/file.rs",
  "lines_deleted": 3,
  "lines_inserted": 4,
  "new_total_lines": 519,
  "worktree_hint": "Wrote to main project root. Git worktrees detected: [/home/user/repo/.worktrees/feat]. If working in a worktree, call activate_project(\"/home/user/repo/.worktrees/feat\") first."
}
```

### Scope

All 5 write tools: `edit_lines`, `create_file`, `replace_symbol`, `insert_code`,
`rename_symbol`.

`replace_symbol` and `insert_code` currently omit `path` from their response — add it
alongside the hint for transparency.

## Files Changed

| File | Change |
|------|--------|
| `src/util/path_security.rs` | Add `worktree_hint()` + `list_git_worktrees()` + 2 tests |
| `src/tools/file.rs` | `edit_lines`, `create_file`: call hint, include in response |
| `src/tools/symbol.rs` | `replace_symbol`, `insert_code`, `rename_symbol`: call hint, include in response; add `path` to replace/insert responses |
| `src/prompts/server_instructions.md` | Fix false "HARD-BLOCKED" claim |

## Tests

- `worktree_hint_none_when_no_worktrees` — no `.git/worktrees/` → `None`
- `worktree_hint_some_when_worktrees_exist` — valid `gitdir` file → `Some(message with path)`

## Pre-existing unrelated change to commit first

`src/server.rs` has an unstaged fix (LSP `-32800` RequestCancelled → recoverable error).
Commit this separately before implementing the worktree hint.
