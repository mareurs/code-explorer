# Code Review — LSP & Symbol Reading/Editing

**Date:** 2026-03-03  
**Scope:** `src/lsp/client.rs`, `src/lsp/ops.rs`, `src/tools/symbol.rs`

---

## Strengths

- **Atomic writes** — `write_lines` uses a `.tmp`-rename pattern; crash-safe.
- **`did_open` dedup** — `open_files` set with canonical paths prevents duplicate notifications (correct LSP spec handling).
- **UTF-16 → byte conversion** — `utf16_to_byte_offset` correctly handles non-ASCII in `apply_text_edits`.
- **Kill-on-drop** — `LspClient::drop` sends SIGTERM via `libc::kill` as a safety net even if graceful shutdown was skipped.
- **Post-rename text sweep** — `RenameSymbol` does a word-boundary regex sweep of files LSP didn't touch, which catches string literals and doc comments.
- **BUG-019 guard** — The stale-LSP line check + `validate_symbol_range` cross-checking AST is a solid defense.

---

## Issues

### Important — `insert_code` missing `guard_worktree_write`

**File:** `src/tools/symbol.rs` — `InsertCode::call`

`replace_symbol`, `remove_symbol`, and `rename_symbol` all call `super::guard_worktree_write(ctx).await?` as their first line. `insert_code` does not. In a multi-worktree environment, `insert_code` can silently write to the wrong project.

**Fix:** Add `super::guard_worktree_write(ctx).await?;` as the first line of `InsertCode::call`.

---

### Important — `is_valid_symbol_start_line` is Rust-only, breaks `replace_symbol` on other languages

**File:** `src/tools/symbol.rs` — `is_valid_symbol_start_line` (~line 296)

The keyword allowlist is entirely Rust-specific:

```rust
let item_starts = [
    "fn ", "pub ", "pub(", "async ", "struct ", "impl ", "trait ", "enum ", ...
];
```

For any non-Rust language, valid symbol start lines will be rejected as "stale":
- Python: `def foo():` → blocked (`def ` not in list)
- Python: `class Foo:` → blocked
- TypeScript: `function foo()` → blocked
- TypeScript: `class Foo` → blocked
- TypeScript: `interface Foo` → blocked
- Go: `func Foo()` → blocked
- Java: `public void foo()` → blocked

The error message "symbol location appears stale" is actively misleading when the LSP is working correctly. This makes `replace_symbol` broken for most non-Rust files.

**Fix options (pick one):**
1. Expand the list to be language-aware (pass `language_id` through to `is_valid_symbol_start_line`)
2. Make the check opt-in for Rust only
3. Remove the check entirely and rely solely on `validate_symbol_range` (which already uses AST cross-checking as the canonical defense)

---

### Minor — `did_change` sends hardcoded `version: 1`

**File:** `src/lsp/client.rs` — `LspClient::did_change` (~line 786)

```rust
text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version: 1 },
```

The LSP spec requires versions to be monotonically increasing per-file. Sending `1` every time is technically incorrect. Most servers tolerate it, but strict servers (some JVM-based ones) may log warnings or behave incorrectly on quick successive changes.

**Fix:** Add a per-file version counter (e.g. `AtomicI64` in `LspClient` keyed by path) and increment it on each `did_change` and `did_open`.

---

### Minor — `find_symbol_by_name_path` silently returns first match on ambiguous names

**File:** `src/tools/symbol.rs` — `find_symbol_by_name_path` (~line 2292)

Searching with a bare `"call"` or `"new"` returns the first tree match with no warning. Every `Tool` impl has an `async fn call`, so the wrong one will be silently selected on an ambiguous name.

**Fix:** When multiple symbols match the query, return a `RecoverableError` listing all full name paths so the caller can supply a more specific one.

---

### Minor — `insert_code` blank-line insertion is asymmetric

**File:** `src/tools/symbol.rs` — `InsertCode::call` (~line 1496)

- `position="before"`: always appends a blank line after inserted code.
- `position="after"`: only inserts a blank if the next line is non-empty.

This asymmetry is undocumented and will surprise users inserting before the last symbol in a file or before a closing `}`.

**Fix:** Document the behavior explicitly, or make both modes use the same non-empty-next-line check.

---

## Assessment

**Ready to proceed with fixes?** Yes.

**Priority order:**
1. `insert_code` missing `guard_worktree_write` — one-liner, clear correctness bug
2. `is_valid_symbol_start_line` language scope — needs design decision before implementing
3. `did_change` version counter — low-risk minor improvement
4. Ambiguous name matching — UX improvement, doesn't corrupt data
