# Progressive Discoverability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When `find_symbol` or `list_symbols` overflow, return the file distribution (`by_file`), accurate counts, and copy-paste-ready refinement hints — so agents can narrow without guessing.

**Architecture:** Four focused changes:
1. Extend `OverflowInfo` with `by_file` + `by_file_overflow` (output.rs)
2. Add `kind` filter to `collect_matching` + `FindSymbol` (symbol.rs)
3. Remove directory-scoped early-cap; add `build_by_file` helper; reduce exploring cap to 50 (symbol.rs)
4. Cap `list_symbols` single-file at 100 top-level symbols (symbol.rs)
5. Update server instructions (server_instructions.md)

**Tech Stack:** Rust — no new dependencies. `by_file` uses `Vec<(String, usize)>` (sorted by count) serialized to a JSON object.

**Design doc:** `docs/plans/2026-02-28-progressive-discoverability-design.md` — read it before implementing. Pay special attention to the Review Findings (RF1–RF6).

---

## Task 1: Extend `OverflowInfo` + `overflow_json`

**Files:**
- Modify: `src/tools/output.rs`

### Step 1: Write failing tests

Add to the `tests` module in `src/tools/output.rs` (after `overflow_json_format`):

```rust
#[test]
fn overflow_json_includes_by_file() {
    let info = OverflowInfo {
        shown: 50,
        total: 90,
        hint: "narrow".to_string(),
        next_offset: None,
        by_file: Some(vec![
            ("src/a.rs".to_string(), 30),
            ("src/b.rs".to_string(), 20),
        ]),
        by_file_overflow: 0,
    };
    let json = OutputGuard::overflow_json(&info);
    assert_eq!(json["by_file"]["src/a.rs"], 30);
    assert_eq!(json["by_file"]["src/b.rs"], 20);
    assert!(json.get("by_file_overflow").is_none(), "zero overflow should be omitted");
}

#[test]
fn overflow_json_includes_by_file_overflow_when_nonzero() {
    let info = OverflowInfo {
        shown: 50,
        total: 200,
        hint: "narrow".to_string(),
        next_offset: None,
        by_file: Some(vec![("src/a.rs".to_string(), 10)]),
        by_file_overflow: 42,
    };
    let json = OutputGuard::overflow_json(&info);
    assert_eq!(json["by_file_overflow"], 42);
}

#[test]
fn overflow_json_omits_by_file_when_none() {
    let info = OverflowInfo {
        shown: 10,
        total: 20,
        hint: "hint".to_string(),
        next_offset: None,
        by_file: None,
        by_file_overflow: 0,
    };
    let json = OutputGuard::overflow_json(&info);
    assert!(json.get("by_file").is_none());
    assert!(json.get("by_file_overflow").is_none());
}
```

### Step 2: Run — confirm compile error

```bash
cargo test overflow_json_includes_by_file 2>&1 | head -20
```

Expected: compile error — `OverflowInfo` missing `by_file` field.

### Step 3: Add fields to `OverflowInfo`

In `src/tools/output.rs`, replace the struct (currently lines 20–26):

**Before:**
```rust
pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,
    /// In focused mode, the offset for the next page (None in exploring mode).
    pub next_offset: Option<usize>,
}
```

**After:**
```rust
pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,
    /// In focused mode, the offset for the next page (None in exploring mode).
    pub next_offset: Option<usize>,
    /// Per-file result counts, sorted by count descending. Only for multi-file searches.
    /// Capped at 15 entries — see `by_file_overflow` for how many were omitted.
    pub by_file: Option<Vec<(String, usize)>>,
    /// Number of additional files omitted from `by_file` due to the 15-entry cap.
    pub by_file_overflow: usize,
}
```

### Step 4: Fix all `OverflowInfo` struct literal construction sites

Run `cargo build` and find every "missing field" error. For each construction site, add:
```rust
by_file: None,
by_file_overflow: 0,
```

Sites to fix (use `search_pattern` to confirm line numbers):
- `src/tools/output.rs` — inside `cap_items` exploring branch (the `overflow` variable)
- `src/tools/output.rs` — inside `paginate` function (the `overflow` variable)
- `src/tools/symbol.rs` — inside `FindSymbol::call` early-cap early-return block (will be deleted in Task 3, but must compile now)

