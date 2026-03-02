---
# "Show the Data" — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Update 9 hollow `format_for_user()` functions so humans watching Claude work see actual data, not just counts.

**Architecture:** All changes are in `src/tools/user_format.rs` (format functions) and `src/tools/usage.rs` (one new `format_for_user` method). The LLM-facing JSON (`call()`) is not touched for any tool. Tests live in the `#[cfg(test)]` section at the bottom of `user_format.rs`.

**Tech Stack:** Rust, `serde_json::Value`, existing ANSI color constants in `user_format.rs`

---

## Before Starting

Read `docs/plans/2026-03-01-show-the-data-design.md` for the design rationale.

Run the test suite to establish baseline:
```
cargo test 2>&1 | tail -5
```
Expected: all tests passing (533+).

---

### Task 1: `list_memories` — show topic names

**Files:**
- Modify: `src/tools/user_format.rs` — `format_list_memories` function

**Step 1: Write the failing test**

Find the `#[cfg(test)]` block in `user_format.rs` (near line 1190). Add inside the existing `tests` module:

```rust
#[test]
fn format_list_memories_shows_topic_names() {
    let result = json!({
        "topics": ["architecture", "conventions", "gotchas"]
    });
    let out = format_list_memories(&result);
    assert!(out.contains("architecture"), "should list topic names");
    assert!(out.contains("conventions"), "should list topic names");
    assert!(out.contains("gotchas"), "should list topic names");
    assert!(out.contains('3'), "should include count");
}

#[test]
fn format_list_memories_empty() {
    let result = json!({ "topics": [] });
    let out = format_list_memories(&result);
    assert!(out.contains('0'), "should say 0 topics");
}
```

**Step 2: Run to confirm it fails**

```
cargo test format_list_memories 2>&1 | tail -20
```
Expected: FAIL — the current output doesn't contain the topic names.

**Step 3: Replace `format_list_memories`**

Find the current function (around line 887) and replace with:

```rust
pub fn format_list_memories(result: &Value) -> String {
    let topics = match result["topics"].as_array() {
        Some(t) if !t.is_empty() => t,
        _ => return "0 topics".to_string(),
    };
    let mut out = format!("{} topics", topics.len());
    for topic in topics.iter() {
        if let Some(name) = topic.as_str() {
            out.push_str(&format!("\n  {name}"));
        }
    }
    out
}
```

**Step 4: Run tests**

```
cargo test format_list_memories 2>&1 | tail -10
```
Expected: PASS.

**Step 5: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): list_memories shows topic names"
```

---

### Task 2: `read_memory` — show content

**Files:**
- Modify: `src/tools/user_format.rs` — `format_read_memory` function

**Step 1: Write the failing test**

Add inside the `tests` module:

```rust
#[test]
fn format_read_memory_shows_content() {
    let result = json!({
        "topic": "architecture",
        "content": "## Layers\n\nAgent → Server → Tools"
    });
    let out = format_read_memory(&result);
    assert!(out.contains("architecture"), "should show topic");
    assert!(out.contains("Layers"), "should show content");
    assert!(out.contains("Agent → Server → Tools"), "should show full content");
}

#[test]
fn format_read_memory_not_found_unchanged() {
    let result = json!({ "topic": "missing", "content": null });
    let out = format_read_memory(&result);
    assert!(out.contains("not found"), "should say not found");
    assert!(out.contains("missing"), "should include topic name");
}
```

**Step 2: Run to confirm failure**

```
cargo test format_read_memory 2>&1 | tail -20
```

**Step 3: Replace `format_read_memory`**

```rust
pub fn format_read_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    match result["content"].as_str() {
        None => format!("not found · {topic}"),
        Some(content) => {
            let mut out = topic.to_string();
            for line in content.lines() {
                out.push_str(&format!("\n  {line}"));
            }
            out
        }
    }
}
```

**Step 4: Run tests**

```
cargo test format_read_memory 2>&1 | tail -10
```

**Step 5: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): read_memory shows content instead of char count"
```

---

### Task 3: `list_libraries` — show names and index status

