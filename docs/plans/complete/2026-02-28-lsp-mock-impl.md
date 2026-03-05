# LSP Mock Infrastructure — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce thin trait abstractions over LSP types so tool tests can inject a mock LSP client, then write regression tests for BUG-003 and BUG-004.

**Architecture:** Two traits (`LspClientOps`, `LspProvider`) thin-wrap the existing `LspClient` and `LspManager`. `ToolContext.lsp` changes from `Arc<LspManager>` to `Arc<dyn LspProvider>`. A `MockLspClient`/`MockLspProvider` pair (cfg-gated) provides canned `document_symbols` responses for tests.

**Tech Stack:** Rust, `async_trait`, `tokio`, `tempfile` (already in dev-deps)

**Design doc:** `docs/plans/2026-02-28-lsp-mock-design.md`

---

### Task 1: Define `LspClientOps` and `LspProvider` traits

**Files:**
- Create: `src/lsp/ops.rs`

This is purely additive — nothing else changes yet.

**Step 1: Create the traits file**

```rust
// src/lsp/ops.rs
use std::path::Path;
use std::sync::Arc;

use crate::lsp::SymbolInfo;

/// Abstract interface over LSP operations used by tools.
/// `LspClient` implements this; `MockLspClient` implements it for tests.
#[async_trait::async_trait]
pub trait LspClientOps: Send + Sync {
    async fn document_symbols(
        &self,
        path: &Path,
        language_id: &str,
    ) -> anyhow::Result<Vec<SymbolInfo>>;

    async fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<SymbolInfo>>;

    async fn references(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>>;

    async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>>;

    async fn hover(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Option<String>>;

    async fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
        language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit>;

    async fn did_change(&self, path: &Path) -> anyhow::Result<()>;
}

/// Abstract factory that starts or retrieves an LSP client for a given language.
/// `LspManager` implements this; `MockLspProvider` implements it for tests.
#[async_trait::async_trait]
pub trait LspProvider: Send + Sync {
    async fn get_or_start(
        &self,
        language: &str,
        workspace_root: &Path,
    ) -> anyhow::Result<Arc<dyn LspClientOps>>;

    async fn notify_file_changed(&self, path: &Path);
}
```

**Step 2: Build**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: no errors (file is not wired in yet — just exists).

**Step 3: Commit**

```bash
git add src/lsp/ops.rs
git commit -m "feat(lsp): define LspClientOps and LspProvider traits"
```

---

### Task 2: Implement `LspClientOps` for `LspClient`

**Files:**
- Modify: `src/lsp/client.rs` (add impl block at the end, before tests)

**Step 1: Add the impl**

Add this block just before the `#[cfg(test)]` section in `src/lsp/client.rs`:

```rust
#[async_trait::async_trait]
impl crate::lsp::ops::LspClientOps for LspClient {
    async fn document_symbols(
        &self,
        path: &std::path::Path,
        language_id: &str,
    ) -> anyhow::Result<Vec<crate::lsp::SymbolInfo>> {
        self.document_symbols(path, language_id).await
    }

    async fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<crate::lsp::SymbolInfo>> {
        self.workspace_symbols(query).await
    }

    async fn references(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        self.references(path, line, col, language_id).await
    }

    async fn goto_definition(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        self.goto_definition(path, line, col, language_id).await
    }

    async fn hover(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.hover(path, line, col, language_id).await
    }

    async fn rename(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        new_name: &str,
        language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit> {
        self.rename(path, line, col, new_name, language_id).await
    }

    async fn did_change(&self, path: &std::path::Path) -> anyhow::Result<()> {
        self.did_change(path).await
    }
}
```