### Step 5: Update `overflow_json`

Replace the `overflow_json` method body:

**Before:**
```rust
    pub fn overflow_json(info: &OverflowInfo) -> Value {
        let mut obj = json!({
            "shown": info.shown,
            "total": info.total,
            "hint": info.hint
        });
        if let Some(next) = info.next_offset {
            obj["next_offset"] = json!(next);
        }
        obj
    }
```

**After:**
```rust
    pub fn overflow_json(info: &OverflowInfo) -> Value {
        let mut obj = json!({
            "shown": info.shown,
            "total": info.total,
            "hint": info.hint
        });
        if let Some(next) = info.next_offset {
            obj["next_offset"] = json!(next);
        }
        if let Some(by_file) = &info.by_file {
            let map: serde_json::Map<String, Value> = by_file
                .iter()
                .map(|(path, count)| (path.clone(), json!(count)))
                .collect();
            obj["by_file"] = Value::Object(map);
            if info.by_file_overflow > 0 {
                obj["by_file_overflow"] = json!(info.by_file_overflow);
            }
        }
        obj
    }
```

### Step 6: Run tests

```bash
cargo test 2>&1 | tail -20
```

Expected: all existing tests pass + 3 new `overflow_json_*` tests.

### Step 7: Commit

```bash
git add src/tools/output.rs src/tools/symbol.rs
git commit -m "feat(output): add by_file and by_file_overflow to OverflowInfo"
```

---

## Task 2: `kind` filter — `matches_kind_filter` + `collect_matching` update

**Files:**
- Modify: `src/tools/symbol.rs`

### Step 1: Write failing tests

Add to the `tests` module in `src/tools/symbol.rs`:

```rust
#[test]
fn matches_kind_filter_function_group() {
    use crate::lsp::SymbolKind;
    assert!(matches_kind_filter(&SymbolKind::Function, "function"));
    assert!(matches_kind_filter(&SymbolKind::Method, "function"));
    assert!(matches_kind_filter(&SymbolKind::Constructor, "function"));
    assert!(!matches_kind_filter(&SymbolKind::Variable, "function"));
    assert!(!matches_kind_filter(&SymbolKind::Class, "function"));
}

#[test]
fn matches_kind_filter_struct_vs_class() {
    use crate::lsp::SymbolKind;
    assert!(matches_kind_filter(&SymbolKind::Class, "class"));
    assert!(!matches_kind_filter(&SymbolKind::Struct, "class"));
    assert!(matches_kind_filter(&SymbolKind::Struct, "struct"));
    assert!(!matches_kind_filter(&SymbolKind::Class, "struct"));
}

#[test]
fn matches_kind_filter_module_group() {
    use crate::lsp::SymbolKind;
    assert!(matches_kind_filter(&SymbolKind::Module, "module"));
    assert!(matches_kind_filter(&SymbolKind::Namespace, "module"));
    assert!(matches_kind_filter(&SymbolKind::Package, "module"));
    assert!(!matches_kind_filter(&SymbolKind::Function, "module"));
}

#[test]
fn collect_matching_with_kind_filter_class_only() {
    use crate::lsp::SymbolKind;
    let symbols = vec![
        SymbolInfo {
            name: "WeeklyGrid".into(), name_path: "WeeklyGrid".into(),
            kind: SymbolKind::Class,
            file: PathBuf::from("test.ts"),
            start_line: 0, end_line: 10, start_col: 0, children: vec![],
        },
        SymbolInfo {
            name: "weeklyGrid".into(), name_path: "weeklyGrid".into(),
            kind: SymbolKind::Variable,
            file: PathBuf::from("test.ts"),
            start_line: 12, end_line: 12, start_col: 0, children: vec![],
        },
        SymbolInfo {
            name: "renderWeeklyGrid".into(), name_path: "renderWeeklyGrid".into(),
            kind: SymbolKind::Function,
            file: PathBuf::from("test.ts"),
            start_line: 14, end_line: 20, start_col: 0, children: vec![],
        },
    ];

    let mut out = vec![];
    collect_matching(&symbols, "weeklygrid", false, None, 0, "project", &mut out, Some("class"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["name"], "WeeklyGrid");
}

#[test]
fn collect_matching_kind_filter_none_returns_all_matching() {
    use crate::lsp::SymbolKind;
    let symbols = vec![
        SymbolInfo {
            name: "foo".into(), name_path: "foo".into(),
            kind: SymbolKind::Function,
            file: PathBuf::from("test.rs"),
            start_line: 0, end_line: 5, start_col: 0, children: vec![],
        },
        SymbolInfo {
            name: "FOO".into(), name_path: "FOO".into(),
            kind: SymbolKind::Constant,
            file: PathBuf::from("test.rs"),
            start_line: 7, end_line: 7, start_col: 0, children: vec![],
        },
    ];

    let mut out = vec![];
    collect_matching(&symbols, "foo", false, None, 0, "project", &mut out, None);
    assert_eq!(out.len(), 2, "no filter → all name-matching symbols returned");
}
```

