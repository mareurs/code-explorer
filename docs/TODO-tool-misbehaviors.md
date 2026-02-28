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
**Status:** ✅ FIXED — `expected_content` guard added (commit `e03bce7`)

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
**Status:** ⚠️ PARTIALLY FIXED — `trim_symbol_start` skips leading `}` lines, but the fix has a blind spot. Observed recurrence: `replace_symbol` on `impl Tool for EditLines/call` (2026-02-28) still ate the closing `}` of `input_schema`, corrupting the file. The exact trigger is unclear — may be a case where the LSP range starts on the first real line of the target `fn` but the preceding method's `}` is somehow still included. Regression tests: `tests/symbol_lsp.rs::replace_symbol_preserves_preceding_close_brace`. **Workaround: use `edit_lines` instead of `replace_symbol` for method bodies in impl blocks.**

**What happened:**
Called `replace_symbol` on `impl Tool for EditLines/input_schema`. The LSP's symbol range
for `input_schema` apparently included the closing `    }` and blank line of the *preceding*
`description` method. My replacement body started with `fn input_schema...` (without that
`    }` prefix), so the description method lost its closing brace — making it span into
`input_schema` and beyond in the compiler's view.

**Root cause:**
The LSP (rust-analyzer) reports the symbol range for a method as including any leading
whitespace or closing tokens from the prior method that appear before the `fn` keyword.
When `replace_symbol` replaces that range with content that doesn't re-emit those tokens,
they're silently deleted.

**Reproduction hint:**
Any `replace_symbol` on a method that is not the first in an impl block. The preceding
method's closing `}` and the blank line separator are both at risk.

**Fix applied:**
`trim_symbol_start(start, &lines)` scans forward from `sym.start_line` skipping lines
that are empty or contain only `}`, `},`, or `};` — landing on the actual `fn`/`pub`/
keyword. Applied in both `replace_symbol::call` and `insert_code::call` ("before" case).

---

### BUG-004 — `insert_code` inserts inside a function body instead of after it

**Date:** 2026-02-28
**Severity:** High — silently corrupts the file
**Status:** ✅ PARTIALLY FIXED — `trim_symbol_start` applied to the "before" case. Regression tests: `tests/symbol_lsp.rs::insert_code_before_skips_lead_in`. The "after" case (using `end_line`) is not yet protected.

**What happened:**
Called `insert_code(name_path="tests/edit_lines_missing_params_errors", position="after")`.
The insertion was placed *inside* `edit_lines_delete_past_eof_errors` — inside its
`json!({...})` body — rather than after `edit_lines_missing_params_errors`.

**Root cause hypothesis:**
The LSP symbol range for `edit_lines_missing_params_errors` apparently ends at a line that
is *within* a neighboring (likely the next) function. `insert_code` uses the `end_line` of
the symbol to determine the insertion point, but the `end_line` was stale or incorrect.
Could also be a name-path resolution issue — the tool may have matched the wrong function.

**Fix ideas:**
- After `insert_code`, verify the insertion with `search_pattern` to confirm the new code
  is in the expected location.
- Consider using `edit_lines` for insertions when position must be precise.
- For the "after" case: similarly apply a `trim_symbol_end` that scans backwards from
  `end_line` to find the actual closing token, guarding against over-extended LSP ranges.

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