**Files:**
- Modify: `src/tools/user_format.rs` — `format_list_libraries` function

**Step 1: Verify JSON structure**

Before writing the test, check the actual field names in the `list_libraries` JSON output. Run:

```
cargo test list_libraries -- --nocapture 2>&1 | head -50
```

Or look at the `ListLibraries::call` implementation to find field names:
```
find_symbol("ListLibraries", include_body=true)
```

The library objects should have `name` and `indexed` fields. Confirm this before writing tests.

**Step 2: Write the failing test**

```rust
#[test]
fn format_list_libraries_shows_names_and_status() {
    let result = json!({
        "libraries": [
            {"name": "serde", "indexed": true},
            {"name": "tokio", "indexed": false}
        ]
    });
    let out = format_list_libraries(&result);
    assert!(out.contains("serde"), "should show library name");
    assert!(out.contains("tokio"), "should show library name");
    assert!(out.contains("indexed"), "should show index status");
    assert!(out.contains("not indexed") || out.contains("tokio"), "unindexed lib shown");
}
```

**Step 3: Run to confirm failure**

```
cargo test format_list_libraries 2>&1 | tail -20
```

**Step 4: Replace `format_list_libraries`**

```rust
pub fn format_list_libraries(result: &Value) -> String {
    let libs = match result["libraries"].as_array() {
        Some(l) if !l.is_empty() => l,
        _ => return "0 libraries".to_string(),
    };
    let name_width = libs
        .iter()
        .filter_map(|l| l["name"].as_str())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("{} libraries", libs.len());
    for lib in libs.iter() {
        let name = lib["name"].as_str().unwrap_or("?");
        let status = if lib["indexed"].as_bool().unwrap_or(false) {
            "indexed"
        } else {
            "not indexed"
        };
        out.push_str(&format!("\n  {name:<name_width$}  {status}"));
    }
    out
}
```

**Step 5: Run tests**

```
cargo test format_list_libraries 2>&1 | tail -10
```

**Step 6: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): list_libraries shows names and index status"
```

---

### Task 4: `find_references` — show first 5 locations

**Files:**
- Modify: `src/tools/user_format.rs` — `format_find_references` function

**Step 1: Verify JSON structure**

Check what fields the reference objects have. Use:
```
find_symbol("FindReferences", include_body=true)
```
Look for the `call()` method. References should have `file` and `line` fields at minimum.

**Step 2: Write the failing test**

```rust
#[test]
fn format_find_references_shows_locations() {
    let result = json!({
        "total": 8,
        "references": [
            {"file": "src/tools/symbol.rs", "line": 142},
            {"file": "src/tools/symbol.rs", "line": 198},
            {"file": "src/server.rs", "line": 87},
            {"file": "src/agent.rs", "line": 210},
            {"file": "src/main.rs", "line": 45},
            {"file": "src/config.rs", "line": 12}
        ]
    });
    let out = format_find_references(&result);
    assert!(out.contains("8 refs"), "should show total");
    assert!(out.contains("src/tools/symbol.rs:142"), "should show locations");
    assert!(out.contains("src/server.rs:87"), "should show locations");
    assert!(out.contains("+3 more") || out.contains("more"), "should show trailer for hidden refs");
    // Should not show 6th ref since cap is 5
    assert!(!out.contains("src/config.rs"), "should cap at 5");
}

