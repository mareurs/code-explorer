# Tool Misbehaviors — Observed in the Wild

This is a living document. **Update it whenever you observe unexpected, wrong, or dangerous
behavior from codescout's own tools while working on the codebase.** Each entry should
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
**Status:** ✅ FIXED — UTF-16 → byte offset corrected in `apply_text_edits`; post-rename
corruption scan added to detect wrong-column edits from the LSP.

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

> **Resolved by design (2026-03-02):** Symbol range redesign removed all range-manipulation heuristics. We now trust LSP ranges directly. See `docs/plans/2026-03-02-symbol-range-redesign-design.md`.

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

> **Resolved by design (2026-03-02):** Symbol range redesign removed all range-manipulation heuristics. We now trust LSP ranges directly. See `docs/plans/2026-03-02-symbol-range-redesign-design.md`.

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

### BUG-006 — `index_status` / `index_project`: second call fails with shadow-table conflict

**Date:** 2026-03-01
**Severity:** High — `index_status` crashes on every call after the first post-indexing call
**Status:** ✅ FIXED — `BEGIN IMMEDIATE` + re-check in `maybe_migrate_to_vec0`;
`open_db` after `build_index` wrapped in `spawn_blocking`. Regression tests:
`migration_race_loser_exposes_shadow_table_conflict`,
`concurrent_open_db_migrations_do_not_corrupt`.

**What happened:**
First `index_status` call after `index_project` succeeds. Every subsequent call returned:
`"Could not create '_info' shadow table: table 'chunk_embeddings_info' already exists"`.

**Root cause:**
Classic TOCTOU (time-of-check / time-of-use) race in `maybe_migrate_to_vec0`:

1. `build_index` completes; `embedding_dims` is now set in `meta`; plain `chunk_embeddings`
   holds BLOB data.
2. `IndexProject`'s background `tokio::spawn` calls `open_db` **directly on the async
   thread** (no `spawn_blocking`) to read post-index stats — this is connection A.
3. `index_status` is called concurrently; its `spawn_blocking` calls `open_db` — this is
   connection B.
4. Both connections read `sqlite_master` *outside any transaction* and both observe
   `"plain table"`.
5. Connection A enters `BEGIN` (deferred), gets write lock, migrates plain → vec0,
   commits.  Shadow tables `chunk_embeddings_info` etc. are now live.
6. Connection B enters `BEGIN` (deferred), gets write lock.  B's view sees vec0 now.
   B runs `ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1` — SQLite allows
   renaming a virtual table since 3.26.0, **but does NOT rename shadow tables**.
   `chunk_embeddings_info` remains under its original name.
   B then runs `CREATE VIRTUAL TABLE chunk_embeddings USING vec0(...)` — fails with
   `"table 'chunk_embeddings_info' already exists"`.

**Fix applied:**
- `maybe_migrate_to_vec0`: changed `BEGIN` → `BEGIN IMMEDIATE` so only one connection
  can be attempting migration at a time.  Added a re-check inside the exclusive
  transaction: if the table is already vec0, ROLLBACK and return `Ok(())`.
- `IndexProject::call`: wrapped post-build `open_db` stats call in
  `tokio::task::spawn_blocking` so it runs on a dedicated thread and the async runtime
  is not blocked.  Also restructured to gather stats before acquiring the `Mutex` guard
  (a `MutexGuard` is `!Send` and cannot be held across an `.await`).

---

### BUG-007 — `run_command`: pipeline false-positive blocks `git diff src/server.rs | head -80`

**Date:** 2026-03-01
**Severity:** Medium — blocks legitimate git+pipe workflows
**Status:** ✅ FIXED — per-segment pipeline check in `check_source_file_access`.

**What happened:**
`run_command("git diff src/server.rs | head -80")` returned
`"shell access to source files is blocked"` with a hint to use `read_file` instead.
`head` is being used to limit `git diff` output, not to read the `.rs` file directly.

**Root cause:**
`check_source_file_access` applied its two regexes (`SOURCE_ACCESS_COMMANDS` and
`SOURCE_EXTENSIONS`) against the entire command string. `head` matched in segment 2,
`.rs` matched in segment 1 — both satisfied, so blocked. The check had no awareness
of pipeline boundaries.

**Fix applied:**
Split the command on `|` and find the first segment where BOTH regexes match. If no
single segment contains both a blocked command and a source extension, return `None`.
New tests: `source_file_access_allows_git_diff_piped_to_head`,
`source_file_access_blocks_cat_in_same_segment_as_source_file`.

---

### BUG-008 — `list_symbols`: 50-symbol file returns ~13k tokens due to uncounted children

