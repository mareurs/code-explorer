# Design: LSP Mock Infrastructure for Symbol Tool Tests

**Date:** 2026-02-28
**Status:** Approved

## Problem

LSP-backed tools (`replace_symbol`, `insert_code`, `find_symbol`, `list_symbols`,
`goto_definition`, `hover`, `find_references`, `rename_symbol`) are currently untestable
without a live language server. The root issue is that `ToolContext.lsp` is a concrete
`Arc<LspManager>` which spawns real child processes — no seam exists for injection.

The immediate motivation is to write regression tests for BUG-003 (`replace_symbol` eating
the preceding method's closing `}`) and BUG-004 (`insert_code` inserting at the wrong
position), both of which are file-splice bugs triggered by specific LSP-reported symbol
ranges.

## Design

### Two new traits (`src/lsp/ops.rs`)

```rust
#[async_trait]
pub trait LspClientOps: Send + Sync + 'static {
    async fn document_symbols(&self, path: &Path, lang: &str) -> Result<Vec<SymbolInfo>>;
    async fn workspace_symbols(&self, query: &str, lang: &str) -> Result<Vec<SymbolInfo>>;
    async fn references(&self, path: &Path, line: u32, col: u32, lang: &str) -> Result<Vec<Location>>;
    async fn goto_definition(&self, path: &Path, line: u32, lang: &str) -> Result<Vec<Location>>;
    async fn hover(&self, path: &Path, line: u32, col: u32, lang: &str) -> Result<Option<String>>;
    async fn rename(&self, path: &Path, line: u32, col: u32, new_name: &str, lang: &str) -> Result<WorkspaceEdit>;
    async fn did_change(&self, path: &Path, lang: &str, content: &str) -> Result<()>;
}

#[async_trait]
pub trait LspProvider: Send + Sync + 'static {
    async fn get_or_start(&self, lang: Language, root: &Path) -> Result<Arc<dyn LspClientOps>>;
    async fn notify_file_changed(&self, path: &Path);
}
```

`LspClient` implements `LspClientOps`. `LspManager` implements `LspProvider`.

### ToolContext changes (`src/tools/mod.rs`)

```rust
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,   // was: Arc<LspManager>
}
```

`get_lsp_client` in `symbol.rs` returns `(Arc<dyn LspClientOps>, String)` instead of
`(Arc<LspClient>, String)`. No tool `call()` bodies change.

### Mock implementation (`src/lsp/mock.rs`)

Gate: `#[cfg(any(test, feature = "test-utils"))]`

```rust
pub struct MockLspClient {
    symbols: HashMap<PathBuf, Vec<SymbolInfo>>,
}

impl MockLspClient {
    pub fn new() -> Self { ... }
    pub fn with_symbols(mut self, path: impl Into<PathBuf>, syms: Vec<SymbolInfo>) -> Self { ... }
}
```

All `LspClientOps` methods on `MockLspClient` return `Ok(Default::default())` unless
configured. `document_symbols` looks up `self.symbols[path]`.

```rust
pub struct MockLspProvider {
    client: Arc<MockLspClient>,
}

impl MockLspProvider {
    pub fn with_client(client: MockLspClient) -> Arc<Self> { ... }
}
```

`get_or_start` always returns `Arc::clone(&self.client)`. `notify_file_changed` is a no-op.

### Test helpers (`tests/integration.rs`)

```rust
// existing helper, updated to use Arc<dyn LspProvider>
async fn project_with_files(files: &[(&str, &str)]) -> (TempDir, ToolContext);

// new helper for LSP-backed tool tests
async fn project_with_lsp_mock(
    files: &[(&str, &str)],
    mock: MockLspClient,
) -> (TempDir, ToolContext);
```

### Test file (`tests/symbol_lsp.rs`)

Initial tests covering BUG-003 and BUG-004:

- `replace_symbol_preserves_preceding_close_brace` — feeds `start_line=0` where line 0
  is `}` from preceding method; asserts the brace is retained after replacement.
- `replace_symbol_applies_when_start_is_clean` — normal case, no lead-in.
- `insert_code_before_skips_lead_in` — `position="before"` with lead-in; asserts insertion
  lands after the garbage, not before it.
- `insert_code_after_lands_past_symbol` — normal "after" case.

## Files Changed

| File | Change |
|------|--------|
| `src/lsp/ops.rs` | **New** — `LspClientOps` + `LspProvider` traits |
| `src/lsp/mock.rs` | **New** — `MockLspClient` + `MockLspProvider` |
| `src/lsp/client.rs` | Add `impl LspClientOps for LspClient` |
| `src/lsp/manager.rs` | Add `impl LspProvider for LspManager` |
| `src/lsp/mod.rs` | Re-export new traits and mock |
| `src/tools/mod.rs` | `ToolContext.lsp: Arc<dyn LspProvider>` |
| `src/tools/symbol.rs` | `get_lsp_client` → `Arc<dyn LspClientOps>` |
| `tests/integration.rs` | Update `project_with_files`, add `project_with_lsp_mock` |
| `tests/symbol_lsp.rs` | **New** — BUG-003/004 regression tests |

## Constraints

- No tool `call()` bodies change — they only see the trait interface.
- Mock methods not relevant to a given test return `Ok(Default::default())` silently.
- Mock is cfg-gated; zero impact on production binary size.
- `LspClient`'s `did_open` / open-file dedup tracking is internal implementation detail —
  not part of `LspClientOps` (tools don't call it directly; manager handles it).