#[test]
fn format_find_references_five_or_fewer_no_trailer() {
    let result = json!({
        "total": 3,
        "references": [
            {"file": "src/a.rs", "line": 1},
            {"file": "src/b.rs", "line": 2},
            {"file": "src/c.rs", "line": 3}
        ]
    });
    let out = format_find_references(&result);
    assert!(out.contains("src/a.rs:1"));
    assert!(!out.contains("more"), "no trailer when all fit");
}
```

**Step 3: Run to confirm failure**

```
cargo test format_find_references 2>&1 | tail -20
```

**Step 4: Replace `format_find_references`**

```rust
pub fn format_find_references(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or_else(|| {
        result["references"]
            .as_array()
            .map(|a| a.len() as u64)
            .unwrap_or(0)
    });

    if total == 0 {
        return "No references found.".to_string();
    }

    let refs = match result["references"].as_array() {
        Some(r) => r,
        None => return format!("{total} refs"),
    };

    const MAX_SHOW: usize = 5;
    let mut out = format!("{total} refs");
    for r in refs.iter().take(MAX_SHOW) {
        let file = r["file"].as_str().unwrap_or("?");
        let line = r["line"].as_u64().unwrap_or(0);
        out.push_str(&format!("\n  {file}:{line}"));
    }
    let shown = refs.len().min(MAX_SHOW);
    let hidden = (total as usize).saturating_sub(shown);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more"));
    }
    out
}
```

**Step 5: Run tests**

```
cargo test format_find_references 2>&1 | tail -10
```

**Step 6: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): find_references shows first 5 locations"
```

---

### Task 5: `list_functions` — show function names

**Files:**
- Modify: `src/tools/user_format.rs` — `format_list_functions` function

**Step 1: Verify JSON structure**

Check `ListFunctions::call()` for the exact shape of function objects:
```
find_symbol("ListFunctions", include_body=true)
```
Look for what fields each function entry has. Expect `name`, `start_line`, `end_line`.

**Step 2: Write the failing test**

```rust
#[test]
fn format_list_functions_shows_names() {
    let result = json!({
        "file": "src/tools/symbol.rs",
        "functions": [
            {"name": "collect_matching", "start_line": 100, "end_line": 140},
            {"name": "build_by_file", "start_line": 150, "end_line": 180},
            {"name": "matches_kind_filter", "start_line": 190, "end_line": 200}
        ]
    });
    let out = format_list_functions(&result);
    assert!(out.contains("src/tools/symbol.rs"), "should show file");
    assert!(out.contains("collect_matching"), "should show function name");
    assert!(out.contains("build_by_file"), "should show function name");
    assert!(out.contains('3'), "should show count");
}

#[test]
fn format_list_functions_caps_at_eight() {
    let funcs: Vec<serde_json::Value> = (0..12)
        .map(|i| json!({"name": format!("func_{i}"), "start_line": i, "end_line": i + 5}))
        .collect();
    let result = json!({ "file": "src/big.rs", "functions": funcs });
    let out = format_list_functions(&result);
    assert!(out.contains("func_0"), "should show first func");
    assert!(!out.contains("func_8"), "should not show 9th func");
    assert!(out.contains("+4 more") || out.contains("more"), "should show trailer");
}
```

**Step 3: Run to confirm failure**

```
cargo test format_list_functions 2>&1 | tail -20
```

**Step 4: Replace `format_list_functions`**

```rust
pub fn format_list_functions(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let funcs = match result["functions"].as_array() {
        Some(f) if !f.is_empty() => f,
        _ => return format!("{file} — 0 functions"),
    };
    const MAX_SHOW: usize = 8;
    let total = funcs.len();
    let mut out = format!("{file} — {total} functions");
    for f in funcs.iter().take(MAX_SHOW) {
        let name = f["name"].as_str().unwrap_or("?");
        out.push_str(&format!("\n  {name}"));
    }
    let hidden = total.saturating_sub(MAX_SHOW);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more"));
    }
    out
}
```

**Step 5: Run tests**

```
cargo test format_list_functions 2>&1 | tail -10
```