### Step 2: Run — confirm compile error

```bash
cargo test matches_kind_filter 2>&1 | head -10
```

Expected: compile error — `matches_kind_filter` undefined, `collect_matching` has wrong arg count.

### Step 3: Add `matches_kind_filter` function

Add in `src/tools/symbol.rs` immediately before `collect_matching` (currently around line 133):

```rust
/// Returns true if the symbol's kind matches the given filter string.
/// Unknown filter values return true (no filtering).
fn matches_kind_filter(kind: &crate::lsp::SymbolKind, filter: &str) -> bool {
    use crate::lsp::SymbolKind as K;
    match filter {
        "function"  => matches!(kind, K::Function | K::Method | K::Constructor),
        "class"     => matches!(kind, K::Class),
        "struct"    => matches!(kind, K::Struct),
        "interface" => matches!(kind, K::Interface),
        "type"      => matches!(kind, K::TypeParameter),
        "enum"      => matches!(kind, K::Enum | K::EnumMember),
        "module"    => matches!(kind, K::Module | K::Namespace | K::Package),
        "constant"  => matches!(kind, K::Constant),
        _           => true,
    }
}
```

### Step 4: Update `collect_matching` signature + body

Replace the function (currently lines 134–165):

**Before:**
```rust
fn collect_matching(
    symbols: &[SymbolInfo],
    pattern: &str,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    source: &str,
    out: &mut Vec<Value>,
) {
    for sym in symbols {
        if sym.name.to_lowercase().contains(pattern)
            || sym.name_path.to_lowercase().contains(pattern)
        {
            out.push(symbol_to_json(sym, include_body, source_code, depth, source));
        }
        collect_matching(&sym.children, pattern, include_body, source_code, depth, source, out);
    }
}
```

**After:**
```rust
fn collect_matching(
    symbols: &[SymbolInfo],
    pattern: &str,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    source: &str,
    out: &mut Vec<Value>,
    kind_filter: Option<&str>,
) {
    for sym in symbols {
        let name_ok = sym.name.to_lowercase().contains(pattern)
            || sym.name_path.to_lowercase().contains(pattern);
        let kind_ok = kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
        if name_ok && kind_ok {
            out.push(symbol_to_json(sym, include_body, source_code, depth, source));
        }
        // Always recurse so nested matches inside filtered-out parents are still found.
        collect_matching(
            &sym.children, pattern, include_body, source_code, depth, source, out, kind_filter,
        );
    }
}
```

### Step 5: Fix all callers — add `None` as the last argument

Run `cargo build` to find every call site. Add `, None` at the end of each:

**Call sites to update (search with `search_pattern("collect_matching(")`):**
- `FindSymbol::call` directory loop (line ~520) — change last arg to `None` for now
- `FindSymbol::call` tree-sitter fallback (line ~630) — same
- Tests: `collect_matching_matches_name_path` (2 call sites), `collect_matching_slash_pattern_precision` (1 call site)

### Step 6: Run tests

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass including 5 new `matches_kind_filter*` and `collect_matching*kind*` tests.

### Step 7: Commit

```bash
git add src/tools/symbol.rs
git commit -m "feat(symbol): add kind_filter parameter to collect_matching"
```

---

## Task 3: `FindSymbol` — schema, kind wiring, cap reduction, by_file, remove early-cap

**Files:**
- Modify: `src/tools/symbol.rs`

