//! Regression tests for LSP-backed symbol tools using a mock LSP client.
//!
//! These tests verify the file-splice logic (trim_symbol_start fix for BUG-003/004)
//! without requiring a live language server. The mock returns pre-configured
//! symbol positions that reproduce the exact LSP range quirk rust-analyzer exhibits.

use code_explorer::agent::Agent;
use code_explorer::lsp::{MockLspClient, MockLspProvider, SymbolInfo, SymbolKind};
use code_explorer::tools::symbol::{InsertCode, ReplaceSymbol};
use code_explorer::tools::{Tool, ToolContext};
use serde_json::json;

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a ToolContext with a mock LSP provider.
///
/// `files` are written relative to a fresh tempdir (which becomes the project root).
/// `build_mock` receives the absolute project root so it can pre-load symbols keyed
/// by their absolute paths (which is what the tool passes to `document_symbols`).
async fn ctx_with_mock(
    files: &[(&str, &str)],
    build_mock: impl FnOnce(&std::path::Path) -> MockLspClient,
) -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let mock = build_mock(dir.path());
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: MockLspProvider::with_client(mock),
        output_buffer: std::sync::Arc::new(code_explorer::tools::output_buffer::OutputBuffer::new(
            20,
        )),
    };
    (dir, ctx)
}

/// Build a minimal SymbolInfo for use in mock fixtures (0-indexed lines).
fn sym(
    name: &str,
    start_line: u32,
    end_line: u32,
    path: impl Into<std::path::PathBuf>,
) -> SymbolInfo {
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

// ── BUG-003: replace_symbol preserves the preceding method's closing `}` ──────

/// When rust-analyzer reports a symbol as starting at the `}` line of the
/// preceding method, `replace_symbol` must skip that lead-in and start the
/// replacement at the actual `fn` keyword — not silently delete the `}`.
#[tokio::test]
async fn replace_symbol_preserves_preceding_close_brace() {
    // File layout (0-indexed):
    //  0: "    }"          ← closing brace of a preceding method (LSP lead-in)
    //  1: ""               ← blank line
    //  2: "    fn target() {"
    //  3: "        old_body();"
    //  4: "    }"
    let src = "    }\n\n    fn target() {\n        old_body();\n    }\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // LSP reports start_line=0 (the `}` line) — the BUG-003 scenario
            vec![sym("target", 0, 4, file)],
        )
    })
    .await;

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": "    fn target() {\n        new_body();\n    }"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("    }"),
        "preceding close brace must be preserved; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "replacement body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
    // The `}` must come before the new fn body
    let brace_pos = result.find("    }").unwrap();
    let fn_pos = result.find("fn target").unwrap();
    assert!(
        brace_pos < fn_pos,
        "preceding `}}` must appear before fn target; got:\n{result}"
    );
}

#[tokio::test]
async fn replace_symbol_preserves_paren_close_brace() {
    // BUG-003 blind spot: `trim_symbol_start` didn't handle `})` patterns (e.g. the closing
    // of a `json!({...})` macro in a preceding method). The LSP sometimes reports start_line
    // at the `})` line, which trim previously stopped at rather than skipping.
    //
    // File layout (0-indexed):
    //  0: "        })"     ← closing `)` of json! macro in the preceding method
    //  1: "    }"          ← closing brace of the preceding method
    //  2: ""               ← blank line
    //  3: "    fn target() {"
    //  4: "        old_body();"
    //  5: "    }"
    let src = "        })\n    }\n\n    fn target() {\n        old_body();\n    }\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // LSP reports start_line=0 (the `})` line) — the BUG-003 blind-spot scenario
            vec![sym("target", 0, 5, file)],
        )
    })
    .await;

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": "    fn target() {\n        new_body();\n    }"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("        })"),
        "preceding `}})` must be preserved; got:\n{result}"
    );
    assert!(
        result.contains("    }"),
        "preceding method close brace must be preserved; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "replacement body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
    // The `})` must come before the new fn body
    let paren_brace_pos = result.find("        })").unwrap();
    let fn_pos = result.find("fn target").unwrap();
    assert!(
        paren_brace_pos < fn_pos,
        "preceding `}})` must appear before fn target; got:\n{result}"
    );
}

/// Normal case: LSP start_line points directly at `fn` — no lead-in to skip.
#[tokio::test]
async fn replace_symbol_clean_start_line() {
    // File layout (0-indexed):
    //  0: "fn foo() {"
    //  1: "    old();"
    //  2: "}"
    let src = "fn foo() {\n    old();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("foo", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "foo",
                "new_body": "fn foo() {\n    new();\n}"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("new()"),
        "replacement must apply; got:\n{result}"
    );
    assert!(
        !result.contains("old()"),
        "old body must be gone; got:\n{result}"
    );
}

// ── BUG-004: insert_code "before" skips lead-in ───────────────────────────────

/// When LSP reports a symbol as starting on the `}` of the preceding method,
/// `insert_code(position="before")` must land AFTER that `}`, not before it.
#[tokio::test]
async fn insert_code_before_skips_lead_in() {
    // File layout (0-indexed):
    //  0: "    }"          ← closing brace of preceding method (LSP lead-in)
    //  1: ""               ← blank line
    //  2: "    fn target() {"
    //  3: "    }"
    let src = "    }\n\n    fn target() {\n    }\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("target", 0, 3, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "position": "before",
                "code": "    // inserted\n"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("// inserted"),
        "insertion must be present; got:\n{result}"
    );
    // The `}` at position 0 must remain on its own line
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(
        lines[0].trim(),
        "}",
        "preceding brace must remain on line 0; got:\n{result}"
    );
    // Insertion must come AFTER the `}`
    let brace_pos = result.find("    }").unwrap();
    let insert_pos = result.find("// inserted").unwrap();
    assert!(
        insert_pos > brace_pos,
        "insertion must land after the preceding `}}`; got:\n{result}"
    );
}

/// Normal "after" case: symbol at [0,1], insertion goes after line 1.
#[tokio::test]
async fn insert_code_after_lands_past_symbol() {
    let src = "fn foo() {\n}\n\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("foo", 0, 1, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "foo",
                "position": "after",
                "code": "fn bar() {}\n"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("fn foo()"),
        "original must be present; got:\n{result}"
    );
    assert!(
        result.contains("fn bar()"),
        "insertion must be present; got:\n{result}"
    );
    let foo_pos = result.find("fn foo()").unwrap();
    let bar_pos = result.find("fn bar()").unwrap();
    assert!(
        bar_pos > foo_pos,
        "bar must be inserted after foo; got:\n{result}"
    );
}