> **Note on method dispatch:** `self.document_symbols(...)` inside a trait impl calls the
> *inherent* method on `LspClient` (inherent methods have priority over trait methods in
> Rust's method resolution). If the compiler reports ambiguity, replace with
> `LspClient::document_symbols(self, ...)` (UFCS) for all methods.

**Step 2: Build**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: no errors.

**Step 3: Commit**

```bash
git add src/lsp/client.rs
git commit -m "feat(lsp): implement LspClientOps for LspClient"
```

---

### Task 3: Implement `LspProvider` for `LspManager` + re-export

**Files:**
- Modify: `src/lsp/manager.rs` (add impl block)
- Modify: `src/lsp/mod.rs` (re-export traits + add `new_arc` convenience)

**Step 1: Add impl to `manager.rs`**

Add this block just before the `#[cfg(test)]` section:

```rust
#[async_trait::async_trait]
impl crate::lsp::ops::LspProvider for LspManager {
    async fn get_or_start(
        &self,
        language: &str,
        workspace_root: &std::path::Path,
    ) -> anyhow::Result<std::sync::Arc<dyn crate::lsp::ops::LspClientOps>> {
        // LspManager::get_or_start (UFCS) calls the inherent method, returns Arc<LspClient>
        let client = LspManager::get_or_start(self, language, workspace_root).await?;
        Ok(client as std::sync::Arc<dyn crate::lsp::ops::LspClientOps>)
    }

    async fn notify_file_changed(&self, path: &std::path::Path) {
        // LspManager::notify_file_changed (UFCS) calls the inherent method
        LspManager::notify_file_changed(self, path).await
    }
}

impl LspManager {
    /// Convenience constructor that returns `Arc<dyn LspProvider>`.
    /// Use this in `ToolContext` construction instead of `Arc::new(LspManager::new())`.
    pub fn new_arc() -> std::sync::Arc<dyn crate::lsp::ops::LspProvider> {
        std::sync::Arc::new(Self::new())
    }
}
```

**Step 2: Re-export from `src/lsp/mod.rs`**

Add to the top-level re-exports in `src/lsp/mod.rs`:

```rust
pub mod ops;
pub use ops::{LspClientOps, LspProvider};
```

**Step 3: Build**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: no errors.

**Step 4: Commit**

```bash
git add src/lsp/manager.rs src/lsp/mod.rs
git commit -m "feat(lsp): implement LspProvider for LspManager, add new_arc constructor"
```

---

### Task 4: Thread `Arc<dyn LspProvider>` through `ToolContext`

This is the **breaking change** — it will fail to compile until all construction sites are updated.

**Files:**
- Modify: `src/tools/mod.rs` — `ToolContext.lsp` field type
- Modify: `src/tools/symbol.rs` — `get_lsp_client` return type + `lsp()` test helper
- Modify: `src/server.rs` — `ToolContext` construction
- Modify: `src/tools/file.rs`, `tools/semantic.rs`, `tools/git.rs`, `tools/library.rs`,
  `tools/usage.rs`, `tools/memory.rs`, `tools/ast.rs`, `tools/workflow.rs`, `tools/config.rs`
  — all have test helper functions that construct `ToolContext`
- Modify: `tests/integration.rs`, `tests/rename_symbol.rs`, `tests/e2e/harness.rs`

**Step 1: Change `ToolContext` field in `src/tools/mod.rs`**

```rust
// Before:
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<LspManager>,
}

// After:
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn crate::lsp::LspProvider>,
}
```

Also update the import — remove `use crate::lsp::LspManager;` if it's no longer needed here.

**Step 2: Update `get_lsp_client` in `src/tools/symbol.rs`**

```rust
// Before:
async fn get_lsp_client(
    ctx: &ToolContext,
    path: &Path,
) -> anyhow::Result<(std::sync::Arc<crate::lsp::LspClient>, String)> {

// After:
async fn get_lsp_client(
    ctx: &ToolContext,
    path: &Path,
) -> anyhow::Result<(std::sync::Arc<dyn crate::lsp::LspClientOps>, String)> {
```

The body stays identical — `ctx.lsp.get_or_start(lang, &root)` now returns `Arc<dyn LspClientOps>` via the trait.

**Step 3: Update the `lsp()` test helper in `src/tools/symbol.rs`**

Search for `fn lsp() -> Arc<LspManager>` (around line 1697) and change it to:

```rust
fn lsp() -> Arc<dyn crate::lsp::LspProvider> {
    LspManager::new_arc()
}
```

**Step 4: Update all other test helpers**

All other per-file test helpers that construct `ToolContext` use the same pattern. Find them all with:

```bash
grep -rn "Arc::new(LspManager::new())" src/ tests/
```

Replace each occurrence of `Arc::new(LspManager::new())` with `LspManager::new_arc()`.

Common patterns to replace:
```rust
// Before (anywhere in tests):
lsp: Arc::new(LspManager::new()),

// After:
lsp: LspManager::new_arc(),
```

Also update any place that explicitly types the field:
```rust
// Before:
let lsp: Arc<LspManager> = Arc::new(LspManager::new());

// After:
let lsp = LspManager::new_arc();
```

**Step 5: Update `src/server.rs`**

Find the `ToolContext` construction (around line 170) and update `lsp:` field similarly.

**Step 6: Update `tests/integration.rs`**

```rust
// In project_with_files:
let ctx = ToolContext {
    agent,
    lsp: LspManager::new_arc(),   // was: Arc::new(LspManager::new())
};
```

Same pattern in `tests/rename_symbol.rs` and `tests/e2e/harness.rs`.

**Step 7: Build and fix any remaining errors**

```bash
cargo build 2>&1
```

Chase down any remaining type errors one by one. They'll all be the same pattern — `Arc<LspManager>` where `Arc<dyn LspProvider>` is expected.

**Step 8: Run full test suite**

```bash
cargo test 2>&1 | grep -E "FAILED|^error|test result"
```

Expected: all tests pass (same count as before).

**Step 9: Commit**

```bash
git add -p   # stage all modified files
git commit -m "refactor(tools): ToolContext.lsp is now Arc<dyn LspProvider>"
```

---

### Task 5: Add `MockLspClient` and `MockLspProvider`

**Files:**
- Create: `src/lsp/mock.rs`
- Modify: `src/lsp/mod.rs` (re-export mock under `cfg(test)`)

**Step 1: Create `src/lsp/mock.rs`**

```rust
// src/lsp/mock.rs
//! Test-only mock implementations of LspClientOps and LspProvider.
//! Returned symbol lists are configured at construction time; all
//! other LSP methods return Ok(Default::default()) silently.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::lsp::ops::{LspClientOps, LspProvider};
use crate::lsp::SymbolInfo;

pub struct MockLspClient {
    symbols: HashMap<PathBuf, Vec<SymbolInfo>>,
}

impl MockLspClient {
    pub fn new() -> Self {
        Self { symbols: HashMap::new() }
    }

    /// Pre-load symbol results for a given file path.
    /// The path is stored as-is; it must match what the tool passes to `document_symbols`.
    pub fn with_symbols(mut self, path: impl Into<PathBuf>, syms: Vec<SymbolInfo>) -> Self {
        self.symbols.insert(path.into(), syms);
        self
    }
}

#[async_trait::async_trait]
impl LspClientOps for MockLspClient {
    async fn document_symbols(
        &self,
        path: &Path,
        _language_id: &str,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        Ok(self.symbols.get(path).cloned().unwrap_or_default())
    }

    async fn workspace_symbols(&self, _query: &str) -> anyhow::Result<Vec<SymbolInfo>> {
        Ok(vec![])
    }

    async fn references(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        Ok(vec![])
    }

    async fn goto_definition(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        Ok(vec![])
    }

    async fn hover(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn rename(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _new_name: &str,
        _language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit> {
        Ok(lsp_types::WorkspaceEdit::default())
    }

    async fn did_change(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct MockLspProvider {
    client: Arc<MockLspClient>,
}

impl MockLspProvider {
    pub fn with_client(client: MockLspClient) -> Arc<dyn LspProvider> {
        Arc::new(Self { client: Arc::new(client) })
    }
}

#[async_trait::async_trait]
impl LspProvider for MockLspProvider {
    async fn get_or_start(
        &self,
        _language: &str,
        _workspace_root: &Path,
    ) -> anyhow::Result<Arc<dyn LspClientOps>> {
        Ok(Arc::clone(&self.client) as Arc<dyn LspClientOps>)
    }

    async fn notify_file_changed(&self, _path: &Path) {
        // no-op
    }
}
```

**Step 2: Re-export from `src/lsp/mod.rs`**

Add:

```rust
#[cfg(test)]
pub mod mock;
#[cfg(test)]
pub use mock::{MockLspClient, MockLspProvider};
```

**Step 3: Build**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: no errors.

**Step 4: Commit**

```bash
git add src/lsp/mock.rs src/lsp/mod.rs
git commit -m "feat(lsp): add MockLspClient and MockLspProvider for tests"
```

---

### Task 6: Write BUG-003 and BUG-004 regression tests

**Files:**
- Create: `tests/symbol_lsp.rs`
- Modify: `tests/integration.rs` (add `project_with_lsp_mock` helper)

**Step 1: Add `project_with_lsp_mock` helper to `tests/integration.rs`**

Add after `project_with_files`:

```rust
#[cfg(test)]
async fn project_with_lsp_mock(
    files: &[(&str, &str)],
    mock: code_explorer::lsp::MockLspClient,
) -> (tempfile::TempDir, code_explorer::tools::ToolContext) {
    use code_explorer::lsp::MockLspProvider;
    let (dir, mut ctx) = project_with_files(files).await;
    ctx.lsp = MockLspProvider::with_client(mock);
    (dir, ctx)
}
```

**Step 2: Write the failing tests in `tests/symbol_lsp.rs`**

```rust
//! Regression tests for LSP-backed symbol tools using a mock LSP client.
//! These tests verify the file-splice logic without requiring a live language server.

use code_explorer::lsp::{MockLspClient, MockLspProvider, SymbolInfo, SymbolKind};
use code_explorer::tools::symbol::{InsertCode, ReplaceSymbol};
use code_explorer::tools::{Tool, ToolContext};
use code_explorer::Agent;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

async fn ctx_with_mock(
    files: &[(&str, &str)],
    mock: MockLspClient,
) -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: MockLspProvider::with_client(mock),
    };
    (dir, ctx)
}

fn sym(name: &str, start_line: u32, end_line: u32, path: impl Into<std::path::PathBuf>) -> SymbolInfo {
    SymbolInfo {
        name: name.to_string(),
        name_path: name.to_string(),
        kind: SymbolKind::Function,
        file: path.into(),
        start_line,
        end_line,
        start_col: 0,
        children: vec![],
    }
}

// ── BUG-003: replace_symbol must preserve the preceding method's closing `}` ──

#[tokio::test]
async fn replace_symbol_preserves_preceding_close_brace() {
    // File has two methods. The LSP reports `target` as starting on line 0,
    // which is actually the `}` closing the preceding method — the BUG-003 scenario.
    let src = "    }\n\n    fn target() {\n        old_body();\n    }\n";
    // Line indices (0-based): 0=`}`, 1=``, 2=`fn target`, 3=`old_body`, 4=`}`
    let (dir, ctx) = ctx_with_mock(
        &[("src/lib.rs", src)],
        MockLspClient::new().with_symbols(
            dir.path().join("src/lib.rs"),  // absolute path — matches what the tool uses
            vec![sym("target", 0, 4, dir.path().join("src/lib.rs"))],
        ),
    ).await;
    // NOTE: the mock is built after `dir` — use a two-step setup:
    // see the adjusted version below.
    todo!("see adjusted test below")
}

// Adjusted version (mock built after dir is known):
#[tokio::test]
async fn replace_symbol_preserves_preceding_close_brace_v2() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();

    // Line 0 (0-indexed): `    }` — closing brace of preceding method
    // Line 1: blank
    // Line 2: `    fn target() {`
    // Line 3: `        old_body();`
    // Line 4: `    }`
    let src = "    }\n\n    fn target() {\n        old_body();\n    }\n";
    let file = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, src).unwrap();

    // LSP reports `target` starting at line 0 (the `}` line) — BUG-003 scenario
    let mock = MockLspClient::new()
        .with_symbols(file.clone(), vec![sym("target", 0, 4, file.clone())]);

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: MockLspProvider::with_client(mock),
    };

    ReplaceSymbol.call(
        json!({
            "path": file.display().to_string(),
            "name_path": "target",
            "new_body": "    fn target() {\n        new_body();\n    }"
        }),
        &ctx,
    ).await.unwrap();

    let result = std::fs::read_to_string(&file).unwrap();
    assert!(result.contains("    }"),   "preceding close brace must be preserved");
    assert!(result.contains("new_body()"), "replacement body must be applied");
    assert!(!result.contains("old_body()"), "old body must be gone");
}

#[tokio::test]
async fn replace_symbol_clean_start_line() {
    // Normal case: LSP start_line points directly to `fn`, no lead-in.
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();

    // Line 0: `fn foo() {`
    // Line 1: `    old();`
    // Line 2: `}`
    let src = "fn foo() {\n    old();\n}\n";
    let file = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, src).unwrap();

    let mock = MockLspClient::new()
        .with_symbols(file.clone(), vec![sym("foo", 0, 2, file.clone())]);

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext { agent, lsp: MockLspProvider::with_client(mock) };

    ReplaceSymbol.call(
        json!({
            "path": file.display().to_string(),
            "name_path": "foo",
            "new_body": "fn foo() {\n    new();\n}"
        }),
        &ctx,
    ).await.unwrap();

    let result = std::fs::read_to_string(&file).unwrap();
    assert!(result.contains("new()"), "replacement must apply");
    assert!(!result.contains("old()"), "old body must be gone");
}

// ── BUG-004: insert_code "before" must skip lead-in ───────────────────────────

#[tokio::test]
async fn insert_code_before_skips_lead_in() {
    // LSP reports target starting on the `}` of the preceding method.
    // insert_code("before") should land AFTER the `}`, not before it.
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();

    // Line 0: `    }` — preceding close brace (lead-in)
    // Line 1: blank
    // Line 2: `    fn target() {`
    // Line 3: `    }`
    let src = "    }\n\n    fn target() {\n    }\n";
    let file = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, src).unwrap();

    let mock = MockLspClient::new()
        .with_symbols(file.clone(), vec![sym("target", 0, 3, file.clone())]);

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext { agent, lsp: MockLspProvider::with_client(mock) };

    InsertCode.call(
        json!({
            "path": file.display().to_string(),
            "name_path": "target",
            "position": "before",
            "code": "    // inserted\n"
        }),
        &ctx,
    ).await.unwrap();

    let result = std::fs::read_to_string(&file).unwrap();
    // The `}` at line 0 must still be on its own line (not merged with the insertion)
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines[0].trim(), "}", "preceding brace must remain on line 0");
    assert!(result.contains("// inserted"), "insertion must be present");
    // The insertion must appear AFTER the `}`, not before it
    let brace_pos = result.find("    }").unwrap();
    let insert_pos = result.find("// inserted").unwrap();
    assert!(insert_pos > brace_pos, "insertion must land after the preceding `}`");
}

#[tokio::test]
async fn insert_code_after_lands_past_symbol() {
    // Normal "after" case: symbol ends at line 2 (`}`), insertion goes after.
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();

    let src = "fn foo() {\n}\n\n";
    let file = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, src).unwrap();

    let mock = MockLspClient::new()
        .with_symbols(file.clone(), vec![sym("foo", 0, 1, file.clone())]);

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext { agent, lsp: MockLspProvider::with_client(mock) };

    InsertCode.call(
        json!({
            "path": file.display().to_string(),
            "name_path": "foo",
            "position": "after",
            "code": "fn bar() {}\n"
        }),
        &ctx,
    ).await.unwrap();

    let result = std::fs::read_to_string(&file).unwrap();
    assert!(result.contains("fn foo()"), "original must be present");
    assert!(result.contains("fn bar()"), "insertion must be present");
    // bar must come after foo
    let foo_pos = result.find("fn foo()").unwrap();
    let bar_pos = result.find("fn bar()").unwrap();
    assert!(bar_pos > foo_pos, "bar must be inserted after foo");
}
```

