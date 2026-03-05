# Tool Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce from 32 → 23 tools by merging redundant tools, collapsing CRUD into dispatch, and renaming one tool whose scope grew substantially.

**Architecture:** Three phases: (1) additive — add new params/tool without removing anything; (2) removal — delete 9 now-redundant tools; (3) rename+enrich — `get_config` becomes `project_status` with index/usage/library data folded in. Always run `cargo test` after each task.

**Tech stack:** Rust, `serde_json::json!`, `async-trait`, `crate::ast::extract_docstrings`, `crate::embed::index`, `crate::usage::db`.

---

## Task 1: Add `include_docs` param to `list_symbols`

**Files:**
- Modify: `src/tools/symbol.rs` — `impl Tool for ListSymbols`
- Modify: `src/tools/ast.rs` — tests module (add one new test case)

**Step 1: Write the failing test**

In `src/tools/ast.rs` tests module, add after the existing `extract_docstrings_rust` test (around line 338):

```rust
#[tokio::test]
async fn list_symbols_include_docs_returns_docstrings() {
    let content = r#"
/// A documented function.
fn documented() {}

fn undocumented() {}
"#;
    let (dir, ctx) = project_ctx_with_file("test.rs", content).await;
    let tool = crate::tools::symbol::ListSymbols;
    let result = tool
        .call(
            json!({ "path": "test.rs", "include_docs": true }),
            &ctx,
        )
        .await
        .unwrap();
    let docstrings = result["docstrings"].as_array().expect("docstrings field missing");
    assert!(!docstrings.is_empty(), "expected at least one docstring");
    assert!(
        docstrings.iter().any(|d| d["symbol_name"].as_str().unwrap_or("").contains("documented")),
        "expected docstring for 'documented'"
    );
    drop(dir);
}
```

**Step 2: Run test to verify it fails**

```
cargo test list_symbols_include_docs --lib
```

Expected: FAIL — `"docstrings"` field is absent.

**Step 3: Add `include_docs` to `list_symbols` schema**

In `src/tools/symbol.rs`, find `impl Tool for ListSymbols` → `fn input_schema`. Add after the `scope` property:

```rust
"include_docs": {
    "type": "boolean",
    "default": false,
    "description": "When true, include docstrings for each file alongside symbols (tree-sitter). Replaces list_docs."
},
```

**Step 4: Add `include_docs` logic to `list_symbols` description**

Replace the `fn description` body:

```rust
fn description(&self) -> &str {
    "Return a tree of symbols (functions, classes, methods, etc.) in a file or directory. \
     Uses LSP for accurate results. Pass include_docs=true to also return docstrings \
     (replaces list_docs). Signatures are always included (replaces list_functions)."
}
```

**Step 5: Wire `include_docs` into `call()`**

At the top of `call()`, after `let guard = OutputGuard::from_input(&input);`, add:

```rust
let include_docs = input["include_docs"].as_bool().unwrap_or(false);
```

In the **single-file branch** (after `Ok(json!({ "file": rel_path, "symbols": json_symbols }))`), replace that final return with:

```rust
let mut result = json!({ "file": rel_path, "symbols": json_symbols });
if include_docs {
    if let Ok(docstrings) = crate::ast::extract_docstrings(&full_path) {
        let docs: Vec<Value> = docstrings
            .iter()
            .map(|d| json!({
                "symbol_name": d.symbol_name,
                "content": d.content,
                "start_line": d.start_line + 1,
                "end_line": d.end_line + 1,
            }))
            .collect();
        result["docstrings"] = json!(docs);
    }
}
Ok(result)
```

In the **overflow branch** (where result has both `"symbols"` and `"overflow"`), add the same `include_docs` block before the `return Ok(result)`.

In the **directory and glob branches**, for each per-file `json!({ "file": ..., "symbols": ... })` entry, add docstrings similarly when `include_docs` is true.

**Step 6: Run test to verify it passes**

```
cargo test list_symbols_include_docs --lib
```

Expected: PASS.

**Step 7: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

Expected: all tests pass, no warnings.

**Step 8: Commit**

```
git add src/tools/symbol.rs src/tools/ast.rs
git commit -m "feat(list_symbols): add include_docs param — replaces list_docs"
```

---

## Task 2: Add `scope` param to `index_project` (fold `index_library`)