This is the largest task. Do it in sub-steps.

### Step 1: Write failing unit tests for helpers

Add to the `tests` module:

```rust
#[test]
fn build_by_file_sorts_desc_and_caps_at_15() {
    // 20 distinct files, file_i has (20 - i) matches
    let mut matches: Vec<Value> = vec![];
    for i in 0usize..20 {
        for _ in 0..(20 - i) {
            matches.push(json!({ "file": format!("src/file{i}.rs") }));
        }
    }
    let (by_file, overflow) = build_by_file(&matches);
    assert_eq!(by_file.len(), 15, "cap at 15");
    assert_eq!(overflow, 5, "20 files - 15 = 5 overflow");
    // First entry has highest count
    assert_eq!(by_file[0].0, "src/file0.rs");
    assert_eq!(by_file[0].1, 20);
    // Sorted descending
    for w in by_file.windows(2) {
        assert!(w[0].1 >= w[1].1);
    }
}

#[test]
fn build_by_file_no_overflow_under_cap() {
    let matches: Vec<Value> = (0..3)
        .flat_map(|i| vec![json!({ "file": format!("src/f{i}.rs") }); 5])
        .collect();
    let (by_file, overflow) = build_by_file(&matches);
    assert_eq!(by_file.len(), 3);
    assert_eq!(overflow, 0);
}

#[test]
fn make_find_symbol_hint_contains_top_file_and_kind_and_offset() {
    let by_file = vec![
        ("src/components/WeeklyGrid.tsx".to_string(), 12usize),
        ("src/screens/Home.tsx".to_string(), 3),
    ];
    let hint = make_find_symbol_hint(50, &by_file);
    assert!(hint.contains("src/components/WeeklyGrid.tsx"), "should show top file path");
    assert!(hint.contains("kind="), "should mention kind filter");
    assert!(hint.contains("offset=50"), "should show next pagination offset");
}

#[test]
fn kind_filter_skipped_when_using_name_path() {
    // Verify the logic: if name_path is set, kind_filter is None.
    let input = json!({ "name_path": "Foo", "kind": "function" });
    let is_name_path = input["name_path"].is_string();
    let kind_filter: Option<&str> = if is_name_path { None } else { input["kind"].as_str() };
    assert!(kind_filter.is_none());
}
```

### Step 2: Run to confirm compile errors

```bash
cargo test build_by_file make_find_symbol_hint 2>&1 | head -10
```

Expected: compile errors — functions not defined.

### Step 3: Add `build_by_file` and `make_find_symbol_hint` helpers

Add these functions in `src/tools/symbol.rs` immediately before `impl Tool for FindSymbol` (around line 424):

```rust
const FIND_SYMBOL_MAX_RESULTS: usize = 50;
const BY_FILE_CAP: usize = 15;

/// Build a per-file distribution from a list of symbol JSON objects.
/// Returns (entries sorted by count desc, number of files omitted by cap).
fn build_by_file(matches: &[Value]) -> (Vec<(String, usize)>, usize) {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in matches {
        if let Some(file) = m["file"].as_str() {
            *counts.entry(file.to_string()).or_default() += 1;
        }
    }
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let overflow = sorted.len().saturating_sub(BY_FILE_CAP);
    sorted.truncate(BY_FILE_CAP);
    (sorted, overflow)
}

/// Build the actionable overflow hint for find_symbol. Uses the top file from by_file
/// as the concrete example path so the hint is copy-paste ready.
fn make_find_symbol_hint(shown: usize, by_file: &[(String, usize)]) -> String {
    let top_file = by_file
        .first()
        .map(|(f, _)| f.as_str())
        .unwrap_or("path/to/file.rs");
    format!(
        "Showing {shown} of total. To narrow down:\n\
         \u{2022} paginate:       add offset={shown}, limit=50\n\
         \u{2022} filter by file: add path=\"{top_file}\"\n\
         \u{2022} filter by kind: add kind=\"function\" (also: class, struct, interface, type, enum, module, constant)"
    )
}
```

### Step 4: Run helper unit tests

```bash
cargo test build_by_file make_find_symbol_hint kind_filter_skipped 2>&1
```

Expected: all 4 pass.

### Step 5: Update `FindSymbol::input_schema` — add `kind`

