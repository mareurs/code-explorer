# list_symbols Quality Refactor — Design

**Date:** 2026-03-01  
**Status:** Approved  
**Goal:** Surface LSP function signatures in `list_symbols` output; remove per-symbol redundancy.

---

## Problem

`list_symbols` (and `find_symbol`) output contains two categories of waste:

**Redundant fields (always the same value):**
- `"file"` — in single-file mode, duplicates the top-level `"file"` key; in directory/glob mode, duplicates the parent object's `"file"` key. Children carry it too.
- `"source"` — hardcoded `"project"` on every symbol, every child, in every mode. Scope parsing exists but does not affect output.

**Missing useful information:**
- `"signature"` — LSP's `DocumentSymbol.detail` field contains function signatures (parameters + return types). `convert_document_symbols` currently ignores it completely. Agents must make a separate `hover` call per function to learn its signature.

**Token cost (per symbol, exploring mode):**
```
"file": "src/tools/symbol.rs",   ← ~30 chars, always redundant in list_symbols
"source": "project",             ← ~20 chars, never informative
```
~50 chars × 50 symbols per typical file = ~2,500 chars wasted per call.

---

## Design

### Approach: B — Add `signature` + strip redundant fields

Extend the data model to capture `detail` from LSP; update serialization to emit `signature` and suppress `file`/`source` per-symbol in `list_symbols`. `find_symbol` keeps `file` (it produces a flat cross-file list where `file` is the only location context).

---

## Changes

### 1. `src/lsp/symbols.rs` — extend `SymbolInfo`

Add one optional field:

```rust
pub struct SymbolInfo {
    pub name: String,
    pub name_path: String,
    pub kind: SymbolKind,
    pub file: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub children: Vec<SymbolInfo>,
    /// LSP DocumentSymbol.detail — function signature or type annotation.
    /// None when LSP does not provide it (tree-sitter fallback, flat SymbolInformation, struct/type symbols).
    pub detail: Option<String>,
}
```

Default for all existing construction sites: `detail: None`.

### 2. `src/lsp/client.rs` — capture `detail` in `convert_document_symbols`

```rust
// Before
super::SymbolInfo {
    name: ds.name.clone(),
    ...
    children,
}

// After
super::SymbolInfo {
    name: ds.name.clone(),
    ...
    children,
    detail: ds.detail.filter(|s| !s.is_empty()),
}
```

The flat `SymbolInformation` fallback path has no `detail` field — stays `None`.

### 3. `src/tools/symbol.rs` — update `symbol_to_json`

**Signature change** — add `show_file: bool` parameter:

```rust
fn symbol_to_json(
    sym: &SymbolInfo,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    show_file: bool,   // ← replaces `source: &str`
) -> Value
```

**Output changes:**

| Field | Before | After |
|---|---|---|
| `"file"` | always present | only when `show_file = true` |
| `"source"` | always `"project"` | removed |
| `"signature"` | absent | present when `sym.detail` is `Some` |

**Call sites:**

| Call site | `show_file` |
|---|---|
| `list_symbols` (single-file, directory, glob) | `false` |
| `find_symbol` workspace search (line ~677) | `true` |

### 4. `src/tools/ast.rs` — `collect_functions` / `list_functions`

Remove `"source": "project"` from the emitted JSON in `collect_functions`. Same rationale — constant noise. The `SymbolInfo` struct change requires adding `detail: None` to any manual `SymbolInfo` construction in tree-sitter paths.

---

## Output Examples

### list_symbols — single file (Rust, rust-analyzer)

**Before:**
```json
{
  "file": "src/tools/symbol.rs",
  "symbols": [
    {"name":"call","name_path":"ListSymbols/call","kind":"Method",
     "file":"src/tools/symbol.rs","start_line":299,"end_line":471,"source":"project"},
    {"name":"is_glob","name_path":"is_glob","kind":"Function",
     "file":"src/tools/symbol.rs","start_line":15,"end_line":17,"source":"project"}
  ]
}
```

**After:**
```json
{
  "file": "src/tools/symbol.rs",
  "symbols": [
    {"name":"call","name_path":"ListSymbols/call","kind":"Method",
     "start_line":299,"end_line":471,
     "signature":"(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value>"},
    {"name":"is_glob","name_path":"is_glob","kind":"Function",
     "start_line":15,"end_line":17,
     "signature":"(path: &str) -> bool"}
  ]
}
```

### find_symbol — cross-file flat list (file stays)

```json
{
  "symbols": [
    {"name":"call","name_path":"ListSymbols/call","kind":"Method",
     "file":"src/tools/symbol.rs","start_line":299,"end_line":471,
     "signature":"(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value>"}
  ]
}
```

### Language coverage

| Language | LSP | `detail` quality |
|---|---|---|
| Rust | rust-analyzer | Excellent — full param list + return type |
| TypeScript/JS | tsserver | Good — `(method) Foo.bar: (x: T) => R` |
| Go | gopls | Good — `func (r *T) Handle(...)` |
| Python | pyright | Good; pylsp weaker (often module path only) |
| Java | eclipse.jdt.ls | Mixed — often parent class name |
| Kotlin | kotlin-lsp | Similar to Java |
| C/C++ | clangd | Good — full signature |

When `detail` is absent or empty, `"signature"` is omitted. No regression for languages where the LSP doesn't populate it.

---

## Tests to Add

- `convert_document_symbols_captures_detail` — verifies `ds.detail` maps to `sym.detail`; empty string collapses to `None`
- `symbol_to_json_omits_file_when_show_file_false`
- `symbol_to_json_includes_file_when_show_file_true`
- `symbol_to_json_includes_signature_when_detail_present`
- `symbol_to_json_omits_signature_when_detail_absent`

Update any existing tests that assert `"source": "project"` on symbol JSON.

---

## Out of Scope

- Sorting by kind/visibility (language-specific, deferred)
- `find_references` `"source"` field — different tool, separate decision
- Full scope-awareness (library vs project) — scope parsing exists but output is unaffected; can be revisited when library scope is fully implemented