**Files:**
- Modify: `src/tools/semantic.rs` — `impl Tool for IndexProject`
- Modify: `src/tools/library.rs` — add a test case

**Step 1: Write the failing test**

In `src/tools/library.rs` tests module, after `index_library_errors_for_unknown` (around line 248):

```rust
#[tokio::test]
async fn index_project_scope_lib_errors_for_unknown() {
    let (dir, ctx) = project_ctx().await;
    // Register nothing — querying an unknown lib name should return RecoverableError
    let tool = crate::tools::semantic::IndexProject;
    let result = tool
        .call(json!({ "scope": "lib:nonexistent" }), &ctx)
        .await;
    assert!(result.is_err(), "expected error for unknown library");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("nonexistent") || msg.contains("not found"),
        "error should mention the library name: {msg}"
    );
    drop(dir);
}
```

**Step 2: Run test to verify it fails**

```
cargo test index_project_scope_lib_errors_for_unknown --lib
```

Expected: FAIL — `index_project` ignores `scope` and succeeds (or panics differently).

**Step 3: Add `scope` to `index_project` schema**

In `src/tools/semantic.rs`, find `impl Tool for IndexProject` → `fn input_schema`. Add after the `force` property:

```rust
"scope": {
    "type": "string",
    "default": "project",
    "description": "Scope to index: 'project' (default) to index the active project, or 'lib:<name>' to index a registered library. Replaces index_library."
},
```

**Step 4: Update `index_project` description**

```rust
fn description(&self) -> &str {
    "Build or incrementally update the semantic search index for the active project. \
     Use scope='lib:<name>' to index a registered library (replaces index_library)."
}
```

**Step 5: Add scope dispatch to `call()`**

In `impl Tool for IndexProject`, at the top of `call()`, before the `let force = ...` line, add:

```rust
let scope_str = input["scope"].as_str().unwrap_or("project");

if let Some(lib_name) = scope_str.strip_prefix("lib:") {
    let force = input["force"].as_bool().unwrap_or(false);

    let (root, lib_path) = {
        let inner = ctx.agent.inner.read().await;
        let project = inner.active_project.as_ref().ok_or_else(|| {
            crate::tools::RecoverableError::with_hint(
                "No active project. Use activate_project first.",
                "Call activate_project(\"/path/to/project\") to set the active project.",
            )
        })?;
        let entry = project.library_registry.lookup(lib_name).ok_or_else(|| {
            crate::tools::RecoverableError::with_hint(
                format!("Library '{}' not found in registry.", lib_name),
                "Use list_libraries to see registered libraries.",
            )
        })?;
        (project.root.clone(), entry.path.clone())
    };

    let source = format!("lib:{}", lib_name);
    crate::embed::index::build_library_index(&root, &lib_path, &source, force).await?;

    {
        let mut inner = ctx.agent.inner.write().await;
        let project = inner.active_project.as_mut().unwrap();
        if let Some(entry) = project.library_registry.lookup_mut(lib_name) {
            entry.indexed = true;
        }
        let registry_path = project.root.join(".codescout").join("libraries.json");
        project.library_registry.save(&registry_path)?;
    }

    let source2 = source.clone();
    let root2 = root.clone();
    let (file_count, chunk_count) = tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root2)?;
        let by_source = crate::embed::index::index_stats_by_source(&conn)?;
        let lib_stats = by_source.get(&source2);
        anyhow::Ok((
            lib_stats.map_or(0, |s| s.file_count),
            lib_stats.map_or(0, |s| s.chunk_count),
        ))
    })
    .await??;

    return Ok(json!({
        "status": "ok",
        "library": lib_name,
        "source": source,
        "files_indexed": file_count,
        "chunks": chunk_count,
    }));
}
```

**Step 6: Run test to verify it passes**

```
cargo test index_project_scope_lib_errors_for_unknown --lib
```

Expected: PASS.

**Step 7: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

**Step 8: Commit**

```
git add src/tools/semantic.rs src/tools/library.rs
git commit -m "feat(index_project): add scope param — replaces index_library"
```

---

## Task 3: Add `memory` dispatch tool

**Files:**
- Modify: `src/tools/memory.rs` — add `Memory` struct at the end, before `#[cfg(test)]`
- Modify: `src/server.rs` — register `Memory`

**Step 1: Write the failing tests**