> **Note on test structure:** The `ctx_with_mock` helper at the top of the file is the
> clean version to use going forward. The `replace_symbol_preserves_preceding_close_brace`
> test with `todo!` should be removed — it exists only to show the two-step pattern
> problem. Keep `_v2` and rename it to drop the `_v2` suffix.

**Step 3: Clean up — remove the `todo!` test, rename `_v2`**

Delete the first `replace_symbol_preserves_preceding_close_brace` test entirely.
Rename `replace_symbol_preserves_preceding_close_brace_v2` →
`replace_symbol_preserves_preceding_close_brace`.

Refactor all four tests to use the `ctx_with_mock` helper to remove duplication.

Final clean version of each test body:

```rust
#[tokio::test]
async fn replace_symbol_preserves_preceding_close_brace() {
    let src = "    }\n\n    fn target() {\n        old_body();\n    }\n";
    let (dir, ctx) = ctx_with_mock(
        &[("src/lib.rs", src)],
        |file: &std::path::Path| MockLspClient::new()
            .with_symbols(file.to_path_buf(), vec![sym("target", 0, 4, file)]),
    ).await;
    // ...
}
```

> **Simpler approach:** since `ctx_with_mock` needs the absolute path for the mock, pass
> a closure or build the mock inside the helper after the dir is known. The cleanest
> ergonomic design: update `ctx_with_mock` to accept a `FnOnce(&Path) -> MockLspClient`:
>
> ```rust
> async fn ctx_with_mock(
>     files: &[(&str, &str)],
>     build_mock: impl FnOnce(&std::path::Path) -> MockLspClient,
> ) -> (tempfile::TempDir, ToolContext) {
>     let dir = tempdir().unwrap();
>     std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
>     for (name, content) in files {
>         let path = dir.path().join(name);
>         if let Some(parent) = path.parent() {
>             std::fs::create_dir_all(parent).unwrap();
>         }
>         std::fs::write(path, content).unwrap();
>     }
>     let mock = build_mock(dir.path());
>     let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
>     let ctx = ToolContext {
>         agent,
>         lsp: MockLspProvider::with_client(mock),
>     };
>     (dir, ctx)
> }
> ```