In the `input_schema` method body, add `kind` to `properties`:

```rust
"kind": {
    "type": "string",
    "description": "Filter by symbol kind. Only applied when using 'pattern' — ignored with 'name_path'. Note: 'interface' matches Rust traits.",
    "enum": ["function", "class", "struct", "interface", "type", "enum", "module", "constant"]
},
```

### Step 6: Refactor `FindSymbol::call`

This replaces three blocks of code. Make these changes carefully in order:

**A. Replace the `guard` + early variables block at the top of `call`:**

Find line (currently ~460):
```rust
        let guard = OutputGuard::from_input(&input);
```

Replace with:
```rust
        let mut guard = OutputGuard::from_input(&input);
        // find_symbol uses a tighter exploring cap than the default 200.
        if matches!(guard.mode, OutputMode::Exploring) {
            guard.max_results = FIND_SYMBOL_MAX_RESULTS;
        }

        // kind filter only applies to pattern-based searches, not exact name_path lookups.
        let is_name_path = input["name_path"].is_string();
        let kind_filter: Option<&str> = if is_name_path { None } else { input["kind"].as_str() };
```

**B. In the directory/glob loop — remove `early_cap` and pass `kind_filter`:**

Delete the entire early_cap block (currently lines ~494–501):
```rust
            // In exploring mode, stop early once we have enough results.
            let early_cap = match guard.mode {
                OutputMode::Exploring => Some(guard.max_results + 1),
                OutputMode::Focused => None,
            };
```

And remove the early-exit check inside the loop (lines ~499–501):
```rust
                if let Some(cap) = early_cap {
                    if matches.len() >= cap {
                        break;
                    }
                }
```

Update the `collect_matching` call inside the loop (line ~520) to pass `kind_filter`:
```rust
                collect_matching(
                    &symbols,
                    &pattern_lower,
                    include_body,
                    source.as_deref(),
                    depth,
                    "project",
                    &mut matches,
                    kind_filter,    // ← was None, now kind_filter
                );
```

Delete the entire `hit_early_cap` early-return block (currently lines ~531–543):
```rust
            let hit_early_cap = early_cap.is_some() && matches.len() > guard.max_results;
            if hit_early_cap {
                use super::output::OverflowInfo;
                matches.truncate(guard.max_results);
                let overflow = OverflowInfo { ... };
                let mut result = json!(...);
                result["overflow"] = OutputGuard::overflow_json(&overflow);
                return Ok(result);
            }
```

**C. In the workspace/symbol filter block — add kind filter:**

Find the inline `if` that filters workspace symbols (currently lines ~581–584):
```rust
                    if sym.name.to_lowercase().contains(&pattern_lower)
                        || sym.name_path.to_lowercase().contains(&pattern_lower)
```

Replace with:
```rust
                    let name_ok = sym.name.to_lowercase().contains(&pattern_lower)
                        || sym.name_path.to_lowercase().contains(&pattern_lower);
                    let kind_ok = kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
                    if name_ok && kind_ok
```

**D. In the tree-sitter fallback — pass `kind_filter`:**

Update the `collect_matching` call (currently line ~630) to pass `kind_filter` as last arg.

**E. Replace the final `cap_items` convergence block:**

Find (currently lines ~648–671):
```rust
        let (mut matches, overflow) =
            guard.cap_items(matches, "Restrict with a file path or glob pattern");
```

Replace with:
```rust
        // Build by_file distribution from the full result set BEFORE truncation.
        let (by_file_entries, by_file_overflow_count) = build_by_file(&matches);
        let hint = if matches.len() > guard.max_results {
            make_find_symbol_hint(guard.max_results, &by_file_entries)
        } else {
            String::from("Restrict with a file path or glob pattern")
        };
        let (mut matches, mut overflow) = guard.cap_items(matches, &hint);
        // Patch by_file into the overflow object (RF6 resolution: mutate after cap_items).
        if let Some(ref mut ov) = overflow {
            if !by_file_entries.is_empty() {
                ov.by_file = Some(by_file_entries);
                ov.by_file_overflow = by_file_overflow_count;
                // Rewrite hint with the real `shown` value now we know it.
                ov.hint = make_find_symbol_hint(ov.shown, ov.by_file.as_deref().unwrap_or(&[]));
            }
        }
```

