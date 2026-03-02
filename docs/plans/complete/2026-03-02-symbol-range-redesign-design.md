# Symbol Range Redesign — "Trust LSP"

**Date:** 2026-03-02
**Status:** Design approved
**Motivation:** BUG-016 exposed that our range-manipulation heuristics are brittle and language-specific. Research into Serena, Krait, and Zed shows that every other LSP-wrapping project trusts LSP ranges directly. Our heuristics are the outlier — and the source of corruption bugs.

## Philosophy

**Trust the LSP for ranges. Validate, don't fix. Fail loudly.**

Remove all range-manipulation heuristics that try to "correct" LSP ranges. The only post-processing we keep is:

- **Bounds checking** — prevents panics on out-of-range indices
- **Degenerate range detection** — tree-sitter catches obviously wrong ranges (`start == end` for multi-line symbols), returns `RecoverableError` instead of silently fixing
- **Doc-comment walk for insert positioning only** — Krait pattern: decides *where* to insert new code before a symbol, never modifies a symbol's own range

## Research Summary

| Project | Language | Approach to LSP ranges |
|---------|----------|----------------------|
| **Serena** (oraios/serena) | Python | Trusts LSP `range` completely. Zero post-processing. 44KB symbol.py but no range manipulation. |
| **Krait** (Codestz/krait) | Rust | Trusts LSP `range` completely. `lines.splice(start..end+1, new_lines)` is the entire replace. Doc-comment walk only for `insert_before` positioning. |
| **Zed** (zed-industries/zed) | Rust | Richest model (`range`, `selectionRange`, `body_range`, `annotation_range`) but doesn't do symbol editing. |
| **code-explorer** (us) | Rust | 8 range-manipulation helpers, each BUG fix applied only to the tool that triggered it. Source of BUG-016. |

**Krait is our reference design** — simplest, most honest about what it doesn't know, language-agnostic.

## Current State: The Problem

### 8 Range-Manipulation Helpers

| Function | Purpose | BUG it fixes |
|----------|---------|-------------|
| `trim_symbol_start` | Skip preceding `}` and blank lines | BUG-003 |
| `trim_symbol_end` | Skip following `{` and blank lines | BUG-004 |
| `clamp_end_to_closing_brace` | Walk backward to find `}` | BUG-014 (caused BUG-016) |
| `is_declaration_line` | Reject non-declaration start lines | BUG-013 |
| `scan_backwards_for_docs` | Include doc comments in removal range | BUG-010 |
| `collapse_blank_lines` | Clean up double blanks after removal | Cosmetic |
| `augment_body_range_from_ast` | Fix degenerate (single-line) LSP ranges | gopls workaround |
| `find_ast_end_line_in` | Tree-sitter lookup for `augment_body_range_from_ast` | Helper |

### Inconsistent Application

Each BUG fix was applied only to the tool that triggered it:

| Defense | `replace_symbol` | `remove_symbol` | `insert_code` |
|---------|-----------------|----------------|--------------|
| `trim_symbol_start` | Yes | Yes | Yes |
| `is_declaration_line` | Yes | No | No |
| `clamp_end_to_closing_brace` | No | Yes | No |
| `trim_symbol_end` | No | No | Yes (after) |
| `scan_backwards_for_docs` | No | Yes | Yes (before) |
| Inverted-range guard | No | Yes | No |
| `collapse_blank_lines` | No | Yes | No |

Additionally:
- `is_declaration_line` is **Rust-only** — its prefix list doesn't cover Python, TypeScript, Go, Java, or Kotlin
- The **read path** (`find_symbol include_body=true`) uses raw LSP ranges, while write tools apply various trims — agents see a different body extent than what `replace_symbol` would replace
- `clamp_end_to_closing_brace` **caused BUG-016** — the fix for BUG-014 introduced a worse bug (file corruption + panic on `const` items)

## Design

### Functions to Remove

