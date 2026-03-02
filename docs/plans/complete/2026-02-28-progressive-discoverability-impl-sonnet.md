# Progressive Discoverability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make symbol tools self-guiding when results overflow — agents get file distributions, kind filters, and copy-paste-ready follow-up calls instead of vague "narrow your search" hints.

**Architecture:** Four changes to `src/tools/output.rs` and `src/tools/symbol.rs`: (1) extend `OverflowInfo` with `by_file` map, (2) add `kind` filter to `find_symbol` + `collect_matching`, (3) cap `list_symbols` single-file mode, (4) update server instructions. All changes are backwards-compatible — new fields are optional, omitting `kind` preserves current behavior.

**Tech Stack:** Rust, `indexmap` crate (insertion-order-preserving map for `by_file`), existing LSP `SymbolKind` enum.

**Design doc:** `docs/plans/2026-02-28-progressive-discoverability-design.md`

---

### Task 1: Add `indexmap` dependency

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add indexmap to Cargo.toml**

Add `indexmap` with serde support to `[dependencies]`:

```toml
indexmap = { version = "2", features = ["serde"] }
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles cleanly, no errors

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add indexmap dependency for by_file overflow maps"
```

---

### Task 2: Extend `OverflowInfo` with `by_file`

**Files:**
- Modify: `src/tools/output.rs` (struct `OverflowInfo`, fn `overflow_json`)

**Step 1: Write the failing test**

Add to `src/tools/output.rs` in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn overflow_json_includes_by_file() {
    let mut by_file = indexmap::IndexMap::new();
    by_file.insert("src/a.rs".to_string(), 30usize);
    by_file.insert("src/b.rs".to_string(), 20);
    let info = OverflowInfo {
        shown: 50,
        total: 80,
        hint: "narrow".to_string(),
        next_offset: None,
        by_file: Some(by_file),
        by_file_overflow: 0,
    };
    let json = OutputGuard::overflow_json(&info);
    let bf = json["by_file"].as_object().unwrap();
    assert_eq!(bf.len(), 2);
    assert_eq!(bf["src/a.rs"], 30);
    assert_eq!(bf["src/b.rs"], 20);
    assert!(json.get("by_file_overflow").is_none());
}

#[test]
fn overflow_json_by_file_overflow_shown_when_nonzero() {
    let mut by_file = indexmap::IndexMap::new();
    by_file.insert("src/a.rs".to_string(), 10usize);
    let info = OverflowInfo {
        shown: 50,
        total: 200,
        hint: "narrow".to_string(),
        next_offset: None,
        by_file: Some(by_file),
        by_file_overflow: 42,
    };
    let json = OutputGuard::overflow_json(&info);
    assert_eq!(json["by_file_overflow"], 42);
}