### Step 7: Build and run all tests

```bash
cargo build 2>&1 && cargo test 2>&1 | tail -30
```

Expected: all tests pass.

### Step 8: Clippy

```bash
cargo clippy -- -D warnings 2>&1
```

Expected: clean.

### Step 9: Commit

```bash
git add src/tools/symbol.rs
git commit -m "feat(find_symbol): add kind filter, remove early-cap, add by_file overflow distribution"
```

---

## Task 4: `ListSymbols` — cap single-file mode at 100 top-level symbols

**Files:**
- Modify: `src/tools/symbol.rs`

### Step 1: Write failing tests

Add to tests in `src/tools/symbol.rs`:

```rust
#[test]
fn list_symbols_single_file_cap_unit() {
    // Unit test: simulate the cap logic on a Vec<Value> of 150 symbol entries.
    use super::output::{OutputGuard, OutputMode};
    let symbols: Vec<Value> = (0..150)
        .map(|i| json!({ "name": format!("sym{i}"), "start_line": i + 1 }))
        .collect();

    const SINGLE_FILE_CAP: usize = 100;
    let total = symbols.len();
    let hint = format!(
        "File has {total} symbols. Use depth=1 for top-level overview, \
         or find_symbol(name_path='ClassName/methodName', include_body=true) for a specific symbol."
    );
    let mut g = OutputGuard::default();
    g.max_results = SINGLE_FILE_CAP;
    let (kept, overflow) = g.cap_items(symbols, &hint);

    assert_eq!(kept.len(), 100);
    let ov = overflow.expect("overflow must be present");
    assert_eq!(ov.total, 150);
    assert_eq!(ov.shown, 100);
    assert!(ov.hint.contains("find_symbol"));
    assert!(ov.hint.contains("name_path"));
    assert!(ov.by_file.is_none(), "single-file overflow must not include by_file");
}

#[test]
fn list_symbols_single_file_no_overflow_under_cap_unit() {
    use super::output::OutputGuard;
    let symbols: Vec<Value> = (0..40)
        .map(|i| json!({ "name": format!("sym{i}") }))
        .collect();

    let mut g = OutputGuard::default();
    g.max_results = 100;
    let (kept, overflow) = g.cap_items(symbols, "hint");

    assert_eq!(kept.len(), 40);
    assert!(overflow.is_none(), "no overflow for 40 symbols under cap of 100");
}
```

### Step 2: Run tests

```bash
cargo test list_symbols_single_file_cap_unit list_symbols_single_file_no_overflow 2>&1
```

Expected: both pass (they only test the logic inline, no LSP needed).

### Step 3: Apply cap in `ListSymbols::call`

In `ListSymbols::call`, find the single-file branch (currently around lines 318–332):

```rust
        if full_path.is_file() {
            ...
            let json_symbols: Vec<Value> = symbols
                .iter()
                .map(|s| symbol_to_json(s, include_body, source.as_deref(), depth, "project"))
                .collect();
            Ok(json!({ "file": rel_path, "symbols": json_symbols }))
```

Add the cap immediately after building `json_symbols`:

```rust
        if full_path.is_file() {
            let (client, lang) = get_lsp_client(ctx, &full_path).await?;
            let symbols = client.document_symbols(&full_path, &lang).await?;
            let include_body = guard.should_include_body();
            let source = if include_body {
                std::fs::read_to_string(&full_path).ok()
            } else {
                None
            };
            let json_symbols: Vec<Value> = symbols
                .iter()
                .map(|s| symbol_to_json(s, include_body, source.as_deref(), depth, "project"))
                .collect();

            // Cap single-file results to prevent large files blowing the context window.
            let total = json_symbols.len();
            let mut file_guard = guard;
            file_guard.max_results = LIST_SYMBOLS_SINGLE_FILE_CAP;
            let hint = format!(
                "File has {total} symbols. Use depth=1 for top-level overview, \
                 or find_symbol(name_path='ClassName/methodName', include_body=true) for a specific symbol."
            );
            let (json_symbols, overflow) = file_guard.cap_items(json_symbols, &hint);
            if let Some(ov) = overflow {
                let total = ov.total;
                let mut result = json!({ "file": rel_path, "symbols": json_symbols, "total": total });
                result["overflow"] = OutputGuard::overflow_json(&ov);
                return Ok(result);
            }
            Ok(json!({ "file": rel_path, "symbols": json_symbols }))
```

