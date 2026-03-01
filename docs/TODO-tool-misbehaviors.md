# Tool Misbehaviors — Observed in the Wild

This is a living document. **Update it whenever you observe unexpected, wrong, or dangerous
behavior from code-explorer's own tools while working on the codebase.** Each entry should
capture: what you did, what you expected, what actually happened, and a reproduction hint.

---

## Prompt for future sessions

> Before starting any task on this codebase, re-read this file. While you work, watch for
> unexpected tool behavior: wrong edits, corrupt output, silent failures, misleading errors.
> When you find something, **add an entry here before continuing** — even a one-liner is
> enough to capture it while it's fresh. The goal is to build a corpus of real failure cases
> to drive test and UX improvements.

---

## Observed Bugs

### BUG-001 — `edit_lines` blind editing causes wrong-line mistakes

**Date:** 2026-02-28
**Severity:** High — silently corrupts the file
**Status:** ✅ SUPERSEDED — `edit_lines` removed; replaced by `edit_file` (old_string/new_string)

**What happened:**
Wanted to replace `project_explicitly_activated: false,` (line 56) with a variable binding.
Used `edit_lines(start_line=55, delete_count=1, ...)` but line 55 was `active_project,` —
the line above the intended target. The tool replaced the wrong line without any warning,
producing a duplicate `Ok(Self {` block and two compiler errors.

