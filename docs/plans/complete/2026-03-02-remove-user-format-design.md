# Design: Remove user_format module and Role::User dual-audience infrastructure

**Date:** 2026-03-02
**Status:** Approved

## Problem

`src/tools/user_format.rs` was built to support two goals:

1. **LLM-facing buffer summaries** — `format_compact` on each tool calls a `user_format::fmt_xyz`
   function to produce a compact text summary when tool output is buffered (> threshold bytes).
   This works and is actively used.

2. **User-facing ANSI output** — 5 tools (`CreateFile`, `ReplaceSymbol`, `RemoveSymbol`,
   `InsertCode`, `RenameSymbol`) override `call_content` to emit a `Role::User` content block
   with ANSI-formatted diffs/previews, intended for display in the terminal without polluting
   the LLM's context.

Goal 2 never worked. Claude Code does not filter content blocks by audience
(issue #13600, open). The blocks reach the LLM rather than the terminal. We compensated by
adding a `blocks.retain(...)` filter in `server.rs` — but that filter was accidentally removed
in `ccbd0ac` ("no Role::User blocks generated anymore"), causing silent correctness bugs where
ANSI blobs leaked into the LLM's context. We restored the filter, but the infrastructure is
dead weight: the blocks are built, serialized, filtered, and discarded every call.

Beyond correctness, the `user_format` indirection adds friction: `format_compact` bodies are
just thin wrappers delegating to a distant module, making it harder to understand what a tool
actually emits.

## Decision

Remove the dual-audience infrastructure entirely. Collapse `user_format.rs` into the tool
files that own the formatting logic.

## What Gets Deleted

- **5 `call_content` overrides** in `file.rs` (CreateFile) and `symbol.rs` (ReplaceSymbol,
  RemoveSymbol, InsertCode, RenameSymbol). These tools fall back to the default `call_content`
  in the `Tool` trait, which returns compact JSON for small results and calls `format_compact`
  for buffered large results. LLM-visible behavior is identical.

- **`render_diff_header`, `render_edit_diff`, `render_removal_diff`, `render_insert_diff`**
  in `user_format.rs` — only used by the deleted `call_content` overrides.

- **`blocks.retain(|b| b.audience() != ...)` filter + `Role` import** in `server.rs` —
  defensive but dead once nothing emits `Role::User` blocks.

- **Audience-split tests** — `create_file_call_content_returns_two_audience_blocks` and any
  test asserting `Role::User` audience on tool output.

- **`src/tools/user_format.rs`** — the entire file, after all live functions have been moved.

## What Gets Moved

Every top-level `pub fn format_xxx` in `user_format.rs` has exactly one external caller.
Each moves as a **private `fn`** into the tool file that calls it:

| Function | Destination |
|---|---|
| `format_read_file`, `format_list_dir`, `format_search_pattern`, `format_find_file` | `file.rs` |
| `format_list_symbols`, `format_find_symbol`, `format_find_references`, `format_goto_definition`, `format_hover`, `format_replace_symbol`, `format_remove_symbol`, `format_insert_code`, `format_rename_symbol` | `symbol.rs` |
| `format_git_blame` | `git.rs` |
| `format_list_functions`, `format_list_docs` | `ast.rs` |
| `format_semantic_search`, `format_index_project`, `format_index_status`, `format_index_library` | `semantic.rs` |
| `format_onboarding`, `format_run_command` | `workflow.rs` |
| `format_read_memory`, `format_list_memories`, `format_get_config`, `format_activate_project`, `format_get_usage_stats`, `format_list_libraries` | their respective tool files (`memory.rs`, `config.rs`, etc.) |

Internal helper functions that are called by format functions landing in **different tool
files** move to a new **`src/tools/format.rs`** module:

- `truncate_path`
- `format_line_range`
- `format_overflow`
- `common_path_prefix`
- `format_symbol_tree`
- `format_read_file_summary`
- `format_search_simple_mode`
- `format_search_context_mode`

Helpers used only within a single destination file stay private to that file.

## What Stays Unchanged

- All `format_compact` implementations on every tool — they remain the sole path for
  LLM-facing compact summaries.
- The `format_compact` method signature on the `Tool` trait.
- The default `call_content` in the `Tool` trait.

## Result

- `user_format.rs` is gone.
- `src/tools/format.rs` contains only genuinely shared low-level helpers.
- Each tool file is self-contained: its `format_compact` body and its private helpers
  are co-located.
- No dead code, no silent filtering, no dual-audience complexity.

## Future: User-Facing Output

If Claude Code fixes issue #13600 (audience filtering) or #3174 (display
`notifications/message`), user-facing output can be added back. At that point the correct
approach is clear: add a `format_for_user` method to the `Tool` trait (returns `Option<String>`)
and wire it in `call_content` — no opaque `Role` gymnastics, no hidden server-side filters.
