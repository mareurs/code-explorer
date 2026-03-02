# list_symbols Quality Refactor — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Surface LSP function signatures in `list_symbols`/`find_symbol` output and remove per-symbol `file`/`source` noise.

**Architecture:** Add `detail: Option<String>` to `SymbolInfo`, capture it in `convert_document_symbols`, update `symbol_to_json` to emit `"signature"` and suppress `"file"`/`"source"` per-symbol. `list_symbols` passes `show_file: false`; `find_symbol` passes `show_file: true` (cross-file flat output needs file context).

**Tech Stack:** Rust, lsp_types crate (`DocumentSymbol.detail`), serde (add `#[serde(default)]` to new field).

**Design doc:** `docs/plans/2026-03-01-list-symbols-quality-design.md`

---

### Task 1: Extend `SymbolInfo` with `detail` field

**Files:**
- Modify: `src/lsp/symbols.rs`

**Step 1: Add the field**

In `src/lsp/symbols.rs`, add to `SymbolInfo`:

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
    /// None when the LSP does not provide it (tree-sitter fallback, flat
    /// SymbolInformation path, or symbols with no natural signature like structs).
    #[serde(default)]
    pub detail: Option<String>,
}
```

The `#[serde(default)]` ensures deserialization of old stored data (without the field) still works.

**Step 2: Run the compiler to find all broken construction sites**

```bash
cargo build 2>&1 | grep "missing field"
```

Expected: a list of `SymbolInfo { ... }` construction sites that are now missing `detail`.

**Step 3: Add `detail: None` to every construction site**

Files to update:
- `src/ast/parser.rs` — many tree-sitter construction sites (every `symbols.push(SymbolInfo { ... })`)
- `src/lsp/client.rs` — hierarchical path at ~line 68, flat paths at ~line 453 and ~line 545, test construction at ~line 1132
- `src/tools/symbol.rs` — test `SymbolInfo` constructions (augment_body_range tests, find_symbol_in_tree tests, collect_matching_* tests)

For each: add `detail: None,` as the last field before the closing `}`.

**Step 4: Confirm it compiles and tests pass**

```bash
cargo build && cargo test
```

Expected: all existing tests pass, zero compile errors.

**Step 5: Commit**

```bash
git add src/lsp/symbols.rs src/ast/parser.rs src/lsp/client.rs src/tools/symbol.rs
git commit -m "feat(symbols): add detail field to SymbolInfo for LSP signatures"
```

---

### Task 2: Capture `detail` from LSP in `convert_document_symbols`

**Files:**
- Modify: `src/lsp/client.rs`

**Step 1: Write the failing tests**

In `src/lsp/client.rs`, in the `#[cfg(test)]` block near the existing `convert_document_symbols_uses_selection_range` test, add:

```rust
#[test]
fn convert_document_symbols_captures_detail() {
    use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

    let symbols = vec![DocumentSymbol {
        name: "my_func".to_string(),
        detail: Some("(x: i32) -> bool".to_string()),
        kind: LspSymbolKind::FUNCTION,
        tags: None,
        #[allow(deprecated)]
        deprecated: None,
        range: Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 5, character: 1 },
        },
        selection_range: Range {
            start: Position { line: 0, character: 3 },
            end: Position { line: 0, character: 10 },
        },
        children: None,
    }];

    let path = std::env::temp_dir().join("test_detail.rs");
    let result = convert_document_symbols(&symbols, &path, "");

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].detail,
        Some("(x: i32) -> bool".to_string()),
        "detail should be captured from DocumentSymbol"
    );
}

#[test]
fn convert_document_symbols_collapses_empty_detail() {
    use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

    let symbols = vec![DocumentSymbol {
        name: "my_func".to_string(),
        detail: Some("".to_string()), // empty string must become None
        kind: LspSymbolKind::FUNCTION,
        tags: None,
        #[allow(deprecated)]
        deprecated: None,
        range: Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 5, character: 1 },
        },
        selection_range: Range {
            start: Position { line: 0, character: 3 },
            end: Position { line: 0, character: 10 },
        },
        children: None,
    }];

    let path = std::env::temp_dir().join("test_detail_empty.rs");
    let result = convert_document_symbols(&symbols, &path, "");

    assert_eq!(
        result[0].detail,
        None,
        "empty string detail should collapse to None"
    );
}
```

**Step 2: Run to confirm they fail**

```bash
cargo test convert_document_symbols_captures_detail
cargo test convert_document_symbols_collapses_empty_detail
```