**Step 6: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): list_functions shows function names"
```

---

### Task 6: `list_docs` — show docstring previews

**Files:**
- Modify: `src/tools/user_format.rs` — `format_list_docs` function

**Step 1: Verify JSON structure**

Check `ListDocs::call()` for what fields docstring entries have:
```
find_symbol("ListDocs", include_body=true)
```
Expect fields like `symbol` (function/struct name) and `doc` (the docstring text).

**Step 2: Write the failing test**

```rust
#[test]
fn format_list_docs_shows_previews() {
    let result = json!({
        "file": "src/tools/output.rs",
        "docstrings": [
            {"symbol": "OutputGuard", "doc": "Enforces progressive disclosure across all tools."},
            {"symbol": "cap_items", "doc": "Truncate to exploring-mode limit and produce OverflowInfo."},
            {"symbol": "cap_files", "doc": "File-level capping for multi-file result sets."},
            {"symbol": "overflow_json", "doc": "Build the overflow object to include in JSON response."}
        ]
    });
    let out = format_list_docs(&result);
    assert!(out.contains("src/tools/output.rs"), "should show file");
    assert!(out.contains("OutputGuard"), "should show symbol name");
    assert!(out.contains("Enforces progressive"), "should show doc preview");
    assert!(out.contains("+1 more") || out.contains("more"), "should cap at 3");
    assert!(!out.contains("overflow_json"), "4th entry should be hidden");
}
```

**Step 3: Run to confirm failure**

```
cargo test format_list_docs 2>&1 | tail -20
```

**Step 4: Replace `format_list_docs`**

```rust
pub fn format_list_docs(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let docs = match result["docstrings"].as_array() {
        Some(d) if !d.is_empty() => d,
        _ => return format!("{file} — 0 docstrings"),
    };
    const MAX_SHOW: usize = 3;
    let total = docs.len();
    let mut out = format!("{file} — {total} docstrings");
    for entry in docs.iter().take(MAX_SHOW) {
        let symbol = entry["symbol"].as_str().unwrap_or("?");
        let doc = entry["doc"].as_str().unwrap_or("");
        let first_line = doc.lines().next().unwrap_or("").trim();
        let preview = if first_line.len() > 72 {
            format!("{}…", &first_line[..72])
        } else {
            first_line.to_string()
        };
        out.push_str(&format!("\n  {symbol}  {preview}"));
    }
    let hidden = total.saturating_sub(MAX_SHOW);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more"));
    }
    out
}
```

**Step 5: Run tests**

```
cargo test format_list_docs 2>&1 | tail -10
```

**Step 6: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): list_docs shows docstring symbol names and previews"
```

---

### Task 7: `git_blame` — add author breakdown

**Files:**
- Modify: `src/tools/user_format.rs` — `format_git_blame` function

**Step 1: Write the failing test**

```rust
#[test]
fn format_git_blame_shows_author_breakdown() {
    let lines: Vec<serde_json::Value> = vec![
        json!({"author": "alice", "line": 1}),
        json!({"author": "alice", "line": 2}),
        json!({"author": "alice", "line": 3}),
        json!({"author": "bob", "line": 4}),
        json!({"author": "bob", "line": 5}),
        json!({"author": "carol", "line": 6}),
    ];
    let result = json!({ "file": "src/main.rs", "lines": lines });
    let out = format_git_blame(&result);
    assert!(out.contains("src/main.rs"), "should show file");
    assert!(out.contains("alice"), "should show author");
    assert!(out.contains("bob"), "should show author");
    assert!(out.contains("carol"), "should show author");
    assert!(out.contains('3'), "alice has 3 lines");
    assert!(out.contains('2'), "bob has 2 lines");
}

#[test]
fn format_git_blame_single_author_no_breakdown() {
    let lines: Vec<serde_json::Value> = (0..5)
        .map(|i| json!({"author": "solo", "line": i}))
        .collect();
    let result = json!({ "file": "src/lib.rs", "lines": lines });
    let out = format_git_blame(&result);
    assert!(out.contains("src/lib.rs"), "should show file");
    // single author: no need for breakdown table
    assert!(!out.contains('\n'), "no breakdown for single author");
}
```

**Step 2: Run to confirm failure**

```
cargo test format_git_blame 2>&1 | tail -20
```

**Step 3: Replace `format_git_blame`**