In `src/tools/memory.rs` tests module, add at the end (before the closing `}`):

```rust
#[tokio::test]
async fn memory_write_and_read_via_dispatch() {
    let (dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;

    // write
    let w = tool.call(json!({ "action": "write", "topic": "test/key", "content": "hello" }), &ctx).await.unwrap();
    assert_eq!(w, json!("ok"));

    // read
    let r = tool.call(json!({ "action": "read", "topic": "test/key" }), &ctx).await.unwrap();
    assert_eq!(r["content"], json!("hello"));

    drop(dir);
}

#[tokio::test]
async fn memory_list_via_dispatch() {
    let (dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    tool.call(json!({ "action": "write", "topic": "a", "content": "x" }), &ctx).await.unwrap();
    let result = tool.call(json!({ "action": "list" }), &ctx).await.unwrap();
    let topics = result["topics"].as_array().expect("expected topics array");
    assert!(topics.iter().any(|t| t.as_str() == Some("a")));
    drop(dir);
}

#[tokio::test]
async fn memory_delete_via_dispatch() {
    let (dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    tool.call(json!({ "action": "write", "topic": "to_delete", "content": "x" }), &ctx).await.unwrap();
    tool.call(json!({ "action": "delete", "topic": "to_delete" }), &ctx).await.unwrap();
    let result = tool.call(json!({ "action": "read", "topic": "to_delete" }), &ctx).await;
    assert!(result.is_err(), "expected error reading deleted topic");
    drop(dir);
}

#[tokio::test]
async fn memory_unknown_action_returns_recoverable_error() {
    let (dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    let result = tool.call(json!({ "action": "explode" }), &ctx).await;
    assert!(result.is_err());
    drop(dir);
}
```

**Step 2: Run tests to verify they fail**

```
cargo test memory_write_and_read_via_dispatch --lib
```

Expected: FAIL — `Memory` struct not found.

**Step 3: Add `Memory` struct to `src/tools/memory.rs`**

Insert before `#[cfg(test)]` at the bottom of the file:

```rust
pub struct Memory;

#[async_trait::async_trait]
impl Tool for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Persistent project memory — action: \"read\", \"write\", \"list\", \"delete\". \
         topic is a path-like key (e.g. 'debugging/async-patterns'). \
         Pass private=true to use the gitignored private store."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "list", "delete"],
                    "description": "Operation to perform"
                },
                "topic": {
                    "type": "string",
                    "description": "Required for read/write/delete. Path-like key, e.g. 'debugging/async-patterns'."
                },
                "content": {
                    "type": "string",
                    "description": "Required for write. The content to persist."
                },
                "private": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, use the gitignored private store."
                },
                "include_private": {
                    "type": "boolean",
                    "default": false,
                    "description": "For list: also return private topics. Returns { shared, private } instead of { topics }."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let action = super::require_str_param(&input, "action")?;
        match action {
            "write" => {
                let topic = super::require_str_param(&input, "topic")?;
                let content = super::require_str_param(&input, "content")?;
                let private = input["private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        if private {
                            p.private_memory.write(topic, content)?;
                        } else {
                            p.memory.write(topic, content)?;
                        }
                        Ok(json!("ok"))
                    })
                    .await
            }
            "read" => {
                let topic = super::require_str_param(&input, "topic")?;
                let private = input["private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        let store = if private { &p.private_memory } else { &p.memory };
                        match store.read(topic)? {
                            Some(content) => Ok(json!({ "content": content })),
                            None => Err(RecoverableError::with_hint(
                                format!("topic '{}' not found", topic),
                                "Use memory(action='list') to see available topics",
                            )
                            .into()),
                        }
                    })
                    .await
            }
            "list" => {
                let include_private = input["include_private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        if include_private {
                            Ok(json!({ "shared": p.memory.list()?, "private": p.private_memory.list()? }))
                        } else {
                            Ok(json!({ "topics": p.memory.list()? }))
                        }
                    })
                    .await
            }
            "delete" => {
                let topic = super::require_str_param(&input, "topic")?;
                let private = input["private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        if private {
                            p.private_memory.delete(topic)?;
                        } else {
                            p.memory.delete(topic)?;
                        }
                        Ok(json!("ok"))
                    })
                    .await
            }
            _ => Err(RecoverableError::with_hint(
                format!("unknown action '{}'. Must be one of: read, write, list, delete", action),
                "Pass action: 'read', 'write', 'list', or 'delete'",
            )
            .into()),
        }
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        if result["topics"].is_array() || result["shared"].is_array() {
            Some(format_list_memories(result))
        } else if result["content"].is_string() {
            Some(format_read_memory(result))
        } else {
            None
        }
    }
}
```