Expected: FAIL — `assert_eq` fails because `detail` is always `None` currently.

**Step 3: Implement the capture**

In `convert_document_symbols` in `src/lsp/client.rs`, find the `super::SymbolInfo { ... }` block (around line 68). Change:

```rust
// Before
super::SymbolInfo {
    name: ds.name.clone(),
    name_path: name_path.clone(),
    kind: ds.kind.into(),
    file: file.clone(),
    start_line: ds.selection_range.start.line,
    end_line: ds.range.end.line,
    start_col: ds.selection_range.start.character,
    children,
}

// After
super::SymbolInfo {
    name: ds.name.clone(),
    name_path: name_path.clone(),
    kind: ds.kind.into(),
    file: file.clone(),
    start_line: ds.selection_range.start.line,
    end_line: ds.range.end.line,
    start_col: ds.selection_range.start.character,
    children,
    detail: ds.detail.clone().filter(|s| !s.is_empty()),
}
```

The two flat `SymbolInformation` paths (`~line 453`, `~line 545`) stay with `detail: None` — `SymbolInformation` has no `detail` field.

**Step 4: Run tests to confirm they pass**

```bash
cargo test convert_document_symbols
```

Expected: all `convert_document_symbols_*` tests pass.

**Step 5: Commit**

```bash
git add src/lsp/client.rs
git commit -m "feat(lsp): capture DocumentSymbol.detail into SymbolInfo"
```

---

### Task 3: Update `symbol_to_json` — drop `file`/`source`, add `signature`

**Files:**
- Modify: `src/tools/symbol.rs`

**Step 1: Write the failing tests**

In `src/tools/symbol.rs`, in the `#[cfg(test)]` block, add a helper and 5 tests:

```rust
#[cfg(test)]
fn make_test_sym(name: &str, detail: Option<&str>) -> crate::lsp::SymbolInfo {
    crate::lsp::SymbolInfo {
        name: name.to_string(),
        name_path: name.to_string(),
        kind: crate::lsp::SymbolKind::Function,
        file: std::path::PathBuf::from("src/foo.rs"),
        start_line: 0,
        end_line: 5,
        start_col: 0,
        children: vec![],
        detail: detail.map(|s| s.to_string()),
    }
}

#[test]
fn symbol_to_json_omits_file_when_show_file_false() {
    let sym = make_test_sym("foo", None);
    let result = symbol_to_json(&sym, false, None, 0, false);
    assert!(
        result.get("file").is_none(),
        "file must be absent when show_file=false, got: {result}"
    );
    assert_eq!(result["name"], "foo");
}

#[test]
fn symbol_to_json_includes_file_when_show_file_true() {
    let sym = make_test_sym("foo", None);
    let result = symbol_to_json(&sym, false, None, 0, true);
    assert_eq!(result["file"], "src/foo.rs");
}

#[test]
fn symbol_to_json_includes_signature_when_detail_present() {
    let sym = make_test_sym("foo", Some("(x: i32) -> bool"));
    let result = symbol_to_json(&sym, false, None, 0, false);
    assert_eq!(result["signature"], "(x: i32) -> bool");
}

#[test]
fn symbol_to_json_omits_signature_when_detail_absent() {
    let sym = make_test_sym("foo", None);
    let result = symbol_to_json(&sym, false, None, 0, false);
    assert!(
        result.get("signature").is_none(),
        "signature must be absent when detail=None"
    );
}

#[test]
fn symbol_to_json_never_includes_source_field() {
    let sym = make_test_sym("foo", None);
    for show_file in [false, true] {
        let result = symbol_to_json(&sym, false, None, 0, show_file);
        assert!(
            result.get("source").is_none(),
            "source field must never appear (show_file={show_file})"
        );
    }
}
```

**Step 2: Run to confirm they fail**

```bash
cargo test symbol_to_json_omits_file_when_show_file_false
```

Expected: compile error — `symbol_to_json` doesn't have a `bool` last parameter yet.

**Step 3: Rewrite `symbol_to_json`**

Replace the entire function (currently `fn symbol_to_json(sym, include_body, source_code, depth, source: &str)`) with:

```rust
fn symbol_to_json(
    sym: &SymbolInfo,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    show_file: bool,
) -> Value {
    let mut obj = json!({
        "name": sym.name,
        "name_path": sym.name_path,
        "kind": format!("{:?}", sym.kind),
        "start_line": sym.start_line + 1,
        "end_line": sym.end_line + 1,
    });

    if show_file {
        obj["file"] = json!(sym.file.display().to_string());
    }

    if let Some(sig) = &sym.detail {
        obj["signature"] = json!(sig);
    }

    if include_body {
        if let Some(src) = source_code {
            let lines: Vec<&str> = src.lines().collect();
            let start = sym.start_line as usize;
            let end = (sym.end_line as usize + 1).min(lines.len());
            if start < lines.len() {
                obj["body"] = json!(lines[start..end].join("\n"));
            }
        }
    }

    if depth > 0 && !sym.children.is_empty() {
        obj["children"] = json!(sym
            .children
            .iter()
            .map(|c| symbol_to_json(c, include_body, source_code, depth - 1, show_file))
            .collect::<Vec<_>>());
    }

    obj
}
```

Note: the recursive child call now passes `show_file` — children inherit the same mode as their parent.

**Step 4: Fix compile errors at call sites**

`cargo build` will flag every call site that still passes a `&str`. All `list_symbols` call sites (glob, single-file, directory paths — 3 sites) should pass `false`. All `find_symbol` call sites (collect_matching at ~line 175, workspace search at ~line 677) should pass `true`. Fix them:

```rust
// list_symbols call sites (3 locations) — change last arg:
symbol_to_json(s, include_body, source.as_deref(), depth, false)

// find_symbol call sites (2 locations) — change last arg:
symbol_to_json(&sym, include_body, source.as_deref(), depth, true)
```

**Step 5: Run tests**

```bash
cargo test symbol_to_json
```

Expected: all 5 new tests pass. All existing tests pass.

**Step 6: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(symbols): add signature field, remove redundant file/source from symbol_to_json"
```

---

### Task 4: Clean up `collect_functions` in `list_functions`

**Files:**
- Modify: `src/tools/ast.rs`

**Step 1: Write a failing test**

In `src/tools/ast.rs` tests, add:

```rust
#[test]
fn list_functions_omits_source_field() {
    // collect_functions must not emit "source" — it's constant noise.
    use crate::lsp::{SymbolInfo, SymbolKind};
    use std::path::PathBuf;

    let syms = vec![SymbolInfo {
        name: "my_fn".to_string(),
        name_path: "my_fn".to_string(),
        kind: SymbolKind::Function,
        file: PathBuf::from("src/lib.rs"),
        start_line: 0,
        end_line: 5,
        start_col: 0,
        children: vec![],
        detail: None,
    }];

    let mut out = vec![];
    collect_functions(&syms, &mut out);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].get("source").is_none(),
        "collect_functions must not emit 'source' field"
    );
}
```

**Step 2: Run to confirm it fails**

```bash
cargo test list_functions_omits_source_field
```

Expected: FAIL — `source` is present.

**Step 3: Remove `"source"` from `collect_functions`**

In `src/tools/ast.rs`, `collect_functions`, change:

```rust
// Before
out.push(json!({
    "name": sym.name,
    "name_path": sym.name_path,
    "kind": sym.kind,
    "start_line": sym.start_line + 1,
    "end_line": sym.end_line + 1,
    "source": "project",
}));

// After
out.push(json!({
    "name": sym.name,
    "name_path": sym.name_path,
    "kind": sym.kind,
    "start_line": sym.start_line + 1,
    "end_line": sym.end_line + 1,
}));
```

**Step 4: Run tests**

```bash
cargo test list_functions
```

Expected: new test passes, existing list_functions tests pass.

**Step 5: Commit**

```bash
git add src/tools/ast.rs
git commit -m "refactor(ast): remove constant source field from list_functions output"
```

---

### Task 5: Final verification

**Step 1: Format**

```bash
cargo fmt
```

**Step 2: Lint**

```bash
cargo clippy -- -D warnings
```

Fix any warnings before proceeding.

**Step 3: Full test suite**

```bash
cargo test
```

Expected: all tests pass. Count should be ≥ previous count + ~8 new tests.

**Step 4: Manual smoke check**

If LSP is available locally, run the MCP server and call `list_symbols` on a Rust file to confirm `"signature"` appears and `"file"`/`"source"` are absent per-symbol:

```bash
cargo run -- start --project .
```

Then in another session call `list_symbols(path="src/tools/symbol.rs", depth=1)`.

**Step 5: Final commit if fmt/clippy changed anything**

```bash
git add -p
git commit -m "style: fmt and clippy after list_symbols quality refactor"
```