```rust
pub fn format_git_blame(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let lines = match result["lines"].as_array() {
        Some(l) => l,
        None => return file.to_string(),
    };
    let line_count = lines.len();

    let mut author_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for l in lines.iter() {
        if let Some(author) = l["author"].as_str() {
            *author_counts.entry(author).or_insert(0) += 1;
        }
    }

    if author_counts.len() <= 1 {
        let author_note = author_counts
            .keys()
            .next()
            .map(|a| format!(" · {a}"))
            .unwrap_or_default();
        return format!("{file} · {line_count} lines{author_note}");
    }

    let mut authors: Vec<(&str, usize)> = author_counts.into_iter().collect();
    authors.sort_by(|a, b| b.1.cmp(&a.1));

    let name_width = authors.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut out = format!("{file} · {line_count} lines");
    const MAX_AUTHORS: usize = 5;
    for (author, count) in authors.iter().take(MAX_AUTHORS) {
        let label = if *count == 1 { "line" } else { "lines" };
        out.push_str(&format!("\n  {author:<name_width$}  {count} {label}"));
    }
    let hidden = authors.len().saturating_sub(MAX_AUTHORS);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more authors"));
    }
    out
}
```

**Step 4: Run tests**

```
cargo test format_git_blame 2>&1 | tail -10
```

**Step 5: Commit**

```
git add src/tools/user_format.rs
git commit -m "feat(ui): git_blame shows author line-count breakdown"
```

---

### Task 8: `index_status` — add model and staleness detail

**Files:**
- Modify: `src/tools/user_format.rs` — `format_index_status` function

**Step 1: Check what fields `index_status` returns**

Run:
```
find_symbol("IndexStatus", include_body=true)
```
and
```
find_symbol("format_index_status", include_body=true)
```

Check if `model` and `last_indexed` / `indexed_at` fields are already returned in the JSON. If not, they need to be added to `IndexStatus::call()` in `src/tools/semantic.rs` as well (add that as a sub-step if needed).

**Step 2: Write the failing test**

```rust
#[test]
fn format_index_status_shows_model_and_timestamp() {
    let result = json!({
        "indexed": true,
        "file_count": 42,
        "chunk_count": 1234,
        "stale": false,
        "model": "text-embedding-3-small",
        "indexed_at": "2026-03-01 14:22"
    });
    let out = format_index_status(&result);
    assert!(out.contains("42 files"), "should show file count");
    assert!(out.contains("1234 chunks"), "should show chunk count");
    assert!(out.contains("text-embedding-3-small"), "should show model");
    assert!(out.contains("2026-03-01"), "should show timestamp");
}

#[test]
fn format_index_status_stale_shows_commit_count() {
    let result = json!({
        "indexed": true,
        "file_count": 10,
        "chunk_count": 100,
        "stale": true,
        "behind_commits": 5
    });
    let out = format_index_status(&result);
    assert!(out.contains("5 commits behind") || out.contains("stale"), "should note staleness");
}
```

**Step 3: Run to confirm failure**

```
cargo test format_index_status 2>&1 | tail -20
```

**Step 4: Replace `format_index_status`**

```rust
pub fn format_index_status(result: &Value) -> String {
    let indexed = result["indexed"].as_bool().unwrap_or(false);
    if !indexed {
        return "not indexed".to_string();
    }
    let files = result["file_count"].as_u64().unwrap_or(0);
    let chunks = result["chunk_count"].as_u64().unwrap_or(0);

    let mut out = format!("{files} files · {chunks} chunks");

    if let Some(model) = result["model"].as_str() {
        out.push_str(&format!(" · {model}"));
    }
    if let Some(ts) = result["indexed_at"].as_str() {
        out.push_str(&format!(" · {ts}"));
    }
    if result["stale"].as_bool().unwrap_or(false) {
        if let Some(behind) = result["behind_commits"].as_u64().filter(|&n| n > 0) {
            out.push_str(&format!(" · {behind} commits behind"));
        } else {
            out.push_str(" · stale");
        }
    }
    out
}
```

**Step 5: Run tests**

```
cargo test format_index_status 2>&1 | tail -10
```

**Step 6: If `model`/`indexed_at` fields are missing from the JSON**

Check `IndexStatus::call()` in `src/tools/semantic.rs`. If it doesn't include these fields, add them. Look for where the JSON response is constructed and add:
```rust
// inside the json!({...}) return value
"model": config.embedding_model,     // or similar field
"indexed_at": last_indexed_timestamp, // from index metadata
```
Then re-run tests.