**Date:** 2026-03-01
**Severity:** Medium — fills context window on files with many `impl` blocks
**Status:** ✅ FIXED — flat symbol count cap (`LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP = 150`).

**What happened:**
`list_symbols("src/tools/symbol.rs")` reported "50 symbols" (top-level cap of 100 not
reached) but produced ~13k tokens in the MCP response. Claude Code flagged it as
`⚠ Large MCP response`.

**Root cause:**
`LIST_SYMBOLS_SINGLE_FILE_CAP = 100` counts top-level symbols only. With `depth=1`
(default), each top-level symbol embeds its children in the JSON. A file with 50
`impl` blocks × 4 methods each = 250 flat entries even though only 50 top-level
symbols were reported. No overflow was triggered because 50 < 100.

**Fix applied:**
Added `LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP = 150` and a `flat_symbol_count` helper that
counts top-level + depth-1 children. When flat count exceeds the cap, greedy
top-level truncation produces an overflow with a hint mentioning `depth=0` and
`find_symbol`. The existing top-level cap of 100 remains as a secondary check for
files with many childless symbols.
New tests: `list_symbols_flat_cap_triggers_on_symbol_with_many_children`,
`list_symbols_flat_cap_not_triggered_for_leaf_heavy_symbols`.

---

### BUG-009 — `find_symbol`: LspManager `starting` map not cleaned up on async cancellation

**Date:** 2026-03-01
**Severity:** Low — stale entry self-heals on next call, but can cause spurious re-start attempts
**Status:** ✅ FIXED — `StartingCleanup` RAII guard in `do_start` + `std::sync::Mutex` for `starting`

**What happened:**
`find_symbol("User", kind: "class", path: "src/main/kotlin/edu/planner/domain/models/")` in
backend-kotlin project timed out after 60s. A subsequent call with a specific file path
returned 0 results instead of also timing out.

**Root cause (two distinct issues):**

1. **Primary: `tool_timeout_secs = 60` < Kotlin LSP cold-start time.**
   `server.rs:call_tool` wraps every tool call in `tokio::time::timeout(tool_timeout_secs)`.
   Kotlin LSP (JVM/IntelliJ-based) takes ~90-120s to complete `initialize`. The tool times
   out first. Fixed by raising `tool_timeout_secs = 300` in `backend-kotlin/project.toml`.

2. **Secondary: `starting` map not cleaned up on async cancellation.**
   When the tool timeout fires and drops the `do_start` future, `starting.remove(language)`
   never runs (it was only in the success/failure arms, not on cancellation). The stale
   closed-channel entry stays in `starting`. The next caller sees it, falls through to the
   "starter failed" branch, and unnecessarily attempts a second start.
   NOTE: The child process is NOT a zombie — `Drop for LspClient` already aborts the reader
   task and SIGTERMs the child. The only leaked resource was the stale map entry.

**Fix applied (secondary issue):**
- Changed `starting: tokio::sync::Mutex<...>` → `starting: std::sync::Mutex<...>` (safe
  since the lock is never held across `await` points).
- Added `StartingCleanup` RAII guard in `do_start` that calls `starting.remove()` in its
  `Drop` impl, covering success, failure, and async cancellation paths uniformly.
- Also refactored: config resolution (`servers::default_config`) moved before the barrier in
  `get_or_start`, so unknown languages fail fast without touching `starting` at all.
- Regression tests: `failed_start_cleans_up_starting_map` and
  `cancelled_get_or_start_cleans_up_starting_map` in `src/lsp/manager.rs`.

---

### BUG-010 — `insert_code`: inserts between `#[derive]` attribute and struct definition

**Date:** 2026-03-01
**Severity:** High — produces uncompilable code silently
**Status:** ✅ FIXED — `"before"` branch now calls `scan_backwards_for_docs` after `trim_symbol_start`, walking back past `#[...]` and `///`/`//!` lines before inserting

> **Resolved by design (2026-03-02):** Symbol range redesign removed all range-manipulation heuristics. We now trust LSP ranges directly. See `docs/plans/2026-03-02-symbol-range-redesign-design.md`.

**What happened:**
Called `insert_code(name_path="CodeScoutServer", path="src/server.rs", position="before", code="const USER_OUTPUT_ENABLED: bool = false;\n")`.
Expected the const to land _before_ the doc comment `/// The MCP server handler` that precedes the struct.
Instead, the const was inserted between `#[derive(Clone)]` and `pub struct CodeScoutServer` — splitting the attribute from the item it annotates:

```rust
/// The MCP server handler — holds shared agent state and a registry of tools.
#[derive(Clone)]
const USER_OUTPUT_ENABLED: bool = false;   // ← inserted HERE (wrong)

pub struct CodeScoutServer {
```

This caused two compiler errors:
- `E0774: derive may only be applied to structs, enums and unions`
- `E0277: the trait bound … Clone is not satisfied`

**Reproduction hint:**
Any struct with leading `#[derive(...)]` + `/// doc comment`. Use `insert_code(position="before")` targeting the struct name.
The tool resolves the struct's first line as the `#[derive]` line (or possibly the opening `pub struct` line), then inserts immediately before the `pub struct` declaration — after any attributes on that line range.

**Root cause hypothesis:**
`insert_code` uses the LSP symbol range for the struct, whose `start_line` points to the first attribute (`#[derive]`). The "before" logic then inserts at `start_line`, which is _inside_ the attribute group rather than before the entire annotated item. Specifically, `trim_symbol_start` skips lines that look like closing braces but does not skip `#[...]` attribute lines.

**Fix ideas:**
- In the "before" branch of `InsertCode::call`, walk _backward_ from `start_line` past any contiguous `#[…]` attribute lines and doc-comment lines (`///`, `//!`), then insert before that extended prefix.
- Add a regression test: insert before a struct that has `#[derive]` + `///` and assert the const appears before the `///` line.

---

### BUG-011 — `find_symbol`: returns local variable children when `name_path` is specified

**Date:** 2026-03-02
**Severity:** Medium — significant noise; agent asks for 1 symbol, gets 15+ extra entries
**Status:** ✅ FIXED — `collect_matching` now requires exact `name_path` equality; `Variable`-kind children filtered. Regression test: `find_symbol_name_path_does_not_return_local_variable_children`.

**What happened:**
Called `find_symbol(name_path="impl Tool for FindReferences/call", include_body=true)`.
Expected: 1 result (the `call` method body).
Got: 19 results — the method plus every local variable declaration inside it.

**Root cause hypothesis:**
`collect_matching` matched all symbols whose `name_path` **starts with** the given path, not just the exact match. Local variable declarations in Rust are represented as `Variable` kind child symbols.

---

### BUG-012 — `goto_definition`: identifier column detection uses naive `str::find()`

**Date:** 2026-03-02
**Severity:** Medium — tool near-unusable; usage stats showed 100% error rate
**Status:** ✅ FIXED — unknown identifier now falls back to first-nonwhitespace column instead of erroring. Regression test: `goto_definition_unknown_identifier_falls_back_to_first_nonwhitespace`.

**What happened:**
`goto_definition(path="src/tools/file.rs", line=13, identifier="OutputGuard")` returned
`RecoverableError: "identifier 'OutputGuard' not found on line 13"`. The identifier was not on
that line (my mistake), but the 100% failure rate across all historical calls indicated a
systematic issue with `str::find()` returning `None` for common cases.

---

### BUG-013 — `replace_symbol`: replaces wrong line range when LSP reports incorrect start_line

**Date:** 2026-03-02
**Severity:** High
**Status:** ✅ FIXED — `is_declaration_line` guard rejects start lines that don't contain a Rust item keyword; returns `RecoverableError` before touching the file.

> **Resolved by design (2026-03-02):** Symbol range redesign removed all range-manipulation heuristics. We now trust LSP ranges directly. See `docs/plans/2026-03-02-symbol-range-redesign-design.md`.

**What happened:**
Called `replace_symbol(name_path="format_get_usage_stats", path="src/tools/user_format.rs")`.
The tool reported `"replaced_lines":"1206-1259"` but the actual function declaration was at line 1164.
The LSP resolved the symbol to an inner `let p50` binding at line 1206 rather than the function.
Result: duplicate function stub, deleted ANSI constants + helper functions, 29 compile errors.

**Root cause hypothesis:**
LSP sometimes resolves a `name_path` to an inner local variable binding rather than the function declaration, producing a `start_line` that points inside the body. The `trim_symbol_start` function skips `}` lines but does not validate that the resolved line contains a Rust item keyword.

---

### BUG-014 — `remove_symbol`: over-extends range into sibling constants

**Date:** 2026-03-02
**Severity:** High — silently deletes code that follows the target symbol
**Status:** ✅ FIXED — `clamp_end_to_closing_brace` walks backward from the LSP end until a `}` line is found

> **Resolved by design (2026-03-02):** Symbol range redesign removed all range-manipulation heuristics. We now trust LSP ranges directly. See `docs/plans/2026-03-02-symbol-range-redesign-design.md`.