| Function | Why it existed | Why we remove it |
|----------|---------------|-----------------|
| `trim_symbol_start` | rust-analyzer includes preceding `}` in range | Fixing LSP bugs at wrong layer. Trust the LSP. |
| `trim_symbol_end` | LSP over-extends into next symbol's `{` | Same — trust LSP. |
| `clamp_end_to_closing_brace` | Walk backward to find `}` | Source of BUG-016. The cure was worse than the disease. |
| `is_declaration_line` | LSP resolves to inner `let` binding | Rust-only. Doesn't work for any other language. |
| `scan_backwards_for_docs` (from remove/replace paths) | Delete symbol without deleting its docs | Most LSPs include docs in `range` per spec. If not, orphaned docs are cosmetic. |
| `collapse_blank_lines` | Cosmetic cleanup after removal | Over-engineering. Agent can clean up if needed. |

### Functions to Keep/Modify

#### 1. `find_insert_before_line` (renamed from `scan_backwards_for_docs`)

Kept **only** for `insert_code(before)`. Walks upward past doc comments and attributes to find where to insert new code. Never modifies a symbol's range.

Extended with Krait's language-agnostic patterns:

```rust
fn find_insert_before_line(lines: &[&str], symbol_start: usize) -> usize {
    let mut cursor = symbol_start;
    while cursor > 0 {
        let trimmed = lines[cursor - 1].trim();
        let is_attr_or_doc = trimmed.starts_with("#[")     // Rust attributes
            || trimmed.starts_with('@')                     // Python/Java/TS decorators
            || trimmed.starts_with("///")                   // Rust doc comments
            || trimmed.starts_with("//!")                   // Rust inner doc comments
            || trimmed.starts_with("/**")                   // JSDoc/JavaDoc
            || trimmed.starts_with("* ")                    // JSDoc/JavaDoc continuation
            || trimmed == "*/"                              // Block comment end
            || trimmed.starts_with("/*");                   // Block comment start
        if is_attr_or_doc {
            cursor -= 1;
        } else {
            break;
        }
    }
    cursor
}
```

Key difference from current `scan_backwards_for_docs`: does NOT consume blank lines. A blank line stops the walk, which correctly separates unrelated code from doc comments.

#### 2. `validate_symbol_range` (new, replaces `augment_body_range_from_ast`)

Detects degenerate LSP ranges where `start_line == end_line` but tree-sitter says the symbol spans multiple lines. Returns `RecoverableError` instead of silently fixing.

```rust
fn validate_symbol_range(sym: &SymbolInfo) -> Result<()> {
    if sym.start_line != sym.end_line {
        return Ok(()); // Non-degenerate range, trust it
    }
    // Single-line range — check if tree-sitter disagrees
    let Ok(ast_syms) = crate::ast::extract_symbols(&sym.file) else {
        return Ok(()); // No AST available, trust LSP
    };
    if let Some(ast_end) = find_ast_end_line_in(&ast_syms, &sym.name, sym.start_line) {
        if ast_end > sym.start_line + 1 {
            // LSP says 1 line, tree-sitter says many — suspicious
            bail!(RecoverableError::with_hint(
                format!(
                    "LSP returned suspicious range for '{}' (line {}, but AST shows it spans to line {})",
                    sym.name, sym.start_line + 1, ast_end + 1,
                ),
                "The LSP server may have returned a selection range instead of the full symbol range. \
                 Try edit_file for this symbol, or check list_symbols to verify the range.",
            ));
        }
    }
    Ok(())
}
```

### New Tool Logic

#### `replace_symbol`

```
validate_symbol_range(sym)
start = sym.start_line as usize
end   = (sym.end_line as usize + 1).min(lines.len())
guard: start >= lines.len() → RecoverableError
lines.splice(start..end, new_body_lines)
atomic_write → json!("ok")
```

#### `remove_symbol`

```
validate_symbol_range(sym)
start = sym.start_line as usize
end   = (sym.end_line as usize + 1).min(lines.len())
guard: start >= lines.len() → RecoverableError
lines.splice(start..end, empty)
atomic_write → json!("ok")
```