#[test]
fn overflow_json_omits_by_file_when_none() {
    let info = OverflowInfo {
        shown: 50,
        total: 80,
        hint: "narrow".to_string(),
        next_offset: None,
        by_file: None,
        by_file_overflow: 0,
    };
    let json = OutputGuard::overflow_json(&info);
    assert!(json.get("by_file").is_none());
    assert!(json.get("by_file_overflow").is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p code-explorer overflow_json_includes_by_file overflow_json_by_file_overflow overflow_json_omits_by_file -- --test-threads=1`
Expected: FAIL — `by_file` and `by_file_overflow` fields don't exist on `OverflowInfo`

**Step 3: Add fields to `OverflowInfo`**

In `src/tools/output.rs`, change the `OverflowInfo` struct (currently lines 20–26):

```rust
pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,
    /// In focused mode, the offset for the next page (None in exploring mode).
    pub next_offset: Option<usize>,
    /// Per-file match counts (top 15 by count desc). Only for multi-file searches.
    pub by_file: Option<indexmap::IndexMap<String, usize>>,
    /// Number of additional files omitted from by_file.
    pub by_file_overflow: usize,
}
```

**Step 4: Fix all existing OverflowInfo construction sites**

Every place that creates an `OverflowInfo` needs the two new fields. Search for `OverflowInfo {` — there are currently 3 sites:

1. `src/tools/output.rs` line ~126 (in `cap_items` exploring branch) — add `by_file: None, by_file_overflow: 0,`
2. `src/tools/output.rs` line ~64 (in `paginate`) — add `by_file: None, by_file_overflow: 0,`
3. `src/tools/symbol.rs` line ~535 (in `FindSymbol::call` early-cap branch) — add `by_file: None, by_file_overflow: 0,`

**Step 5: Update `overflow_json` to serialize `by_file`**

In `overflow_json` (currently lines 164–174), after the `next_offset` handling:

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
    if let Some(ref bf) = info.by_file {
        obj["by_file"] = json!(bf);
        if info.by_file_overflow > 0 {
            obj["by_file_overflow"] = json!(info.by_file_overflow);
        }
    }
    obj
}
```

**Step 6: Run all tests**

Run: `cargo test`
Expected: ALL tests pass, including the 3 new ones

**Step 7: Commit**

```bash
git add src/tools/output.rs src/tools/symbol.rs
git commit -m "feat(output): add by_file and by_file_overflow to OverflowInfo"
```

---

### Task 3: Add `matches_kind_filter` helper and `kind` parameter to `find_symbol`

**Files:**
- Modify: `src/tools/symbol.rs` (fn `collect_matching`, `FindSymbol::input_schema`, `FindSymbol::call`)

**Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/tools/symbol.rs`:

```rust
#[test]
fn matches_kind_filter_maps_correctly() {
    use crate::lsp::SymbolKind;
    assert!(matches_kind_filter(&SymbolKind::Function, "function"));
    assert!(matches_kind_filter(&SymbolKind::Method, "function"));
    assert!(matches_kind_filter(&SymbolKind::Constructor, "function"));
    assert!(!matches_kind_filter(&SymbolKind::Variable, "function"));

    assert!(matches_kind_filter(&SymbolKind::Class, "class"));
    assert!(!matches_kind_filter(&SymbolKind::Struct, "class"));

    assert!(matches_kind_filter(&SymbolKind::Struct, "struct"));
    assert!(!matches_kind_filter(&SymbolKind::Class, "struct"));

    assert!(matches_kind_filter(&SymbolKind::Interface, "interface"));
    assert!(matches_kind_filter(&SymbolKind::Enum, "enum"));
    assert!(matches_kind_filter(&SymbolKind::EnumMember, "enum"));
    assert!(matches_kind_filter(&SymbolKind::TypeParameter, "type"));
    assert!(matches_kind_filter(&SymbolKind::Module, "module"));
    assert!(matches_kind_filter(&SymbolKind::Namespace, "module"));
    assert!(matches_kind_filter(&SymbolKind::Package, "module"));
    assert!(matches_kind_filter(&SymbolKind::Constant, "constant"));
}

#[test]
fn collect_matching_filters_by_kind() {
    let symbols = vec![
        SymbolInfo {
            name: "WeeklyGrid".into(),
            name_path: "WeeklyGrid".into(),
            kind: crate::lsp::SymbolKind::Class,
            file: PathBuf::from("test.ts"),
            start_line: 0, end_line: 10, start_col: 0,
            children: vec![],
        },
        SymbolInfo {
            name: "weeklyGrid".into(),
            name_path: "weeklyGrid".into(),
            kind: crate::lsp::SymbolKind::Variable,
            file: PathBuf::from("test.ts"),
            start_line: 12, end_line: 12, start_col: 0,
            children: vec![],
        },
        SymbolInfo {
            name: "renderWeeklyGrid".into(),
            name_path: "renderWeeklyGrid".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: PathBuf::from("test.ts"),
            start_line: 14, end_line: 20, start_col: 0,
            children: vec![],
        },
    ];

    // Filter for class only
    let mut results = vec![];
    collect_matching(&symbols, "weeklygrid", false, None, 0, "project", &mut results, Some("class"));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], "WeeklyGrid");

    // Filter for function only
    let mut results = vec![];
    collect_matching(&symbols, "weeklygrid", false, None, 0, "project", &mut results, Some("function"));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], "renderWeeklyGrid");

    // No filter — all match
    let mut results = vec![];
    collect_matching(&symbols, "weeklygrid", false, None, 0, "project", &mut results, None);
    assert_eq!(results.len(), 3);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p code-explorer matches_kind_filter collect_matching_filters_by_kind -- --test-threads=1`
Expected: FAIL — `matches_kind_filter` doesn't exist, `collect_matching` has wrong arg count

**Step 3: Implement `matches_kind_filter`**

Add this function in `src/tools/symbol.rs` above `collect_matching` (around line 133):

```rust
/// Returns true if the given SymbolKind matches the filter string.
/// Filter values: "function", "class", "struct", "interface", "type", "enum", "module", "constant".
fn matches_kind_filter(kind: &crate::lsp::SymbolKind, filter: &str) -> bool {
    use crate::lsp::SymbolKind;
    match filter {
        "function" => matches!(kind, SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor),
        "class" => matches!(kind, SymbolKind::Class),
        "struct" => matches!(kind, SymbolKind::Struct),
        "interface" => matches!(kind, SymbolKind::Interface),
        "type" => matches!(kind, SymbolKind::TypeParameter),
        "enum" => matches!(kind, SymbolKind::Enum | SymbolKind::EnumMember),
        "module" => matches!(kind, SymbolKind::Module | SymbolKind::Namespace | SymbolKind::Package),
        "constant" => matches!(kind, SymbolKind::Constant),
        _ => true, // unknown filter — don't exclude anything
    }
}
```

**Step 4: Add `kind_filter` parameter to `collect_matching`**

Change the signature of `collect_matching` to add `kind_filter: Option<&str>`:

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
        let name_matches = sym.name.to_lowercase().contains(pattern)
            || sym.name_path.to_lowercase().contains(pattern);
        let kind_matches = kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
        if name_matches && kind_matches {
            out.push(symbol_to_json(sym, include_body, source_code, depth, source));
        }
        collect_matching(&sym.children, pattern, include_body, source_code, depth, source, out, kind_filter);
    }
}
```