**Root cause:**
`edit_lines` has no way to confirm what's at the target line before applying the edit.
There is no `old_content` parameter (unlike the builtin `Edit` tool's `old_string`), so
a one-off line count error causes silent corruption.

**Fix applied:**
Added optional `expected_content: String` guard — if line N doesn't match, returns a
`RecoverableError` instead of applying the edit.

---

### BUG-002 — `rename_symbol` LSP rename corrupts unrelated code

**Date:** 2026-02-28
**Severity:** High — produces unparseable source
**Status:** Open

**What happened:**
Renamed test function `project_not_explicitly_activated_on_startup` →
`project_not_explicitly_activated_without_project`. The tool reported success with
`textual_match_count: 0` (meaning the textual sweep found nothing extra), but line ~387
in `agent.rs` was corrupted to:

```
asserproject_not_explicitly_activated_without_project("From file\n"));
```

The LSP rename itself (not the textual sweep) made a bad substitution inside unrelated code,
producing a file that fails to compile with "mismatched closing delimiter" errors.

**Reproduction hint:**
The corrupted line was inside the `project_status_file_takes_precedence_over_toml` test
(lines ~369–384 before the rename). The original line was likely an `assert_eq!` or
similar that the LSP matched as a reference to the renamed symbol — possibly because
rust-analyzer's rename heuristic matched a substring of the function name within a
string literal or doc comment.

**Root cause hypothesis:**
rust-analyzer's rename may have matched a string literal or comment containing the old
function name as a substring. Or the `rename_symbol` tool's textual sweep regex is too
broad and matched a partial occurrence the tool incorrectly reported as 0.

**Fix ideas:**
- After any `rename_symbol` call, immediately run `cargo build` or at least
  `search_pattern` to verify the file is still valid.
- Add a post-rename compilation check in the tool itself (or in the server instructions).
- Investigate whether rust-analyzer's rename is at fault or whether the textual sweep
  regex needs word-boundary anchors (`\b`).
- Consider showing a diff preview before applying rename in destructive mode.

---

### BUG-003 — `replace_symbol` eats closing `}` of preceding method

**Date:** 2026-02-28
**Severity:** High — silently corrupts the file
**Status:** ✅ FIXED — Two root causes identified and resolved. Regression tests:
`tests/symbol_lsp.rs::replace_symbol_preserves_preceding_close_brace`,
`tests/symbol_lsp.rs::replace_symbol_preserves_paren_close_brace`.

**What happened:**
Called `replace_symbol` on `impl Tool for EditLines/input_schema`. The LSP's symbol range
for `input_schema` apparently included the closing `    }` and blank line of the *preceding*
`description` method. My replacement body started with `fn input_schema...` (without that
`    }` prefix), so the description method lost its closing brace — making it span into
`input_schema` and beyond in the compiler's view.

**Root cause (two components):**
1. `trim_symbol_start` originally only skipped exact `}`, `},`, `};` strings but not
   variants like `})` (closing a `json!({...})` macro) or `} // comment`. If the LSP
   placed `start_line` at such a line, the preceding method's closing tokens were deleted.
   **Fixed:** changed check to `t.starts_with('}')` — catches all closing-brace variants.
2. Stale LSP cache: after a first `replace_symbol` write, the LSP wasn't notified of the
   change, so a second call on the same file used stale line numbers, causing wrong splices.
   **Fixed:** `ctx.lsp.notify_file_changed(&full_path)` called after every `write_lines`
   (via `LspManager::notify_file_changed` → `did_change` on each active client).

**Reproduction hint:**
The `})` blind spot: a preceding method that ends with `json!({...})` — the `})` line
caused `trim_symbol_start` to stop rather than skip. The stale-cache case: two consecutive
`replace_symbol` calls on the same file without a `notify_file_changed` in between.

**Fix applied:**
`trim_symbol_start` now uses `t.starts_with('}')` to skip any closing-brace variant.
Applied in both `replace_symbol::call` and `insert_code::call` ("before" case).
`notify_file_changed` notifies all active LSP clients after every `write_lines`.

---

### BUG-004 — `insert_code` inserts inside a function body instead of after it

**Date:** 2026-02-28
**Severity:** High — silently corrupts the file
**Status:** ✅ FIXED — `trim_symbol_start` for "before"; `trim_symbol_end` for "after".
Regression tests: `tests/symbol_lsp.rs::insert_code_before_skips_lead_in`,
`tests/symbol_lsp.rs::insert_code_after_skips_trail_in`.

**What happened:**
Called `insert_code(name_path="tests/edit_lines_missing_params_errors", position="after")`.
The insertion was placed *inside* `edit_lines_delete_past_eof_errors` — inside its
`json!({...})` body — rather than after `edit_lines_missing_params_errors`.

**Root cause:**
LSP over-extends a symbol's `end_line` to include the opening line of the following symbol
(`fn following() {`). `insert_code` used `end_line + 1` directly, landing inside the
following function's body.

**Fix applied:**
Added `trim_symbol_end` (symmetric to `trim_symbol_start`) that walks backward from
`end_line` past lines ending with `{` (next symbol's opening) and blank lines, stopping at
the current symbol's own closing `}`. Applied in the "after" branch of `InsertCode::call`.

---

### BUG-005 — `read_file`: directory path returns hard error instead of RecoverableError

**Date:** 2026-03-01
**Severity:** Medium — aborts parallel tool calls in Claude Code
**Status:** ✅ FIXED

**What happened:**
Called `read_file(path: "src/config")` where `src/config` is a directory. Got:
`Error: failed to read …/src/config: Is a directory (os error 21)` — a hard `anyhow`
error. Claude Code treats `isError: true` responses as fatal, aborting sibling parallel
calls. Should have been a `RecoverableError` with a hint to use `list_dir` instead.

**Root cause:**
The `map_err` on `std::fs::read_to_string` only converts `InvalidData` (binary file) to
`RecoverableError`; all other IO errors fell through to `anyhow::anyhow!()`. No pre-check
for `is_dir()` or `NotFound` was in place.

**Fix applied:**
Added `is_dir()` guard before `read_to_string`. Also converted `NotFound` to
`RecoverableError` in the `map_err` closure.

---

## Template for new entries

```
### BUG-XXX — <tool name>: <one-line description>

**Date:** YYYY-MM-DD
**Severity:** Low / Medium / High
**Status:** Open

**What happened:**
<what you did, what you expected, what happened instead>

**Reproduction hint:**
<minimal steps or context to reproduce>

**Root cause hypothesis:**
<your best guess at why it happened>

**Fix ideas:**
<options for fixing it in the tool or in its UX>

---
```