**Step 4: Register `Memory` in `src/server.rs`**

In `src/server.rs`, find the import for memory tools (around line 33). Add `Memory` to the import:

```rust
use crate::tools::memory::{DeleteMemory, ListMemories, Memory, ReadMemory, WriteMemory};
```

In the `tools: vec![...]` block, add after `Arc::new(DeleteMemory)`:

```rust
Arc::new(Memory),
```

**Step 5: Run tests to verify they pass**

```
cargo test memory_write_and_read_via_dispatch memory_list_via_dispatch memory_delete_via_dispatch memory_unknown_action --lib
```

Expected: all 4 PASS.

**Step 6: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

**Step 7: Commit**

```
git add src/tools/memory.rs src/server.rs
git commit -m "feat(memory): add Memory dispatch tool — single entry point for all memory ops"
```

---

## Task 4: Remove 9 tools from server + path security

**The 9 tools being removed:** `list_functions`, `list_docs`, `index_library`, `write_memory`, `read_memory`, `list_memories`, `delete_memory`, `git_blame`, `index_status`.

(Note: `get_usage_stats` and `get_config` are removed in Task 5 when renamed/merged into `project_status`.)

**Files:**
- Modify: `src/server.rs` — remove 9 Arc::new registrations + remove from imports + update test
- Modify: `src/util/path_security.rs` — remove `git_blame` arm, remove `index_status` from indexing arm

**Step 1: Verify the test currently passes (count = 33)**

```
cargo test server_registers_all_tools --lib
```

Expected: PASS with 33 tools currently registered (32 original + 1 new Memory).

**Step 2: Remove the 9 tool registrations from `src/server.rs`**

In the `tools: vec![...]` block, remove these lines:

```rust
Arc::new(ListFunctions),
Arc::new(ListDocs),
Arc::new(IndexLibrary),
Arc::new(WriteMemory),
Arc::new(ReadMemory),
Arc::new(ListMemories),
Arc::new(DeleteMemory),
Arc::new(GitBlame),
Arc::new(IndexStatus),
```

**Step 3: Update imports in `src/server.rs`**

Remove the old memory tool imports. The memory import line should now be just:

```rust
use crate::tools::memory::Memory;
```

Remove `GitBlame` from the git import. Remove `ListFunctions`, `ListDocs` from ast import. Remove `IndexLibrary` from library import. Remove `IndexStatus` from semantic import.

**Step 4: Update `server_registers_all_tools` test**

In `src/server.rs` tests, update `expected_tools` to remove the 9 names and verify the new count is 24 (33 - 9 = 24):

```rust
let expected_tools = [
    "read_file",
    "list_dir",
    "search_pattern",
    "create_file",
    "find_file",
    "edit_file",
    "run_command",
    "onboarding",
    "find_symbol",
    "find_references",
    "list_symbols",
    "replace_symbol",
    "insert_code",
    "rename_symbol",
    "remove_symbol",
    "goto_definition",
    "hover",
    "semantic_search",
    "index_project",
    "activate_project",
    "get_config",      // will be renamed in Task 5
    "list_libraries",
    "get_usage_stats", // will be removed in Task 5
    "memory",
];
```

**Step 5: Remove `git_blame` arm from `src/util/path_security.rs`**

In `check_tool_access`, remove:

```rust
"git_blame" => {
    if !config.git_enabled {
        bail!(
            "Git tools are disabled. Set security.git_enabled = true in .codescout/project.toml to enable."
        );
    }
}
```

And in the indexing arm, remove `"index_status"`:

```rust
"semantic_search" | "index_project" => {   // was: "semantic_search" | "index_project" | "index_status"
```

**Step 6: Update the security gate test**

In `src/util/path_security.rs`, find `git_disabled_blocks_git_tools`. Remove it or update it to note that git_blame no longer exists. The simplest fix: delete the test entirely since there are no remaining git-specific tools.

Also find `all_read_tools_always_allowed` (or similar) that tests the remaining tools still pass — update if needed.