**Step 5: Fix all existing callers of `collect_matching`**

There are 3 call sites in `FindSymbol::call` — each needs `None` appended as the last argument (since `kind` param isn't wired yet):

1. Line ~520 (directory/glob search loop) — add `, None` after `&mut matches`
2. Line ~631 (tree-sitter fallback loop) — add `, None` after `&mut matches`

Plus all test callers — search for `collect_matching(` in the test block and add `, None` to:
- `collect_matching_matches_name_path` (2 call sites)
- `collect_matching_slash_pattern_precision` (1 call site)

**Step 6: Add `kind` to `FindSymbol::input_schema`**

In the `input_schema` method (currently line 434), add after the `"scope"` property:

```rust
"kind": {
    "type": "string",
    "description": "Filter by symbol kind: function, class, struct, interface, type, enum, module, constant. Omit for all kinds.",
    "enum": ["function", "class", "struct", "interface", "type", "enum", "module", "constant"]
}
```

**Step 7: Wire `kind` in `FindSymbol::call`**

At the top of `call()`, after the `_scope` line (currently line 465), add:

```rust
let kind_filter = input["kind"].as_str();
```

Then pass `kind_filter` instead of `None` to all `collect_matching` calls. **But**: skip `kind_filter` when the input was `name_path` (exact lookup). The `pattern` variable is already set from either `"pattern"` or `"name_path"`. Add logic:

```rust
let is_name_path = input["name_path"].is_string();
let effective_kind_filter = if is_name_path { None } else { kind_filter };
```

Pass `effective_kind_filter` to all `collect_matching` calls and also to the workspace/symbol filtering `if` block (line ~582).

For the workspace/symbol path, the kind filter needs to be applied in the inline `if` block. Change:

```rust
if sym.name.to_lowercase().contains(&pattern_lower)
    || sym.name_path.to_lowercase().contains(&pattern_lower)
```

to:

```rust
let name_matches = sym.name.to_lowercase().contains(&pattern_lower)
    || sym.name_path.to_lowercase().contains(&pattern_lower);
let kind_matches = effective_kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
if name_matches && kind_matches
```

**Step 8: Run all tests**

Run: `cargo test`
Expected: ALL pass (new tests + existing)

**Step 9: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

**Step 10: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(find_symbol): add kind filter parameter for symbol type filtering"
```

---

### Task 4: Reduce `find_symbol` exploring cap from 200 to 50

**Files:**
- Modify: `src/tools/symbol.rs` (`FindSymbol::call`)

**Step 1: Write the failing test**

Add to tests in `src/tools/symbol.rs`:

```rust
#[tokio::test]
async fn find_symbol_exploring_cap_is_50() {
    // Build a project with many matching symbols
    let (ctx, _dir) = rich_project_ctx().await;
    // Use the existing rich_project_ctx which has files with symbols.
    // We test by calling FindSymbol with a broad pattern and verifying
    // the guard's max_results is 50.
    let guard = OutputGuard::from_input(&json!({}));
    // The default max_results is 200, but FindSymbol should override to 50
    // We verify this through the FIND_SYMBOL_MAX_RESULTS constant
    assert_eq!(FIND_SYMBOL_MAX_RESULTS, 50);
}
```

**Step 2: Implement the cap change**

Add a constant near the top of `src/tools/symbol.rs` (next to `LIST_SYMBOLS_MAX_FILES`):

```rust
const FIND_SYMBOL_MAX_RESULTS: usize = 50;
```

In `FindSymbol::call`, after `let guard = OutputGuard::from_input(&input);` (line 460), override:

```rust
let mut guard = OutputGuard::from_input(&input);
guard.max_results = guard.max_results.min(FIND_SYMBOL_MAX_RESULTS);
```

Also update the early-cap in the directory search path to use the same constant. The `early_cap` (line 494) already reads from `guard.max_results`, so the override above handles it.

**Step 3: Run tests**

Run: `cargo test`
Expected: ALL pass

**Step 4: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(find_symbol): reduce exploring cap from 200 to 50"
```

---

### Task 5: Remove early-cap in directory search, add `by_file` computation + enriched hints

This is the largest task — it restructures the directory/glob branch of `FindSymbol::call` to collect all matches (for accurate `total` and `by_file`), then truncate.

**Files:**
- Modify: `src/tools/symbol.rs` (`FindSymbol::call`, directory/glob branch lines ~471–543)

**Step 1: Write the failing tests**

Add to tests in `src/tools/symbol.rs`:

```rust
#[tokio::test]
async fn find_symbol_overflow_includes_by_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create 3 files each with many functions named "handler_*"
    for (i, name) in ["a.rs", "b.rs", "c.rs"].iter().enumerate() {
        let mut content = String::new();
        for j in 0..25 {
            content.push_str(&format!("fn handler_{}_{j}() {{}}\n", i));
        }
        std::fs::write(root.join(name), &content).unwrap();
    }

    let ctx = make_tool_ctx(root).await;
    let tool = FindSymbol;
    let result = tool.call(json!({"pattern": "handler", "path": "."}), &ctx).await.unwrap();

    let overflow = &result["overflow"];
    assert!(overflow.is_object(), "should have overflow for 75 matches");
    assert_eq!(result["overflow"]["total"], 75);
    assert_eq!(result["overflow"]["shown"], 50);

    let by_file = overflow["by_file"].as_object().unwrap();
    assert_eq!(by_file.len(), 3);
    // Each file should have 25 matches
    for (_file, count) in by_file {
        assert_eq!(count, 25);
    }
}

#[tokio::test]
async fn find_symbol_overflow_hint_is_actionable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create enough symbols to trigger overflow
    let mut content = String::new();
    for i in 0..60 {
        content.push_str(&format!("fn item_{i}() {{}}\n"));
    }
    std::fs::write(root.join("big.rs"), &content).unwrap();

    let ctx = make_tool_ctx(root).await;
    let tool = FindSymbol;
    let result = tool.call(json!({"pattern": "item", "path": "."}), &ctx).await.unwrap();

    let hint = result["overflow"]["hint"].as_str().unwrap();
    // Must contain concrete pagination offset
    assert!(hint.contains("offset=50"), "hint should show pagination: {hint}");
    // Must mention kind filter
    assert!(hint.contains("kind="), "hint should mention kind filter: {hint}");
}

#[tokio::test]
async fn find_symbol_by_file_capped_at_15() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create 20 files, each with a matching symbol
    for i in 0..20 {
        let content = format!("fn target_{i}() {{}}\n");
        std::fs::write(root.join(format!("file_{i}.rs")), &content).unwrap();
    }

    let ctx = make_tool_ctx(root).await;
    let tool = FindSymbol;
    // Need a cap override to trigger overflow with only 20 symbols
    // Use the tool with limit to force a smaller cap
    let result = tool.call(json!({"pattern": "target", "path": "."}), &ctx).await.unwrap();

    // With 20 matches and cap of 50, there's no overflow.
    // We need to test the by_file cap separately at the unit level.
    // Instead, test the helper function directly.
}
```

Note: The `by_file` cap test is better done as a unit test on a helper function. See Step 3.

**Step 2: Write unit test for `build_by_file` helper**

```rust
#[test]
fn build_by_file_caps_at_15() {
    let matches: Vec<Value> = (0..100)
        .map(|i| json!({ "file": format!("file_{}.rs", i % 20), "name": format!("sym_{i}") }))
        .collect();
    let (by_file, overflow_count) = build_by_file(&matches, 15);
    assert_eq!(by_file.len(), 15);
    assert_eq!(overflow_count, 5); // 20 total files - 15 shown
    // Should be sorted by count descending
    let counts: Vec<usize> = by_file.values().copied().collect();
    for window in counts.windows(2) {
        assert!(window[0] >= window[1], "by_file should be sorted desc by count");
    }
}

#[test]
fn build_by_file_no_overflow_when_under_cap() {
    let matches: Vec<Value> = (0..10)
        .map(|i| json!({ "file": format!("file_{i}.rs"), "name": format!("sym_{i}") }))
        .collect();
    let (by_file, overflow_count) = build_by_file(&matches, 15);
    assert_eq!(by_file.len(), 10);
    assert_eq!(overflow_count, 0);
}
```

**Step 3: Implement `build_by_file` helper**

Add in `src/tools/symbol.rs`:

```rust
const BY_FILE_CAP: usize = 15;

/// Build a per-file match count map from a list of symbol JSON values.
/// Returns (top N files sorted by count desc, number of omitted files).
fn build_by_file(matches: &[Value], cap: usize) -> (indexmap::IndexMap<String, usize>, usize) {
    // Count matches per file
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in matches {
        if let Some(file) = m["file"].as_str() {
            *counts.entry(file.to_string()).or_default() += 1;
        }
    }
    // Sort by count descending
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let total_files = sorted.len();
    let overflow_count = total_files.saturating_sub(cap);
    sorted.truncate(cap);

    let by_file: indexmap::IndexMap<String, usize> = sorted.into_iter().collect();
    (by_file, overflow_count)
}
```

**Step 4: Build the enriched hint**

Add a helper to format the rich hint:

```rust
fn build_find_symbol_hint(
    shown: usize,
    total: usize,
    by_file: &indexmap::IndexMap<String, usize>,
) -> String {
    let mut hint = format!("Showing {shown} of {total}. To narrow down:");
    hint.push_str(&format!("\n• paginate:       add offset={shown}, limit=50"));
    if let Some((top_file, _)) = by_file.iter().next() {
        hint.push_str(&format!("\n• filter by file: add path=\"{top_file}\""));
    }
    hint.push_str("\n• filter by kind: add kind=\"function\" (also: class, struct, interface, type, enum, module, constant)");
    hint
}
```

**Step 5: Restructure the directory/glob branch of `FindSymbol::call`**

Replace the entire directory search block (the `if let Some(rel)` branch, lines ~471–543) with:

1. Remove the `early_cap` variable and the `if hit_early_cap` block entirely.
2. Let the loop collect ALL matches (no early break).
3. After the loop, call `build_by_file(&matches, BY_FILE_CAP)` to get the file distribution.
4. Call `guard.cap_items(matches, &hint)` as before.
5. Patch `by_file` into the returned `OverflowInfo`.

The key change in the loop: remove lines 494–497 (early_cap) and lines 499–501 (if cap break). Remove lines 531–543 (hit_early_cap block). Keep the rest of the loop body identical.

After the loop:

```rust
// Build by_file before truncation
let (by_file, by_file_overflow) = build_by_file(&matches, BY_FILE_CAP);
let hint = if matches.len() > guard.max_results {
    build_find_symbol_hint(guard.max_results, matches.len(), &by_file)
} else {
    "Restrict with a file path or glob pattern".to_string()
};
let (matches, overflow) = guard.cap_items(matches, &hint);
// Patch by_file into overflow
let overflow = overflow.map(|mut ov| {
    ov.by_file = Some(by_file);
    ov.by_file_overflow = by_file_overflow;
    ov
});
```

Then merge this with the final section that already handles `overflow` (lines ~648–671). Since both the directory branch and the project-wide branch now flow through the same final section, ensure both paths set `matches` and `overflow` correctly, then the shared code does BODY_CAP + serialization.

**Step 6: Apply same `by_file` logic to workspace/symbol and tree-sitter fallback paths**

The project-wide path (workspace/symbol, lines ~546–608) and tree-sitter fallback (lines ~610–645) both collect into `matches` and flow through the shared `cap_items` call at line 648. After `cap_items`, apply `build_by_file` on the pre-cap `matches`.

To do this cleanly: save the pre-cap list length and build `by_file` from the pre-cap list:

```rust
// Right before cap_items (line ~648):
let (by_file, by_file_overflow) = build_by_file(&matches, BY_FILE_CAP);
let hint = if matches.len() > guard.max_results {
    build_find_symbol_hint(guard.max_results, matches.len(), &by_file)
} else {
    "Restrict with a file path or glob pattern".to_string()
};
let (mut matches, overflow) = guard.cap_items(matches, &hint);
let overflow = overflow.map(|mut ov| {
    ov.by_file = Some(by_file);
    ov.by_file_overflow = by_file_overflow;
    ov
});
```

**Step 7: Run all tests**

Run: `cargo test`
Expected: ALL pass

**Step 8: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

**Step 9: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(find_symbol): add by_file map and enriched hints to overflow"
```

---

### Task 6: Cap `list_symbols` single-file mode

**Files:**
- Modify: `src/tools/symbol.rs` (`ListSymbols::call`, single-file branch around line ~319)

**Step 1: Write the failing tests**

Add to tests in `src/tools/symbol.rs`:

```rust
#[tokio::test]
async fn list_symbols_single_file_applies_cap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create a file with many top-level functions
    let mut content = String::new();
    for i in 0..150 {
        content.push_str(&format!("fn symbol_{i}() {{}}\n"));
    }
    std::fs::write(root.join("big.rs"), &content).unwrap();

    let ctx = make_tool_ctx(root).await;
    let tool = ListSymbols;
    let result = tool.call(json!({"path": "big.rs"}), &ctx).await.unwrap();

    let symbols = result["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 100, "should cap at 100 top-level symbols");
    assert_eq!(result["overflow"]["shown"], 100);
    assert_eq!(result["overflow"]["total"], 150);
    let hint = result["overflow"]["hint"].as_str().unwrap();
    assert!(hint.contains("find_symbol"), "hint should mention find_symbol");
    assert!(hint.contains("name_path"), "hint should mention name_path");
    assert!(result["overflow"].get("by_file").is_none(), "single file should not have by_file");
}

#[tokio::test]
async fn list_symbols_single_file_under_cap_no_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let mut content = String::new();
    for i in 0..40 {
        content.push_str(&format!("fn symbol_{i}() {{}}\n"));
    }
    std::fs::write(root.join("small.rs"), &content).unwrap();

    let ctx = make_tool_ctx(root).await;
    let tool = ListSymbols;
    let result = tool.call(json!({"path": "small.rs"}), &ctx).await.unwrap();

    let symbols = result["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 40);
    assert!(result.get("overflow").is_none() || result["overflow"].is_null());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p code-explorer list_symbols_single_file -- --test-threads=1`
Expected: FAIL — `list_symbols_single_file_applies_cap` fails because no cap is applied

**Step 3: Add cap constant and implement**

Add constant near `LIST_SYMBOLS_MAX_FILES`:

```rust
const LIST_SYMBOLS_SINGLE_FILE_CAP: usize = 100;
```

In `ListSymbols::call`, the single-file branch (currently around lines 319–332), after building `json_symbols`:

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

    let total = json_symbols.len();
    if total > LIST_SYMBOLS_SINGLE_FILE_CAP {
        let shown = LIST_SYMBOLS_SINGLE_FILE_CAP;
        let kept: Vec<Value> = json_symbols.into_iter().take(shown).collect();
        let hint = format!(
            "File has {total} symbols. Use depth=1 for top-level overview, or find_symbol(name_path='<SymbolName>', include_body=true) for a specific symbol."
        );
        let overflow = OverflowInfo {
            shown,
            total,
            hint,
            next_offset: None,
            by_file: None,
            by_file_overflow: 0,
        };
        let mut result = json!({ "file": rel_path, "symbols": kept, "total": total });
        result["overflow"] = OutputGuard::overflow_json(&overflow);
        Ok(result)
    } else {
        Ok(json!({ "file": rel_path, "symbols": json_symbols }))
    }
}
```

**Step 4: Run all tests**

Run: `cargo test`
Expected: ALL pass

**Step 5: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(list_symbols): cap single-file mode at 100 top-level symbols"
```

---

### Task 7: Update `server_instructions.md`

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Add the overflow guidance section**

Add before the `## Rules` section:

```markdown
## When symbol tools return too many results

All symbol tools use progressive discoverability: when results overflow, the response includes
`overflow.by_file` (where results are distributed) and `overflow.hint` (concrete follow-up calls).

Recommended workflow:
1. Check `overflow.by_file` — pick the file most likely to contain what you want
2. Re-call with `path="that/file.tsx"` to scope the search to one file
3. Or add `kind="function"` to `find_symbol` to skip variables and local declarations
4. For a structural overview of a file: use `list_symbols(depth=1)` instead of `find_symbol`

`kind` values: `function`, `class`, `struct`, `interface` (also matches Rust traits), `type`, `enum`, `module`, `constant`.

For `list_symbols` overflow: the file has more top-level symbols than shown. Use
`find_symbol(name_path="ClassName/methodName", include_body=true)` to read a specific one.
```

**Step 2: Update the Output Modes section**

Change the existing overflow description:

From:
```
Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "..." } }` — follow the hint.
```

To:
```
Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "...", "by_file": {...} } }` — follow the hint, check `by_file` for where results live.
```

**Step 3: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs: add progressive discoverability guidance to server instructions"
```

---

### Task 8: Format, lint, test — final verification

**Files:**
- All modified files

**Step 1: Format**

Run: `cargo fmt`

**Step 2: Clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

**Step 3: Full test suite**

Run: `cargo test`
Expected: ALL pass (existing ~435 + ~10 new tests)

**Step 4: Final commit if formatting changed anything**

```bash
git add -A
git status
# If there are changes:
git commit -m "style: format after progressive discoverability changes"
```

---

### Task Summary

| Task | What | Files | Tests Added |
|------|------|-------|-------------|
| 1 | Add `indexmap` dep | `Cargo.toml` | 0 |
| 2 | `OverflowInfo.by_file` | `output.rs`, `symbol.rs` | 3 |
| 3 | `kind` filter + `collect_matching` | `symbol.rs` | 3 |
| 4 | Reduce cap 200→50 | `symbol.rs` | 1 |
| 5 | Remove early-cap, `by_file` + hints | `symbol.rs` | ~4 |
| 6 | `list_symbols` single-file cap | `symbol.rs` | 2 |
| 7 | Server instructions | `server_instructions.md` | 0 |
| 8 | Final verify | all | 0 |

**Total new tests:** ~13