#### `insert_code(before)`

```
validate_symbol_range(sym)
insert_at = find_insert_before_line(lines, sym.start_line as usize)
lines.splice(insert_at..insert_at, new_code_lines + blank_separator)
atomic_write → json!("ok")
```

#### `insert_code(after)`

```
validate_symbol_range(sym)
insert_at = (sym.end_line as usize + 1).min(lines.len())
// Add blank separator if next line is non-empty
if lines.get(insert_at).is_some_and(|l| !l.trim().is_empty()) {
    prepend blank line
}
lines.splice(insert_at..insert_at, [separator +] new_code_lines)
atomic_write → json!("ok")
```

### Consistency After Redesign

| Check | `replace_symbol` | `remove_symbol` | `insert_code` |
|-------|-----------------|----------------|--------------|
| `validate_symbol_range` | Yes | Yes | Yes |
| Bounds check (`start >= len`) | Yes | Yes | Yes |
| `find_insert_before_line` | — | — | Yes (before only) |
| Blank separator | — | — | Yes (after only) |

Same defenses everywhere. No special cases per tool.

### Read/Write Path Consistency

Currently `find_symbol(include_body=true)` extracts body using raw `[start_line..end_line+1]`, while write tools apply various trims. After this redesign, both paths use the same range — agents see exactly what `replace_symbol` would replace.

## BUG Impact Analysis

| BUG | What happens after redesign |
|-----|---------------------------|
| BUG-003 (preceding `}` in range) | Included in body. Agent sees it in `include_body` output. Consistent read/write. |
| BUG-004 (following `{` in range) | Included in body. Same consistency argument. |
| BUG-010 (doc comments not in removal) | If LSP includes docs in `range` (per spec), they're removed. If not, they're orphaned — agent can clean up. |
| BUG-013 (inner `let` binding resolution) | LSP gives wrong symbol → we edit wrong range. Agent sees the wrong body via `include_body` and can detect the issue. `RecoverableError` if range is degenerate. |
| BUG-014 (over-extension past `}`) | Trusted. If LSP over-extends, we over-extend. But this is visible in `include_body` output. |
| BUG-016 (panic on `const` removal) | **Eliminated.** No `clamp_end_to_closing_brace` → no backward walk → no underflow → no panic/corruption. |

## Migration Notes

### Tests to Update

Existing tests that assert trim/clamp behavior will need updating:
- Tests for `trim_symbol_start` / `trim_symbol_end` → delete
- Tests for `clamp_end_to_closing_brace` → delete
- Tests for `is_declaration_line` → delete
- Tests for `scan_backwards_for_docs` → adapt for `find_insert_before_line` (no blank line consumption)
- Tests for `collapse_blank_lines` → delete
- `remove_symbol_handles_const_without_closing_brace` → simplify (no longer needs the brace-walk fix, just trust LSP range)
- Tests that mock LSP ranges assuming trim behavior → update expected outputs

### BUG Log Updates

Update `docs/TODO-tool-misbehaviors.md`:
- BUG-003, BUG-004, BUG-010, BUG-013, BUG-014: Mark as "resolved by design" — we no longer work around these LSP behaviors
- BUG-016: Already fixed, now doubly resolved (the function that caused it no longer exists)

## Risk Assessment

**What could go wrong:**
- rust-analyzer returns range including preceding `}` → `replace_symbol` replaces the `}` too → compilation error → agent retries with `edit_file`. Visible, recoverable, not silent corruption.
- LSP doesn't include doc comments in `range` → `remove_symbol` leaves orphaned docs → agent sees them and cleans up. Cosmetic, not corruption.
- gopls returns degenerate range → `validate_symbol_range` catches it → `RecoverableError` with hint to use `edit_file`. Explicit, not silent.

**What we gain:**
- No more silent file corruption (BUG-016 class eliminated)
- Language-agnostic (no Rust-only `is_declaration_line`)
- Consistent read/write paths
- ~150 lines of heuristic code removed
- Uniform defense matrix across all 3 write tools