**Step 7: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

Expected: all tests pass. If any test file directly imports `ListFunctions`, `ListDocs`, etc. — those imports will fail to compile; fix them by removing the import.

**Step 8: Commit**

```
git add src/server.rs src/util/path_security.rs
git commit -m "chore: remove 9 deprecated tools (list_functions, list_docs, index_library, 4 memory tools, git_blame, index_status)"
```

---

## Task 5: Rename `get_config` → `project_status`, fold in index + usage data

**Files:**
- Modify: `src/tools/config.rs` — rename struct + name(), enrich output
- Modify: `src/tools/usage.rs` — remove `GetUsageStats` struct (fold into config.rs)
- Modify: `src/server.rs` — update imports + registration, update test

**Step 1: Write the failing test**

In `src/tools/config.rs` tests module, add after `activate_and_get_config`:

```rust
#[tokio::test]
async fn project_status_returns_all_sections() {
    let (dir, ctx) = {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = crate::agent::Agent::new(Some(dir.path().to_path_buf()))
            .await
            .unwrap();
        let lsp = std::sync::Arc::new(crate::lsp::MockLspProvider::default());
        let output_buffer = std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new());
        let ctx = crate::tools::ToolContext {
            agent: std::sync::Arc::new(agent),
            lsp,
            output_buffer,
            progress: None,
        };
        (dir, ctx)
    };
    let tool = ProjectStatus;
    let result = tool.call(json!({}), &ctx).await.unwrap();
    assert!(result["project_root"].is_string(), "missing project_root");
    assert!(result["config"].is_object(), "missing config section");
    // index section: may be {"indexed": false} if no DB exists — just check the key exists
    assert!(result.get("index").is_some(), "missing index section");
    // usage section
    assert!(result.get("usage").is_some(), "missing usage section");
    // libraries section
    assert!(result.get("libraries").is_some(), "missing libraries section");
    drop(dir);
}
```

**Step 2: Run test to verify it fails**

```
cargo test project_status_returns_all_sections --lib
```

Expected: FAIL — `ProjectStatus` not found.

**Step 3: Rewrite `src/tools/config.rs`**

Replace the `GetConfig` struct and its impl entirely. Also add the needed imports at the top:

```rust
use crate::tools::{RecoverableError, Tool, ToolContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ActivateProject;
pub struct ProjectStatus;

#[async_trait]
impl Tool for ActivateProject {
    // ... (unchanged from current ActivateProject impl)
}

#[async_trait]
impl Tool for ProjectStatus {
    fn name(&self) -> &str {
        "project_status"
    }

    fn description(&self) -> &str {
        "Active project state: config, semantic index health, usage telemetry, and library summary. \
         Pass threshold (float) to include drift scores. Pass window ('1h','24h','7d','30d') for usage window."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "threshold": {
                    "type": "number",
                    "description": "Min avg_drift to include (0.0-1.0). When provided, adds drift data to index section."
                },
                "path": {
                    "type": "string",
                    "description": "Glob pattern to filter drift files (SQL LIKE syntax, e.g. 'src/tools/%')."
                },
                "detail_level": {
                    "type": "string",
                    "enum": ["exploring", "full"],
                    "description": "Drift output detail. 'full' includes most-drifted chunk content."
                },
                "window": {
                    "type": "string",
                    "enum": ["1h", "24h", "7d", "30d"],
                    "description": "Time window for usage stats. Default: 30d."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        // --- Config section (was GetConfig) ---
        let (root, config_val, lib_count, lib_indexed) = ctx
            .agent
            .with_project(|p| {
                let lib_count = p.library_registry.all().len();
                let lib_indexed = p.library_registry.all().iter().filter(|e| e.indexed).count();
                Ok((
                    p.root.clone(),
                    serde_json::to_value(&p.config)?,
                    lib_count,
                    lib_indexed,
                ))
            })
            .await?;

        let mut result = json!({
            "project_root": root.display().to_string(),
            "config": config_val,
            "libraries": { "count": lib_count, "indexed": lib_indexed },
        });

        // --- Index section (was IndexStatus, basic only) ---
        let db_path = crate::embed::index::db_path(&root);
        if !db_path.exists() {
            result["index"] = json!({ "indexed": false });
        } else {
            let root2 = root.clone();
            let index_result = tokio::task::spawn_blocking(move || {
                let conn = crate::embed::index::open_db(&root2)?;
                let stats = crate::embed::index::index_stats(&conn)?;
                let staleness = crate::embed::index::check_index_staleness(&conn, &root2).ok();
                anyhow::Ok((stats, staleness))
            })
            .await;

            match index_result {
                Ok(Ok((stats, staleness))) => {
                    let mut index_section = json!({
                        "indexed": true,
                        "files": stats.file_count,
                        "chunks": stats.chunk_count,
                        "last_updated": stats.indexed_at,
                        "model": stats.model,
                    });
                    if let Some(s) = staleness {
                        if s.stale {
                            index_section["stale"] = json!(true);
                            index_section["behind_commits"] = json!(s.behind_commits);
                        }
                    }

                    // Drift — only if threshold or path param provided
                    let wants_drift = input.get("threshold").is_some() || input.get("path").is_some();
                    if wants_drift {
                        use crate::tools::output::OutputGuard;
                        let (root3, drift_enabled) = ctx
                            .agent
                            .with_project(|p| Ok((p.root.clone(), p.config.embeddings.drift_detection_enabled)))
                            .await?;
                        if !drift_enabled {
                            index_section["drift"] = json!({
                                "status": "disabled",
                                "hint": "Set embeddings.drift_detection_enabled = true in .codescout/project.toml"
                            });
                        } else {
                            let threshold = input["threshold"].as_f64().map(|v| v as f32).unwrap_or(0.1);
                            let path_filter = input["path"].as_str().map(|s| s.to_string());
                            let guard = OutputGuard::from_input(&input);
                            let rows = tokio::task::spawn_blocking(move || {
                                let conn = crate::embed::index::open_db(&root3)?;
                                crate::embed::index::query_drift_report(&conn, Some(threshold), path_filter.as_deref())
                            })
                            .await??;
                            let items: Vec<Value> = rows
                                .iter()
                                .map(|r| {
                                    let mut obj = json!({
                                        "file_path": r.file_path,
                                        "avg_drift": r.avg_drift,
                                        "max_drift": r.max_drift,
                                    });
                                    if guard.should_include_body() {
                                        if let Some(chunk) = &r.max_drift_chunk {
                                            obj["max_drift_chunk"] = json!(chunk);
                                        }
                                    }
                                    obj
                                })
                                .collect();
                            let (items, overflow) = guard.cap_items(items, "Use detail_level='full' with offset for pagination");
                            let total = overflow.as_ref().map_or(items.len(), |o| o.total);
                            let mut drift_result = json!({ "results": items, "total": total });
                            if let Some(ov) = overflow {
                                drift_result["overflow"] = OutputGuard::overflow_json(&ov);
                            }
                            index_section["drift"] = drift_result;
                        }
                    }

                    result["index"] = index_section;
                }
                _ => {
                    result["index"] = json!({ "indexed": false, "error": "failed to read index" });
                }
            }
        }

        // --- Usage section (was GetUsageStats) ---
        let window = input["window"].as_str().unwrap_or("30d");
        match crate::usage::db::open_db(&root)
            .and_then(|conn| crate::usage::db::query_stats(&conn, window))
        {
            Ok(stats) => {
                result["usage"] = serde_json::to_value(stats).unwrap_or(json!(null));
            }
            Err(_) => {
                result["usage"] = json!({ "window": window, "by_tool": [] });
            }
        }

        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_project_status(result))
    }
}

fn format_activate_project(result: &Value) -> String {
    let root = result["activated"]["project_root"]
        .as_str()
        .or_else(|| result["path"].as_str())
        .unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    format!("activated · {name}")
}

fn format_project_status(result: &Value) -> String {
    let root = result["project_root"].as_str().unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    let indexed = result["index"]["indexed"].as_bool().unwrap_or(false);
    let index_str = if indexed {
        let files = result["index"]["files"].as_u64().unwrap_or(0);
        let chunks = result["index"]["chunks"].as_u64().unwrap_or(0);
        format!("index:{files}f/{chunks}c")
    } else {
        "index:none".to_string()
    };
    format!("status · {name} · {index_str}")
}
```

**Step 4: Update `src/server.rs`**

Update imports: replace `GetConfig` with `ProjectStatus`, remove `GetUsageStats` import.