**What happened:**
`remove_symbol` on a function that is immediately followed by `const` declarations deleted not only
the function but also the constants. The LSP `end_line` extended past the function's closing `}` into
the sibling items.

**Reproduction hint:**
Remove a function immediately followed by `const FOO: ... = ...;` declarations. Observe the constants
are deleted along with the function.

**Root cause hypothesis:**
`trim_symbol_end` walks backward past blank lines and lines ending with `{`, but LSP may report an
`end_line` that extends to include sibling constants if there is no blank line separator. The removal
uses the un-trimmed end, consuming more lines than intended.

**Fix ideas:**
- `remove_symbol` should use `trim_symbol_end` (already exists) to trim the end range before deleting.
- Add a guard: if the line after the trimmed end doesn't look like a closing brace or blank, emit a
  warning or RecoverableError.
- Regression test: remove a function preceding a sibling `const`; assert the const survives.

---

### BUG-015 — `edit_file`: returns `"ok"` but silently does not write the file

**Date:** 2026-03-02
**Severity:** High — data loss; agent believes changes were applied when they were not
**Status:** ✅ FIXED — `route_tool_error` now includes `"ok": false` in every `RecoverableError` body

**What happened:**
Multiple `edit_file` calls on `.rs` and `.md` files returned `"ok"` with no error, but the changes
were not present on disk when subsequently read back. Confirmed for at least:
- `tests/symbol_lsp.rs`: BUG-010 test insertion returned `"ok"`, but `search_pattern` immediately
  after confirmed the test was not in the file.
- `docs/TODO-tool-misbehaviors.md`: BUG-011–BUG-014 entries and status updates from the previous
  session returned `"ok"` but were absent from the file at session start.

**Reproduction hint:**
Call `edit_file(path="tests/symbol_lsp.rs", old_string="// ── BUG-004: ...", new_string=<large block>)`.
Immediately call `search_pattern` on a unique string from the new content. Content may be absent.

**Root cause hypothesis:**
Unknown. Possibly:
1. A codescout routing plugin hook intercepts the write and drops it silently.
2. An internal check (multi-line source guard?) rejects the write but returns `"ok"` instead of an error.
3. A file lock or concurrent write causes the edit to be lost.

**Fix ideas:**
- After every `edit_file`, verify with `search_pattern` that the unique new content is present.
- `edit_file` should return an error (not `"ok"`) if the write fails or is blocked.
- Investigate the routing plugin's `PreToolUse` hook for `edit_file`.

---

### BUG-016 — `remove_symbol`: freezes and corrupts file when targeting a `const` item

**Date:** 2026-03-02
**Severity:** High — tool hangs (never returns), partial corrupt write left on disk
**Status:** ✅ FIXED — `clamp_end_to_closing_brace` now takes a `floor` parameter (= `trimmed_start`) so it never walks past the symbol's own range. Defense-in-depth guard rejects `end <= start` before writing. Regression test: `remove_symbol_handles_const_without_closing_brace`.

**What happened:**
Called `remove_symbol(name_path="USER_OUTPUT_ENABLED", path="src/server.rs")` to delete a
module-level `const` (with its preceding 3-line doc comment):

```rust
/// When false, user-audience content blocks are stripped before sending to the
/// MCP client. Flip to `true` once Claude Code implements proper audience
/// filtering (i.e. LLM context no longer receives Role::User-only blocks).
const USER_OUTPUT_ENABLED: bool = false;
```

Expected: the 4 lines above deleted, surrounding code intact.
Actual:
1. The tool **froze** — it never returned and had to be interrupted by the user.
2. A **duplicate `use` import** appeared in the file:
   ```diff
   +use crate::usage::UsageRecorder;
    use crate::usage::UsageRecorder;
   ```
3. The constant was **not removed**.

**Reproduction hint:**
```
// src/server.rs (around line 37-43 before this session's edits):
use crate::usage::UsageRecorder;   // line 37

/// When false, ...                // line 39
/// ...                            // line 40
/// ...                            // line 41
const USER_OUTPUT_ENABLED: bool = false;  // line 42
```
Call: `remove_symbol(name_path="USER_OUTPUT_ENABLED", path="src/server.rs")`
Observe: tool hangs, `git diff` shows duplicate import on line 37, constant untouched.

**Root cause hypothesis:**
`rust-analyzer` may not surface module-level `const` declarations as top-level
`DocumentSymbol` entries (or surfaces them at a different location). `remove_symbol`
may have resolved the name to the wrong range (the import on line 37 instead of the
const on line 42), then performed a corrupt "delete-and-rewrite" of that line, then
blocked waiting for an LSP notification or response that never came.