**Step 4: Run tests**

```bash
cargo test --test symbol_lsp 2>&1
```

Expected: all 4 tests pass.

**Step 5: Run full suite**

```bash
cargo test 2>&1 | grep -E "test result|FAILED"
```

Expected: all tests pass, count increases by 4.

**Step 6: Clippy**

```bash
cargo clippy -- -D warnings 2>&1 | grep -E "^error"
```

Expected: no errors.

**Step 7: Commit**

```bash
git add tests/symbol_lsp.rs tests/integration.rs
git commit -m "test(symbol): regression tests for BUG-003 and BUG-004 via mock LSP"
```

---

## Summary

| Task | Files | Type |
|------|-------|------|
| 1 — traits | `src/lsp/ops.rs` (new) | Additive |
| 2 — LspClient impl | `src/lsp/client.rs` | Additive |
| 3 — LspManager impl + re-export | `src/lsp/manager.rs`, `src/lsp/mod.rs` | Additive |
| 4 — Thread through ToolContext | 12+ files | Breaking refactor |
| 5 — Mock | `src/lsp/mock.rs` (new) | Additive |
| 6 — Tests | `tests/symbol_lsp.rs` (new) | Tests |

Tasks 1–3 are safe to do in any order. Task 4 must come after 1–3. Task 5 can come
before or after 4. Task 6 requires all previous tasks.
