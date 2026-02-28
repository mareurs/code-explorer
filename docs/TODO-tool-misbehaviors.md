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

**What happened:**
Wanted to replace `project_explicitly_activated: false,` (line 56) with a variable binding.
Used `edit_lines(start_line=55, delete_count=1, ...)` but line 55 was `active_project,` —
the line above the intended target. The tool replaced the wrong line without any warning,
producing a duplicate `Ok(Self {` block and two compiler errors.

**Root cause:**
`edit_lines` has no way to confirm what's at the target line before applying the edit.
There is no `old_content` parameter (unlike the builtin `Edit` tool's `old_string`), so
a one-off line count error causes silent corruption.

**Workaround:**
Always run `search_pattern` to verify the exact content and line number of the target line
before calling `edit_lines`. Prefer `replace_symbol` for code whenever possible — it
addresses by name, not line number.

**Fix ideas:**
- Add an optional `expected_content: String` guard — if line N doesn't match, return a
  `RecoverableError` instead of applying the edit.
- Return `old_content` in the success response so the caller can detect the mistake post-hoc.
- Prefer `replace_symbol` for all code edits; reserve `edit_lines` for non-code files only.

---

### BUG-002 — `rename_symbol` LSP rename corrupts unrelated code

**Date:** 2026-02-28
**Severity:** High — produces unparseable source

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

## Template for new entries

```
### BUG-XXX — <tool name>: <one-line description>

**Date:** YYYY-MM-DD
**Severity:** Low / Medium / High

**What happened:**
<what you did, what you expected, what happened instead>

**Reproduction hint:**
<minimal steps or context to reproduce>

**Root cause hypothesis:**
<your best guess at why it happened>

**Fix ideas:**
<options for fixing it in the tool or in its UX>
```