**Fix ideas:**
- Before deleting, validate that the resolved `start_line` contains a Rust item keyword
  (`const`, `fn`, `struct`, `impl`, etc.) — reject with `RecoverableError` if not (same
  guard added for BUG-013).
- Confirm whether `rust-analyzer` exposes `const` items as `DocumentSymbol`; if not,
  fall back to `search_pattern` text-based location for `const`/`static` targets.
- Add a timeout on the LSP call inside `remove_symbol` so it fails fast rather than
  hanging the MCP client.
- Regression test: create a temp `.rs` file with a `const FOO: bool = false;`, call
  `remove_symbol("FOO", ...)`, assert the line is gone and siblings are intact.

---

## Template for new entries

```
### BUG-017 — `git_blame`: fails with "path does not exist in given tree" when project root is a git subdirectory

**Date:** 2026-03-02
**Severity:** Medium — tool unusable when active project ≠ git root
**Status:** ✅ FIXED — `blame_file` now computes the repo-relative path by prepending `strip_prefix(workdir, repo_path)` before calling `repo.blame_file()` and `committed_content()`. Regression test: `blame_works_when_project_root_is_git_subdirectory`.

**What happened:**
Activated `tests/fixtures/kotlin-library` as the project root (a subdirectory inside the
codescout git repo). Called `git_blame` on `src/main/kotlin/library/models/Book.kt`.
Got error: `the path 'main' does not exist in the given tree; class=Tree (14); code=NotFound (-3)`.

The tool correctly discovers the parent `.git` at the codescout root, but then tries to
resolve the file path (`src/main/kotlin/...`) relative to the git root instead of relative to
the active project root — so the git tree lookup fails.

Switching active project to the actual git root (`/home/marius/work/claude/codescout`) and
calling with the full path (`tests/fixtures/kotlin-library/src/main/kotlin/library/models/Book.kt`)
works correctly.

**Reproduction hint:**
```
activate_project("/some/git-repo/subdir")
git_blame("path/to/file.kt")  # fails
```

**Root cause hypothesis:**
`git_blame` opens the git repo by traversing up from the project root (correct), then builds
the path to look up in the git tree using the project-root-relative path instead of the
repo-root-relative path.

**Fix ideas:**
Strip the project root from the file path, then prepend the git-repo-relative prefix of the
project root to get the correct repo-relative path for the tree lookup.

---

### BUG-016 — `insert_code`: inserts after symbol's opening line, not after its closing brace

**Date:** 2026-03-02
**Severity:** High
**Status:** ✅ FIXED — `validate_symbol_range` now catches `ast_end > sym.end_line` (BUG-018
fix). When LSP reports a truncated `end_line` inside the function body, tree-sitter detects the
discrepancy and `insert_code` returns `RecoverableError` instead of corrupting the file.
Regression test: `insert_code_after_rejects_truncated_end_in_nested_fn` in `tests/symbol_lsp.rs`.

**What happened:**
Called `insert_code(name_path="tests/other_tools_do_not_skip_server_timeout", position="after", ...)`.
Expected the new code to appear after the function's closing `}`. Instead it was injected after
the third *body line* of the function (the `name` variable reference inside the for loop), producing
syntactically invalid Rust that broke the entire file.

**Reproduction hint:**
```
insert_code(
    path="src/server.rs",
    name_path="tests/other_tools_do_not_skip_server_timeout",
    position="after",
    code="... new test ..."
)
# → code lands mid-function body, not after closing brace
```

**Root cause:** `insert_code` used `sym.end_line + 1` as the insertion point, trusting the LSP.
For nested functions inside `mod tests`, rust-analyzer sometimes reports a truncated `end_line`
(a line inside the function body rather than the closing `}`). The BUG-018 fix to
`validate_symbol_range` extended the check from `start == end` to `ast_end > sym.end_line`,
which now catches this case. The insertion fails loudly rather than silently corrupting the file.

---

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

### BUG-021 — `edit_file`: parallel calls cause partial state + MCP server "crash"

**Date:** 2026-03-03
**Severity:** High — leaves files in inconsistent partial state; server exit requires `/mcp` restart
**Status:** 🔍 ROOT CAUSE IDENTIFIED (2026-03-03) — two independent issues, one fixable

**What happened:**
Dispatched two `edit_file` calls in the same parallel response (targeting two different source
files: `src/embed/local.rs` and `src/config/project.rs`). The Claude Code permission system
handles each call independently: the first call was approved and returned `"ok"` (edit applied
to `local.rs`); the second call was rejected by the user and returned an error. This left the
two files in an inconsistent state — one edited, one not. Immediately after, the codescout
MCP server crashed and became unavailable, requiring a manual `/mcp` reconnect.

**Reproduction hint:**
1. Dispatch two `edit_file` tool calls in a single parallel response to different source files.
2. Approve the first permission prompt, reject (or let timeout) the second.
3. Observe: first file edited, second file unchanged — inconsistent partial state.
4. codescout MCP server crashes; subsequent tool calls fail until `/mcp` restart.

**Root cause (investigated 2026-03-03 — two separate issues):**

**Issue A — Partial state: inherent to independent parallel writes.**
When two `edit_file` calls target different files, they run as independent `tokio::spawn` tasks
inside rmcp's `serve_inner`. There is no transaction semantics across them. If one is denied
(permission dialog) while the other succeeds, the files are left in a partially-applied state.
This is NOT a bug in our code — it's the correct behavior for two independent operations. The
fix is operational: never dispatch parallel write tool calls.

**Issue B — "Crash" is actually Claude Code closing the stdio pipe (rmcp cancellation race).**
Static analysis of the full code path confirms there are NO panic paths in our production code
that could crash the server:
- All `lock().unwrap()` calls in the hot path (`open_files`, `OutputBuffer`) have trivial
  critical sections (HashSet ops only) — mutex cannot be poisoned by normal use.
- `call_tool_inner` routes ALL errors through `route_tool_error`; no unhandled panics.
- rmcp 0.1.5 spawns each request as `tokio::spawn` with the JoinHandle **dropped** — task
  panics are absorbed by the detached task and never propagate to the serve loop.
- The serve loop in `serve_inner` has no `unwrap()`/`expect()` in its event handler.

The "crash" is the server process exiting cleanly after the **stdio pipe closes**. This maps to
`service.waiting()` returning `QuitReason::Closed` → error propagates via `?` in `run()`.

**Why does Claude Code close the pipe?** Most likely a cancellation race in rmcp 0.1.5:
When Claude Code denies a parallel call, it may send a `notifications/cancelled` for the
in-flight request. rmcp cancels the `CancellationToken` but the spawned task has **no check**
for `context.ct.is_cancelled()` — it runs to completion and sends a response back through
`sink_proxy_tx`. The main loop then writes that response to stdout. Claude Code receives an
unexpected response for an already-cancelled request ID, which may cause it to close the
connection (a Claude Code MCP client bug, not ours).

**Fix:**
- **Operational** (immediate): never dispatch parallel write tool calls. Always finish one
  `edit_file` / `replace_symbol` / `insert_code` / `create_file` before starting the next.
- **rmcp limitation**: rmcp 0.1.5 does not suppress responses for cancelled requests.
  This cannot be fixed in our code without forking rmcp. Upgrading rmcp if a newer version
  respects cancellation tokens in the task-spawn path would help.
- **Defence-in-depth** (optional): add a `[profile.release] panic = "abort"` to Cargo.toml so
  any future panic terminates the process immediately rather than leaving a half-alive server
  (currently no panics exist, but this prevents silent corruption if one is introduced later).

---

### BUG-020 — `edit_file` + `run_command`: cross-tool `@ack_*` handle confusion

**Date:** 2026-03-03
**Severity:** Medium — caused a silent failure loop; the original edit goal was blocked

**What happened:**
`edit_file` on a multi-line `use {…}` import list was blocked (source-file guard) and returned a
`pending_ack` handle `@ack_b32f095e`. The LLM then called `run_command("@ack_b32f095e")`.
`run_command` has its own `@ack_*` early-dispatch that calls `get_dangerous()` — the wrong store.
Result: "ack handle expired or unknown" with hint "Re-run the original command" — both misleading.

**Root causes (3 compounding):**
1. `run_command`'s ack dispatch didn't distinguish command acks from edit acks → misleading error.
2. `infer_edit_hint` returned `insert_code` for the import list (length heuristic, no def-keywords);
   `insert_code` requires a `name_path` which doesn't exist for import identifier lists → wrong advice.
3. The blocking hint only showed `edit_file("@ack_xxx")`, not `acknowledge_risk: true` → LLM
   confused the ack with `run_command`'s own ack protocol.

**Fix (2026-03-03, all tests pass):**
- `workflow.rs`: Cross-tool guard before `get_dangerous()` — if handle is in `pending_edits`,
  return targeted error naming `edit_file` and `acknowledge_risk: true`.
- `file.rs/infer_edit_hint`: Comma + no-`(`/`=`/`->` heuristic → detects import fragments,
  suggests `acknowledge_risk: true` instead of `insert_code`.
- `file.rs` hint template: now leads with `acknowledge_risk: true`, ack handle is secondary.

**Tests added:** `run_command_rejects_edit_file_ack_handle_with_clear_error`,
`infer_edit_hint_import_list_suggests_acknowledge_risk`,
`infer_edit_hint_insert_code_still_fires_for_real_code_insertions`.

---

### BUG-019 — `replace_symbol`: lands in wrong symbol when LSP has stale line numbers

**Date:** 2026-03-02
**Severity:** High — silently corrupts the file; no error returned, `status: "ok"` reported
**Status:** ✅ FIXED — `is_valid_symbol_start_line` guard added to `replace_symbol::call`; rejects start lines that look like function-body content (`let`, expressions) rather than symbol declarations, closing delimiters, comments, or attributes. Returns `RecoverableError` before touching the file. Regression test: `replace_symbol_rejects_stale_lsp_start_line` in `tests/symbol_lsp.rs`.

**What happened:**
Called `replace_symbol(name_path="impl Tool for EditFile/input_schema", path="src/tools/file.rs", new_body=<updated schema>)`.
The tool reported `replaced_lines: "639-665"` and `status: "ok"`, but those lines were
**inside `format_read_file_summary`**, not inside `impl Tool for EditFile`. The result:
- My new `input_schema` body was injected mid-function into `format_read_file_summary`
- `format_read_file_summary`'s own body at L639-665 was overwritten and lost
- The file gained a spurious nested `fn input_schema` function and failed to compile:
  `error: mismatched closing delimiter: `)' at src/tools/file.rs:631`

The correct `impl Tool for EditFile/input_schema` was later found at L988, well past L665.

**Reproduction:**
1. Start with a clean file where symbol X is at line N.
2. Insert new functions/content ABOVE symbol X (shifting it to line N+K) without sending
   `did_change` to the LSP (e.g., via `create_file` on a different path, or if the LSP
   didn't process a prior `notify_file_changed`).
3. Call `replace_symbol(name_path="X", ...)`.
4. LSP returns stale location N (not N+K); `replace_symbol` replaces lines N..N+M
   which now contain different content — silently corrupting that content.

**Root cause:**
`replace_symbol` reads the symbol location from LSP, which has a **stale view** of the file.
The file had ~300 lines of new format-helper functions inserted above `impl Tool for EditFile`,
but the LSP was never notified (no `did_change`). So LSP returned `start_line = 639` —
which was correct for the old file but now falls inside `format_read_file_summary`.
`replace_symbol` then splices `[639..665]` unconditionally, destroying the content there.

**Missing guard:**
No validation that the content at `start_line` actually contains the target symbol name
or a Rust item keyword. If it did, this would have caught the mismatch:
- Expected: `fn input_schema` at L639
- Actual: `let mut out = format!(...)` at L639 → stale, reject with RecoverableError.

**Fix:**
After resolving `(start_line, end_line)` from LSP, read line `start_line` from the
current file and verify it contains `fn`, `pub`, `async`, `struct`, `impl`, `trait`,
`enum`, `type`, `const`, or `static`. If none of those are found, return
`RecoverableError("symbol location appears stale — line {start_line} does not start a Rust item; re-run after LSP syncs")`.
This is the same class of guard added for BUG-013 (`is_declaration_line`), and it would
catch the stale-LSP class of errors.

---

### BUG-022 — `replace_symbol`: called twice on same symbol stacks orphaned doc comments

**Date:** 2026-03-04
**Severity:** Medium — compiles with warnings/errors; requires manual cleanup of orphaned text
**Status:** Open

**What happened:**
Called `replace_symbol("truncate_compact", …)` twice in succession to update the function.
Each call reported success and a replaced-lines range. The second call replaced only the
LSP-reported body range (which excluded the doc comment), leaving the first replacement's
doc comment block orphaned in the file above the second replacement's doc comment. Result:
three stacked `///` blocks where only one was intended. Clippy then flagged
`doc_lazy_continuation` on two of them.

**Probable cause:**
LSP `documentSymbol` range for a free function covers only the `fn` keyword through closing
`}`, not the preceding `///` doc comments. So `replace_symbol` inserts a full new body
(including doc) at the function range start, but leaves the old doc comment untouched just
above it. On the second call the same thing happens again.

**Workaround:**
Call `replace_symbol` only once per symbol per session, or use `edit_file` with a
sufficiently-anchored `old_string` that includes the doc comment text to be replaced.
If duplication occurs, use `edit_file` with the full duplicated block as `old_string`.

### BUG-018 — `replace_symbol`: duplicates body instead of replacing, leaves stray tokens

**Date:** 2026-03-02
**Severity:** High — silently corrupts the file; cargo still compiles past the first error
**Status:** ✅ FIXED — `validate_symbol_range` now checks `ast_end > sym.end_line` (previously only caught the degenerate `start == end` case; truncated off-by-one slipped through). Regression tests: `validate_symbol_range_rejects_truncated_end_line` (unit) and `replace_symbol_rejects_truncated_end_line` (symbol_lsp).

**What happened:**
Used `replace_symbol` to update a function body in `src/server.rs` (the integration test
`call_tool_strips_project_root_from_output`). Instead of replacing the old body, the tool
inserted the new body *inside* the old one, producing a doubly-nested function. It also
left a stray `}` and a duplicate `#[tokio::test]` attribute from the old function boundary.
The file did not compile. Required manual repair via `run_command("sed ...")` to remove the
duplicate section.

**Reproduction:**
Call `replace_symbol` on a `#[tokio::test] async fn` inside a `#[cfg(test)] mod tests`
block. The symbol range likely includes only the function signature line, not the full body,
causing the replacement to be inserted mid-function rather than replacing it.

**Probable cause:**
Same root cause as BUG-003 and BUG-013 — LSP symbol range for functions inside test modules
may have an off-by-one on the end line, causing the replacement to target a smaller range
than the actual function body.

**Workaround:**
Use `edit_file(old_string=<full function text>, new_string=<replacement>)` for test
functions. `replace_symbol` is unreliable for functions inside `mod tests` blocks.

### BUG-024 — `read_file`: panics on files with Unicode box-drawing chars, crashing the MCP server

**Date:** 2026-03-05
**Severity:** High — deterministic crash; any file >5 KB containing multi-byte UTF-8 characters kills the server
**Status:** ✅ FIXED — `floor_char_boundary` helper added to `truncate_compact`; regression tests: `truncate_compact_unicode_does_not_panic`, `floor_char_boundary_lands_on_boundary`

**What happened:**
`read_file("docs/ARCHITECTURE.md")` returned `MCP error -32000: Connection closed` on every call.
`list_symbols` and other tools worked fine. `read_file("Cargo.toml")` (ASCII-only) worked fine.

**Reproduction hint:**
Any file > `TOOL_OUTPUT_BUFFER_THRESHOLD` (5 KB) containing multi-byte UTF-8 characters
(box-drawing chars `─│┌`, CJK, emoji, etc.) will crash `read_file`. `docs/ARCHITECTURE.md` (11 KB)
has box-drawing chars (3 bytes each in UTF-8) in its ASCII diagram.

**Root cause:**
Call chain: `call_content` → JSON > 5 KB → `format_compact` → `format_read_file` (line-numbered
output with Unicode chars) → `truncate_compact(text, soft_max=2000, hard_max=3000)`.

`truncate_compact` had two unsafe byte slices:
1. `text[..search_end]` where `search_end = hard_max` — used for `rfind('\n')`
2. `text[..end]` where `end = hard_max` — the hard-truncate fallback

Both slice at a raw byte offset. If that offset falls inside a multi-byte UTF-8 character,
Rust panics with `byte index N is not a char boundary`. With `panic = "abort"` in
`[profile.release]` (added as BUG-021 defence-in-depth), this aborts the process immediately
instead of being silently absorbed by the detached tokio task.

**Fix applied:**
Added `floor_char_boundary(s, n)` which walks backward from `n` to the nearest valid char
boundary. `truncate_compact` now uses it for both slice points.

---

### BUG-023 — `run_command` (subagent): `git diff` without `--no-pager` hangs for 30s

**Date:** 2026-03-04
**Severity:** Medium — wastes 30s agent time before timeout, no data corruption
**Status:** Open

**What happened:**
Subagent code reviewer ran `git diff 368ffbe..d599093 -- src/tools/workflow.rs` inside
`run_command`. Git invoked the `less` pager even without a TTY, waiting for keyboard input
that never arrives. The command timed out after 30s.

**Reproduction:**
`run_command("git diff HEAD~1")` — any `git diff` call without `--no-pager` in environments
where `core.pager` is configured or git defaults to `less`.

**Probable cause:**
Git reads pager config from `~/.gitconfig` and falls back to `less` unless `GIT_PAGER=cat`
or `--no-pager` is specified. Subprocess has no TTY but git doesn't check for one before
invoking the pager.

**Workaround:**
Always use `git --no-pager diff` or set `GIT_PAGER=cat` prefix: `GIT_PAGER=cat git diff ...`.