Update registration: replace `Arc::new(GetConfig)` with `Arc::new(ProjectStatus)`, remove `Arc::new(GetUsageStats)`.

Update `server_registers_all_tools` test — replace `"get_config"` with `"project_status"` and remove `"get_usage_stats"`. Final expected count: **23**.

**Step 5: Run the new test**

```
cargo test project_status_returns_all_sections --lib
```

Expected: PASS.

**Step 6: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

Expected: all tests pass including `server_registers_all_tools` with count 23.

**Step 7: Commit**

```
git add src/tools/config.rs src/tools/usage.rs src/server.rs
git commit -m "feat(project_status): rename get_config, fold in index + usage + library data (32 → 23 tools)"
```

---

## Task 6: Update descriptions and `server_instructions.md`

**Files:**
- Modify: `src/tools/library.rs` — update `list_libraries` description
- Modify: `src/prompts/server_instructions.md` — remove old tool refs, update navigation guide

**Step 1: Update `list_libraries` description**

In `src/tools/library.rs`, find `impl Tool for ListLibraries` → `fn description`:

```rust
fn description(&self) -> &str {
    "List registered libraries and their index status. \
     Use scope='lib:<name>' in semantic_search, find_symbol, or index_project to target a library."
}
```

**Step 2: Update `src/prompts/server_instructions.md`**

Apply these specific replacements:

**In the "Navigate code" section**, replace:
```
- `list_functions(path)` — quick function/method signatures (tree-sitter, no LSP)
- `list_docs(path)` — extract all docstrings and doc comments from a file (tree-sitter)
```
with:
```
- `list_symbols(path, include_docs=true)` — also returns docstrings alongside symbols (tree-sitter)
```

**Replace** the `git_blame` line:
```
- `git_blame(path)` — who last changed each line and in which commit
```
with:
```
- `run_command("git blame path")` — who last changed each line and in which commit
```

**In the "Library code" section**, replace:
```
- `list_libraries` — show registered libraries and their status
- `index_library(name)` — build embedding index for a library
```
with:
```
- `list_libraries` — show registered libraries and their status
- `index_project(scope='lib:name')` — build embedding index for a library
```

**In the "Project Management" section**, replace:
```
- `get_config` — show active project config and server settings
- `index_project` — build or incrementally update the semantic search index
- `index_status` — index stats, staleness, and drift scores. Pass `threshold` to query drift.
```
with:
```
- `project_status` — active project config, index health, usage telemetry, library summary. Pass `threshold` for drift.
- `index_project` — build or incrementally update the semantic search index. Pass `scope='lib:name'` for libraries.
```

**Replace the Memory section**:
```
- `write_memory(topic, content)` — persist knowledge (topic is path-like, e.g. 'debugging/async-patterns')
- `read_memory(topic)` — retrieve a stored entry
- `list_memories` — list all topics
- `delete_memory(topic)` — remove an entry
- `write_memory(topic, content, private=true)` — store in project-local private store (not surfaced in system instructions; use for sensitive or session-specific notes)
- `list_memories(include_private=true)` — returns both shared and private memories in `{ shared: [...], private: [...] }` shape
```
with:
```
- `memory(action, topic?, content?)` — CRUD for persistent project knowledge.
  - `action: "write"` — `topic` + `content` required. `private=true` for gitignored store.
  - `action: "read"` — `topic` required. `private=true` to read from private store.
  - `action: "list"` — lists all topics. `include_private=true` returns `{ shared, private }`.
  - `action: "delete"` — `topic` required.
```

**Step 3: Verify the server_instructions still looks right**

```
cargo run -- start --project . 2>&1 | head -5
```

(Just a smoke test that the server starts; no actual validation of the markdown content is needed.)

**Step 4: Run full test suite + lint**

```
cargo test && cargo clippy -- -D warnings && cargo fmt
```

**Step 5: Commit**

```
git add src/tools/library.rs src/prompts/server_instructions.md
git commit -m "docs: update descriptions and server_instructions for 23-tool surface"
```

---

## Final Verification

```
cargo test && cargo clippy -- -D warnings
```

Run: `cargo test server_registers_all_tools --lib`
Expected output: `test server_registers_all_tools ... ok` with no count mismatch.

Verify tool count in `src/server.rs` by counting `Arc::new(` lines in `from_parts`: should be exactly **23**.