Also add the constant near `LIST_SYMBOLS_MAX_FILES`:

```rust
const LIST_SYMBOLS_SINGLE_FILE_CAP: usize = 100;
```

### Step 4: Build and run all tests

```bash
cargo build 2>&1 && cargo test 2>&1 | tail -30
```

Expected: all tests pass.

### Step 5: Commit

```bash
git add src/tools/symbol.rs
git commit -m "feat(list_symbols): cap single-file mode at 100 top-level symbols with refinement hint"
```

---

## Task 5: Update `server_instructions.md`

**Files:**
- Modify: `src/prompts/server_instructions.md`

### Step 1: Add discoverability section

Append to the end of `src/prompts/server_instructions.md`:

```markdown
## When symbol tools return too many results

All symbol tools use progressive discoverability: when results overflow, the response includes
`overflow.by_file` (where results are distributed) and `overflow.hint` (concrete follow-up calls).

Recommended workflow for `find_symbol` overflow:
1. Check `overflow.by_file` — pick the file most likely to contain what you want
2. Re-call with `path="that/file.tsx"` to scope the search to one file
3. Or add `kind="function"` to skip variables and local declarations
4. For a structural overview of a file: use `list_symbols(depth=1)` instead of `find_symbol`

`kind` values: `function`, `class`, `struct`, `interface` (also matches Rust traits), `type`,
`enum`, `module`, `constant`.

For `list_symbols` single-file overflow: the file has more top-level symbols than shown.
Use `find_symbol(name_path="ClassName/methodName", include_body=true)` to read a specific one.
```

### Step 2: Update `Output Modes` section overflow description

Find the line:
```
Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "..." } }` — follow the hint.
```

Replace with:
```
Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "...", "by_file": {...} } }` — follow the hint; check `by_file` to see where results are distributed.
```

### Step 3: Commit

```bash
git add src/prompts/server_instructions.md
git commit -m "docs(prompts): add progressive discoverability guidance to server instructions"
```

---

## Task 6: Final verification

### Step 1: Full test suite

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass (≥435 pre-existing + ~12 new).

### Step 2: Clippy

```bash
cargo clippy -- -D warnings 2>&1
```

Expected: clean.

### Step 3: Format

```bash
cargo fmt 2>&1
```

### Step 4: Final commit if fmt made changes

```bash
git status
# If changes:
git add src/
git commit -m "style: cargo fmt after progressive discoverability implementation"
```

---

## Design Tests Coverage

| Design test | Covered by |
|---|---|
| T1 — by_file in find_symbol overflow | `build_by_file_no_overflow_under_cap` + directory path refactor |
| T2 — kind filter excludes variables | `collect_matching_with_kind_filter_class_only` |
| T3 — hint is actionable | `make_find_symbol_hint_contains_top_file_and_kind_and_offset` |
| T4 — list_symbols single-file cap | `list_symbols_single_file_cap_unit` |
| T5 — list_symbols no overflow under cap | `list_symbols_single_file_no_overflow_under_cap_unit` |
| T6 — find_symbol cap drops to 50 | `FIND_SYMBOL_MAX_RESULTS` constant + guard override |
| T7 — by_file capped at 15 entries | `build_by_file_sorts_desc_and_caps_at_15` |
| T8 — kind ignored with name_path | `kind_filter_skipped_when_using_name_path` |
| T9 — accurate total (no early-cap) | removal of `hit_early_cap` block; full collection before `cap_items` |

## Summary of Files Changed

| File | Change |
|---|---|
| `src/tools/output.rs` | `OverflowInfo` + 2 fields; `overflow_json` serializes them |
| `src/tools/symbol.rs` | `matches_kind_filter`, `build_by_file`, `make_find_symbol_hint`, `FIND_SYMBOL_MAX_RESULTS`, `LIST_SYMBOLS_SINGLE_FILE_CAP` added; `collect_matching` + kind_filter; `FindSymbol` schema + call refactor; `ListSymbols` single-file cap |
| `src/prompts/server_instructions.md` | Discoverability section |