**Step 7: Commit**

```
git add src/tools/user_format.rs src/tools/semantic.rs
git commit -m "feat(ui): index_status shows model name and staleness detail"
```

---

### Task 9: `get_usage_stats` — add `format_for_user`

**Files:**
- Modify: `src/tools/usage.rs` — add `format_for_user` method to `GetUsageStats`
- Modify: `src/tools/user_format.rs` — add `format_get_usage_stats` function

**Step 1: Inspect current `GetUsageStats`**

```
find_symbol("GetUsageStats", include_body=true)
```

Check: does it implement `format_for_user`? (It doesn't.) Also check what fields the JSON response has — expect `window`, `by_tool` array with entries containing `tool`, `calls`, `errors`, `p50_latency_ms`.

**Step 2: Write the failing test**

Add to the tests in `user_format.rs`:

```rust
#[test]
fn format_get_usage_stats_shows_per_tool_table() {
    let result = json!({
        "window": "1h",
        "by_tool": [
            {"tool": "find_symbol", "calls": 47, "errors": 0, "p50_latency_ms": 12},
            {"tool": "run_command", "calls": 18, "errors": 2, "p50_latency_ms": 340},
            {"tool": "list_symbols", "calls": 0, "errors": 0, "p50_latency_ms": 0}
        ]
    });
    let out = format_get_usage_stats(&result);
    assert!(out.contains("1h"), "should show window");
    assert!(out.contains("find_symbol"), "should show tool name");
    assert!(out.contains("47"), "should show call count");
    assert!(out.contains("run_command"), "should show tool with errors");
    assert!(!out.contains("list_symbols"), "should omit tools with 0 calls");
}

#[test]
fn format_get_usage_stats_no_calls() {
    let result = json!({
        "window": "1h",
        "by_tool": []
    });
    let out = format_get_usage_stats(&result);
    assert!(out.contains("no calls") || out.contains("0"), "should handle empty");
}
```

**Step 3: Run to confirm failure** (function doesn't exist yet)

```
cargo test format_get_usage_stats 2>&1 | tail -20
```

**Step 4: Add `format_get_usage_stats` to `user_format.rs`**

Add near the other config-related format functions:

```rust
pub fn format_get_usage_stats(result: &Value) -> String {
    let window = result["window"].as_str().unwrap_or("?");
    let by_tool = match result["by_tool"].as_array() {
        Some(t) => t,
        None => return format!("usage · {window}"),
    };

    let mut tools: Vec<&Value> = by_tool
        .iter()
        .filter(|t| t["calls"].as_u64().unwrap_or(0) > 0)
        .collect();
    tools.sort_by(|a, b| {
        b["calls"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["calls"].as_u64().unwrap_or(0))
    });

    if tools.is_empty() {
        return format!("usage · {window} · no calls");
    }

    let name_width = tools
        .iter()
        .filter_map(|t| t["tool"].as_str())
        .map(|n| n.len())
        .max()
        .unwrap_or(4)
        .max(4);

    const MAX_TOOLS: usize = 10;
    let mut out = format!("usage · {window}\n");
    out.push_str(&format!(
        "\n  {:<name_width$}  {:>5}  {:>6}  {:>6}",
        "tool", "calls", "errors", "p50ms"
    ));
    out.push_str(&format!("\n  {}", "─".repeat(name_width + 22)));

    for tool in tools.iter().take(MAX_TOOLS) {
        let name = tool["tool"].as_str().unwrap_or("?");
        let calls = tool["calls"].as_u64().unwrap_or(0);
        let errors = tool["errors"].as_u64().unwrap_or(0);
        let p50 = tool["p50_latency_ms"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "\n  {name:<name_width$}  {calls:>5}  {errors:>6}  {p50:>6}"
        ));
    }

    let hidden = tools.len().saturating_sub(MAX_TOOLS);
    if hidden > 0 {
        out.push_str(&format!("\n\n  … +{hidden} more tools"));
    }
    out
}
```

**Step 5: Add `format_for_user` to `GetUsageStats` in `usage.rs`**

Find `impl Tool for GetUsageStats` and add:

```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_get_usage_stats(result))
}
```

Make sure `user_format` is imported at the top of `usage.rs`:
```rust
use crate::tools::user_format;
```

**Step 6: Run tests**

```
cargo test format_get_usage_stats 2>&1 | tail -10
```

**Step 7: Commit**

```
git add src/tools/user_format.rs src/tools/usage.rs
git commit -m "feat(ui): get_usage_stats shows formatted per-tool table"
```

---

### Task 10: Update `docs/PROGRESSIVE_DISCOVERABILITY.md`

**Files:**
- Modify: `docs/PROGRESSIVE_DISCOVERABILITY.md` — add "Human-Facing Output" section

**Step 1: Find the right place in the doc**

Read the file and identify where to insert (after "Anti-Patterns", before "How Claude Code Processes Tool Output" or at the end before "Checklist for New Tools").

**Step 2: Add the new section**

Insert before the "Checklist for New Tools" section:

```markdown
## Human-Facing Output

The `format_for_user()` method produces output shown to the human watching Claude work in
the Claude Code terminal. This is separate from the JSON returned to the LLM.

**The rule:** If a tool fetches data, its `format_for_user()` must show at least a compact
preview of that data — not just a count.

**Why:** Counts are metadata. The human cannot tell from `"7 topics"` whether Claude found
the right topic, or from `"12 refs"` where those references are. Showing data lets the
human verify Claude is on the right track without inspecting the LLM's full conversation
context.

**What "compact preview" means:**
- Collections (topics, libraries, references): first 5–8 items, then `… +N more`
- Memory content: full content (it's already fetched; hiding it helps nobody)
- Stats tables: cap at 10 rows, sort by most-relevant metric descending
- Author breakdowns: cap at 5 entries sorted by line count descending

**Anti-pattern:** Count-only output when data is available.
```
Bad: `"7 topics"` — count with no names
Bad: `"12 refs"` — count with no locations
Good: `"7 topics\n  architecture\n  conventions\n  …"`
```
```

**Step 3: Add checklist item**

Find the "Checklist for New Tools" section and add:

```markdown
- [ ] If the tool fetches data, does `format_for_user()` preview that data (not just count it)?
- [ ] Is the preview capped (5–8 items) to avoid verbosity?
- [ ] Is there a `… +N more` trailer when items are omitted?
```

**Step 4: Commit**

```
git add docs/PROGRESSIVE_DISCOVERABILITY.md
git commit -m "docs: add human-facing output rule to PROGRESSIVE_DISCOVERABILITY"
```

---

### Task 11: Final verification

**Step 1: Run full test suite**

```
cargo test 2>&1 | tail -10
```
Expected: all tests pass (new tests included).

**Step 2: Run clippy**

```
cargo clippy -- -D warnings 2>&1 | tail -20
```
Expected: no warnings.

**Step 3: Run fmt**

```
cargo fmt 2>&1
cargo fmt --check 2>&1
```
Expected: no changes needed.

**Step 4: Final commit if any fmt fixes needed**

```
git add -p
git commit -m "style: cargo fmt after show-the-data changes"
```

---

## Summary

| Task | File(s) | Change |
|------|---------|--------|
| 1 `list_memories` | `user_format.rs` | List topic names |
| 2 `read_memory` | `user_format.rs` | Show content inline |
| 3 `list_libraries` | `user_format.rs` | Names + indexed status |
| 4 `find_references` | `user_format.rs` | First 5 locations + trailer |
| 5 `list_functions` | `user_format.rs` | First 8 names + trailer |
| 6 `list_docs` | `user_format.rs` | First 3 symbol+preview + trailer |
| 7 `git_blame` | `user_format.rs` | Author breakdown table |
| 8 `index_status` | `user_format.rs` | Model + timestamp + commit-behind |
| 9 `get_usage_stats` | `user_format.rs` + `usage.rs` | Add format_for_user + table |
| 10 Docs | `PROGRESSIVE_DISCOVERABILITY.md` | Human-facing output rule |
| 11 Verify | — | Tests + clippy + fmt |
