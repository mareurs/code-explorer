//! Regression tests for LSP-backed symbol tools using a mock LSP client.
//!
//! These tests verify the "trust LSP" file-splice logic without requiring a live
//! language server. The mock returns pre-configured symbol positions that reproduce
//! LSP range quirks (over-extension, degenerate ranges, lead-in artifacts).

use codescout::agent::Agent;
use codescout::lsp::{MockLspClient, MockLspProvider, SymbolInfo, SymbolKind};
use codescout::tools::symbol::{
    FindSymbol, GotoDefinition, InsertCode, RemoveSymbol, ReplaceSymbol,
};
use codescout::tools::{Tool, ToolContext};
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
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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
        output_buffer: std::sync::Arc::new(codescout::tools::output_buffer::OutputBuffer::new(20)),
        progress: None,
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
        range_start_line: None,
        detail: None,
    }
}

/// Like `sym`, but with an explicit `range_start_line` (simulates documentSymbol
/// which provides both `selectionRange` and `range`).
fn sym_with_range(
    name: &str,
    start_line: u32,
    end_line: u32,
    range_start: u32,
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
        range_start_line: Some(range_start),
        detail: None,
    }
}

// ── replace_symbol: trust LSP start_line ─────────────────────────────────────

/// With "trust LSP" design, when LSP says start_line=0 (the `}` of a preceding
/// method), we replace from line 0. The preceding `}` is replaced along with the
/// old body — there is no lead-in skipping.
#[tokio::test]
async fn replace_symbol_trusts_lsp_start_line() {
    // File layout (0-indexed):
    //  0: "    }"          ← closing brace of a preceding method (LSP start_line=0)
    //  1: ""               ← blank line
    //  2: "    fn target() {"
    //  3: "        old_body();"
    //  4: "    }"
    let src = "    }\n\n    fn target() {\n        old_body();\n    }\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // LSP reports start_line=0 (the `}` line) — trust LSP, replace from there
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
    // With "trust LSP", the preceding `}` is within the LSP range and is replaced
    assert!(
        result.contains("new_body()"),
        "replacement body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
}

/// With "trust LSP" design, when LSP says start_line=0 (the `})` of a preceding
/// method), we replace from line 0. The `})` and `}` lines are gone — no lead-in
/// skipping.
#[tokio::test]
async fn replace_symbol_trusts_lsp_start_with_paren_close() {
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
            // LSP reports start_line=0 (the `})` line) — trust LSP, replace from there
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
    // With "trust LSP", lines 0-5 are replaced — `})` and `}` are gone
    assert!(
        result.contains("new_body()"),
        "replacement body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
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
// ── BUG-018: replace_symbol truncated end_line (inside body, misses closing `}`) ──

/// When LSP reports an end_line that lands inside the function body instead of
/// at the closing `}`, trusting that range causes replace_symbol to splice only
/// the first N lines and leave the tail of the old body in the file — stray
/// tokens, compilation failure, silent corruption.
///
/// validate_symbol_range must catch `end_line < AST end_line` and return
/// RecoverableError before touching the file. Regression test for BUG-018.
#[tokio::test]
async fn replace_symbol_rejects_truncated_end_line() {
    // File layout (0-indexed):
    //  0: "fn target() {"       ← LSP start=0 (correct)
    //  1: "    old_body();"     ← LSP end=1   (WRONG — truncated, misses `}`)
    //  2: "}"                   ← actual end=2, not covered by LSP range
    let src = "fn target() {\n    old_body();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // end_line=1 is inside the body — truncated off-by-one (BUG-018 pattern)
            vec![sym("target", 0, 1, file)],
        )
    })
    .await;

    let err = ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": "fn target() {\n    new_body();\n}"
            }),
            &ctx,
        )
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("suspicious range"),
        "expected suspicious range error, got: {msg}"
    );

    // File must be untouched — truncated splice would have left a stray `}`
    let content = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        content.contains("old_body()"),
        "file must be unmodified after truncated-range guard; got:\n{content}"
    );
}

// ── Read/write symmetry: find_symbol body → replace_symbol round-trip ────────

/// Round-trip: find_symbol(include_body) → modify → replace_symbol preserves attributes.
/// This is the bug that motivated the full-range body change: find_symbol returned
/// body from start_line (no attributes), but replace_symbol replaced from
/// editing_start_line (with attributes), consuming #[test] etc.
#[tokio::test]
async fn replace_symbol_round_trip_preserves_attributes() {
    // File layout (0-indexed):
    //  0: "#[test]"                     <- range_start = 0
    //  1: "/// A test function"
    //  2: "fn target() {"               <- selectionRange.start = 2
    //  3: "    old_body();"
    //  4: "}"                           <- end = 4
    let src = "#[test]\n/// A test function\nfn target() {\n    old_body();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 2, 4, 0, file)])
    })
    .await;

    // Step 1: Read the symbol body (simulates what the agent does)
    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    // The body should include #[test] and /// doc
    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    assert!(
        body.contains("#[test]"),
        "find_symbol body should include attribute; got:\n{body}"
    );

    // Step 2: Agent modifies the body (changes old_body to new_body, keeps attrs)
    let new_body = body.replace("old_body()", "new_body()");

    // Step 3: Replace with the modified body
    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("#[test]"),
        "attribute must be preserved after round-trip; got:\n{result}"
    );
    assert!(
        result.contains("/// A test function"),
        "doc comment must be preserved after round-trip; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "new body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
}

/// Python: decorators above def are in range_start, docstrings are inside the body.
#[tokio::test]
async fn replace_symbol_round_trip_preserves_python_decorator() {
    // File layout (0-indexed):
    //  0: "@staticmethod"               <- range_start = 0
    //  1: "def target():"               <- selectionRange.start = 1
    //  2: "    old_body()"              <- end = 2
    let src = "@staticmethod\ndef target():\n    old_body()\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.py", src)], |root| {
        let file = root.join("src/lib.py");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 1, 2, 0, file)])
    })
    .await;

    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.py",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    assert!(
        body.contains("@staticmethod"),
        "body should include decorator; got:\n{body}"
    );

    let new_body = body.replace("old_body()", "new_body()");

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.py",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.py")).unwrap();
    assert!(
        result.contains("@staticmethod"),
        "decorator must survive round-trip; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "new body must be applied; got:\n{result}"
    );
}

/// Java: @Override annotation + Javadoc above method.
#[tokio::test]
async fn replace_symbol_round_trip_preserves_java_annotation() {
    // File layout (0-indexed):
    //  0: "/** Javadoc comment */"       <- range_start = 0
    //  1: "@Override"
    //  2: "public void target() {"       <- selectionRange.start = 2
    //  3: "    oldBody();"
    //  4: "}"                            <- end = 4
    let src = "/** Javadoc comment */\n@Override\npublic void target() {\n    oldBody();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/Main.java", src)], |root| {
        let file = root.join("src/Main.java");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 2, 4, 0, file)])
    })
    .await;

    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/Main.java",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    assert!(
        body.contains("@Override"),
        "body should include annotation; got:\n{body}"
    );
    assert!(
        body.contains("/** Javadoc"),
        "body should include Javadoc; got:\n{body}"
    );

    let new_body = body.replace("oldBody()", "newBody()");

    ReplaceSymbol
        .call(
            json!({
                "path": "src/Main.java",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/Main.java")).unwrap();
    assert!(
        result.contains("@Override"),
        "annotation must survive; got:\n{result}"
    );
    assert!(
        result.contains("/** Javadoc"),
        "Javadoc must survive; got:\n{result}"
    );
    assert!(
        result.contains("newBody()"),
        "new body applied; got:\n{result}"
    );
}

/// Clean round-trip with no attributes — no regression from the full-range change.
#[tokio::test]
async fn replace_symbol_round_trip_no_attributes() {
    let src = "fn target() {\n    old_body();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // range_start == start — no attributes
            vec![sym_with_range("target", 0, 2, 0, file)],
        )
    })
    .await;

    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    let new_body = body.replace("old_body()", "new_body()");

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("new_body()"),
        "new body must be applied; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
    // File should have exactly the same number of lines
    assert_eq!(result.lines().count(), src.lines().count());
}

/// Agent changes the attribute: #[test] → #[tokio::test]
#[tokio::test]
async fn replace_symbol_round_trip_agent_changes_attribute() {
    let src = "#[test]\nfn target() {\n    old_body();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 1, 3, 0, file)])
    })
    .await;

    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    assert!(body.contains("#[test]"), "body should include attribute");

    // Agent changes the attribute AND the body
    let new_body = body
        .replace("#[test]", "#[tokio::test]")
        .replace("old_body()", "new_body()");

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("#[tokio::test]"),
        "new attribute must be present; got:\n{result}"
    );
    assert!(
        !result.contains("\n#[test]\n"),
        "old attribute must be gone; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "new body must be applied; got:\n{result}"
    );
}

/// Agent modifies the doc comment during a refactor.
#[tokio::test]
async fn replace_symbol_round_trip_agent_changes_doc_comment() {
    let src = "/// Old documentation\nfn target() {\n    old_body();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 1, 3, 0, file)])
    })
    .await;

    let find_result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let body = find_result["symbols"][0]["body"].as_str().unwrap();
    let new_body = body
        .replace(
            "/// Old documentation",
            "/// Updated documentation\n/// With extra detail",
        )
        .replace("old_body()", "new_body()");

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": new_body
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("/// Updated documentation"),
        "new doc must be present; got:\n{result}"
    );
    assert!(
        result.contains("/// With extra detail"),
        "extra doc line must be present; got:\n{result}"
    );
    assert!(
        !result.contains("/// Old documentation"),
        "old doc must be gone; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "new body must be applied; got:\n{result}"
    );
}

/// insert_code(before) with range_start_line set — must insert ABOVE the attribute,
/// not between attribute and fn.
#[tokio::test]
async fn insert_code_before_with_range_start_line_inserts_above_attribute() {
    // File layout (0-indexed):
    //  0: "#[test]"                     <- range_start = 0
    //  1: "fn target() {}"              <- selectionRange.start = 1
    let src = "#[test]\nfn target() {}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 1, 1, 0, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "position": "before",
                "code": "// inserted above"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(
        lines[0], "// inserted above",
        "inserted code must be above #[test]; got:\n{result}"
    );
    // insert_code(before) adds a blank separator line after the inserted code
    assert_eq!(
        lines[1], "",
        "blank separator line after inserted code; got:\n{result}"
    );
    assert_eq!(
        lines[2], "#[test]",
        "#[test] must follow separator; got:\n{result}"
    );
    assert!(
        lines[3].contains("fn target()"),
        "fn must follow #[test]; got:\n{result}"
    );
}

/// find_symbol body_start_line field present and correct in integration context.
#[tokio::test]
async fn find_symbol_body_start_line_field_with_attributes() {
    let src = "#[test]\n/// doc\nfn target() {\n    body();\n}\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 2, 4, 0, file)])
    })
    .await;

    let result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs",
                "include_body": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    let sym = &result["symbols"][0];
    // body_start_line = 1 (1-indexed, the #[test] line)
    assert_eq!(
        sym["body_start_line"].as_u64(),
        Some(1),
        "body_start_line should point to attribute line"
    );
    // start_line = 3 (1-indexed, the fn keyword line)
    assert_eq!(
        sym["start_line"].as_u64(),
        Some(3),
        "start_line should point to fn keyword"
    );
    // body should contain both attribute and fn
    let body = sym["body"].as_str().unwrap();
    assert!(
        body.starts_with("#[test]"),
        "body should start with attribute"
    );
}

/// find_symbol without include_body should NOT have body_start_line.
#[tokio::test]
async fn find_symbol_no_body_start_line_without_include_body() {
    let src = "#[test]\nfn target() {}\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("target", 1, 1, 0, file)])
    })
    .await;

    let result = FindSymbol
        .call(
            json!({
                "name_path": "target",
                "path": "src/lib.rs"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let sym = &result["symbols"][0];
    assert!(
        sym.get("body").is_none(),
        "body should not be present without include_body"
    );
    assert!(
        sym.get("body_start_line").is_none(),
        "body_start_line should not be present without include_body"
    );
}

// ── BUG-010: insert_code "before" must walk past #[attr] and /// doc lines ────

/// `insert_code(position="before")` targeting a struct with a leading doc comment
/// and `#[derive]` attribute must insert the code BEFORE the `///` comment, not
/// between the attribute and the struct declaration.
#[tokio::test]
async fn insert_code_before_walks_past_attributes_and_doc_comments() {
    // File layout (0-indexed):
    //  0: "/// A useful struct."  <- doc comment
    //  1: "#[derive(Clone)]"      <- attribute
    //  2: "pub struct Foo {"      <- LSP start_line points here
    //  3: "    x: u32,"
    //  4: "}"
    //  5: ""
    //  6: "const SENTINEL: &str = \"survives\";"
    let src = "/// A useful struct.\n#[derive(Clone)]\npub struct Foo {\n    x: u32,\n}\n\nconst SENTINEL: &str = \"survives\";\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports start_line=2 (struct declaration), not 0 (doc comment) — BUG-010
        MockLspClient::new().with_symbols(file.clone(), vec![sym("Foo", 2, 4, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "Foo",
                "position": "before",
                "code": "const BEFORE: u32 = 1;\n"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    let before_pos = result.find("BEFORE").unwrap();
    let doc_pos = result.find("/// A useful").unwrap();
    let derive_pos = result.find("#[derive").unwrap();
    assert!(
        before_pos < doc_pos,
        "const must be inserted before the doc comment, got:\n{result}"
    );
    assert!(
        before_pos < derive_pos,
        "const must be inserted before #[derive], got:\n{result}"
    );
    assert!(
        result.contains("const SENTINEL"),
        "sentinel must survive; got:\n{result}"
    );
}

// ── insert_code: trust LSP start/end ─────────────────────────────────────────

/// With "trust LSP", start_line=0 means insert_code(before) inserts at line 0.
/// No lead-in skipping — find_insert_before_line starts from sym.start_line directly.
/// The `}` at line 0 is NOT skipped; insertion lands before everything.
#[tokio::test]
async fn insert_code_before_trusts_lsp_start() {
    // File layout (0-indexed):
    //  0: "    }"          ← LSP says start_line=0 — trust it, insert before line 0
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
    // With "trust LSP", insertion at sym.start_line=0 lands before the `}`
    let insert_pos = result.find("// inserted").unwrap();
    let brace_pos = result.find("    }").unwrap();
    assert!(
        insert_pos < brace_pos,
        "with trust LSP, insertion at sym.start_line=0 lands before `}}`; got:\n{result}"
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

/// BUG-023 regression: when LSP over-extends end_line to the next function's opening
/// line, editing_end_line() caps it to the AST-reported end, so insertion lands
/// between the closing `}` and the next function — NOT inside the next function body.
#[tokio::test]
async fn insert_code_after_caps_overextended_lsp_end() {
    // File layout (0-indexed):
    //  0: "fn target() {"
    //  1: "    body();"
    //  2: "}"
    //  3: "fn following() {"   ← LSP over-extends target's end_line to here
    //  4: "    inside();"
    //  5: "}"
    let src = "fn target() {\n    body();\n}\nfn following() {\n    inside();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=3 (over-extended into fn following() { line)
        MockLspClient::new().with_symbols(file.clone(), vec![sym("target", 0, 3, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "position": "after",
                "code": "// inserted\n"
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
    // editing_end_line caps to AST end (line 2 = closing `}`),
    // so insertion goes after line 2 — between `}` and `fn following()`.
    let insert_pos = result.find("// inserted").unwrap();
    let following_fn = result.find("fn following()").unwrap();
    assert!(
        insert_pos < following_fn,
        "insertion should land before fn following(), not inside it; got:\n{result}"
    );
}

/// BUG-016 regression: insert_code(after) on a nested `mod tests` function
/// where LSP reports end_line as a line *inside* the body (truncated range).
/// validate_symbol_range catches ast_end > sym.end_line and returns
/// RecoverableError — the file is never corrupted.
#[tokio::test]
async fn insert_code_after_rejects_truncated_end_in_nested_fn() {
    // File layout (0-indexed):
    //  0: "#[cfg(test)]"
    //  1: "mod tests {"
    //  2: "    #[test]"
    //  3: "    fn target_test() {"
    //  4: "        let x = 1;"         <- LSP (wrongly) reports end_line here
    //  5: "        assert_eq!(x, 1);"
    //  6: "    }"                       <- true end (AST knows this)
    //  7: "}"
    let src =
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn target_test() {\n        let x = 1;\n        assert_eq!(x, 1);\n    }\n}\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=4 (inside the body), not 6 (closing `}`)
        let inner = SymbolInfo {
            name: "target_test".to_string(),
            name_path: "tests/target_test".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 3,
            end_line: 4, // truncated — true end is line 6
            start_col: 4,
            children: vec![],
            range_start_line: None,
            detail: None,
        };
        let module = SymbolInfo {
            name: "tests".to_string(),
            name_path: "tests".to_string(),
            kind: SymbolKind::Module,
            file: file.clone(),
            start_line: 1,
            end_line: 7,
            start_col: 0,
            children: vec![inner],
            range_start_line: None,
            detail: None,
        };
        MockLspClient::new().with_symbols(file, vec![module])
    })
    .await;

    let result = InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "tests/target_test",
                "position": "after",
                "code": "    #[test]\n    fn new_test() {}\n"
            }),
            &ctx,
        )
        .await;

    // validate_symbol_range must catch ast_end (6) > sym.end_line (4)
    // and return RecoverableError — not silently insert mid-body
    let err = result.expect_err("should fail with RecoverableError for truncated end_line");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("suspicious range"),
        "error should mention suspicious range; got: {msg}"
    );
}

// ── remove_symbol: trust LSP ranges ──────────────────────────────────────────

/// BUG-024 regression (remove_symbol): when LSP over-extends `end_line` to include a
/// sibling `const`, editing_end_line() caps to the AST-reported end so the const survives.
#[tokio::test]
async fn remove_symbol_caps_overextended_lsp_end() {
    // File layout (0-indexed):
    //  0: "fn target() {"
    //  1: "    // body"
    //  2: "}"
    //  3: "const SENTINEL: &str = \"survives\";"  <- LSP over-extends end_line here
    let src = "fn target() {\n    // body\n}\nconst SENTINEL: &str = \"survives\";\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=3 (over-extended to const line — true end is line 2)
        MockLspClient::new().with_symbols(file.clone(), vec![sym("target", 0, 3, file)])
    })
    .await;

    RemoveSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        !result.contains("fn target"),
        "function must be removed; got:\n{result}"
    );
    // editing_end_line caps to AST end (line 2) — SENTINEL is outside the range and survives
    assert!(
        result.contains("SENTINEL"),
        "SENTINEL must survive — it is outside the true symbol range; got:\n{result}"
    );
}

/// When `range_start_line` is set (documentSymbol path), remove_symbol uses it
/// to include attributes and doc comments in the removal range.
#[tokio::test]
async fn remove_symbol_uses_range_start_line_to_include_doc_comment() {
    // File layout (0-indexed):
    //  0: "fn preceding() {"
    //  1: "    // body"
    //  2: "}"
    //  3: "use std::fmt;"
    //  4: ""                                   <- blank line
    //  5: "/// A constant."                    <- range.start = 5
    //  6: "const TARGET: bool = false;"        <- selectionRange.start = 6, end = 6
    //  7: "fn following() {}"
    let src = "fn preceding() {\n    // body\n}\nuse std::fmt;\n\n/// A constant.\nconst TARGET: bool = false;\nfn following() {}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // range_start=5 includes the doc comment
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("TARGET", 6, 6, 5, file)])
    })
    .await;

    RemoveSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "TARGET"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        !result.contains("TARGET"),
        "const must be removed; got:\n{result}"
    );
    assert!(
        !result.contains("A constant"),
        "doc comment included in range_start_line — must also be removed; got:\n{result}"
    );
    assert!(
        result.contains("fn preceding()"),
        "preceding function must survive; got:\n{result}"
    );
    assert!(
        result.contains("fn following()"),
        "following function must survive; got:\n{result}"
    );
    let use_count = result.matches("use std::fmt;").count();
    assert_eq!(
        use_count, 1,
        "use import must not be duplicated; found {use_count} occurrences in:\n{result}"
    );
}

/// When `range_start_line` is `None` (workspace/symbol or tree-sitter), the
/// heuristic fallback walks backwards past doc comments and attributes.
#[tokio::test]
async fn remove_symbol_heuristic_fallback_includes_doc_comment() {
    let src = "fn preceding() {\n    // body\n}\nuse std::fmt;\n\n/// A constant.\nconst TARGET: bool = false;\nfn following() {}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // No range_start_line — triggers heuristic fallback
        MockLspClient::new().with_symbols(file.clone(), vec![sym("TARGET", 6, 6, file)])
    })
    .await;

    RemoveSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "TARGET"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        !result.contains("TARGET"),
        "const must be removed; got:\n{result}"
    );
    assert!(
        !result.contains("A constant"),
        "heuristic should walk back past doc comment; got:\n{result}"
    );
    assert!(
        result.contains("fn preceding()"),
        "preceding function must survive; got:\n{result}"
    );
    assert!(
        result.contains("fn following()"),
        "following function must survive; got:\n{result}"
    );
}

/// When `range_start_line` explicitly excludes the doc comment, trust it.
#[tokio::test]
async fn remove_symbol_range_start_line_excludes_doc_comment() {
    let src = "fn preceding() {\n    // body\n}\nuse std::fmt;\n\n/// A constant.\nconst TARGET: bool = false;\nfn following() {}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // range_start=6 (same as selectionRange) — LSP explicitly says no doc comment
        MockLspClient::new()
            .with_symbols(file.clone(), vec![sym_with_range("TARGET", 6, 6, 6, file)])
    })
    .await;

    RemoveSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "TARGET"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        !result.contains("TARGET"),
        "const must be removed; got:\n{result}"
    );
    assert!(
        result.contains("A constant"),
        "doc comment outside explicit range_start_line — survives; got:\n{result}"
    );
}

// ── find_symbol: name_path exact match (BUG-011) ──────────────────────────────

/// Searching by name_path must return only the exact symbol, not child symbols
/// whose name_path happens to contain the query as a substring.
///
/// Regression for BUG-011: `collect_matching` used `contains()`, so a Variable
/// child with name_path "my_fn/local_var" matched a query for "my_fn".
#[tokio::test]
async fn find_symbol_name_path_does_not_return_local_variable_children() {
    use codescout::lsp::SymbolKind;

    let src = "fn my_fn() {\n    let local_var = 1;\n}\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        let child = SymbolInfo {
            name: "local_var".to_string(),
            name_path: "my_fn/local_var".to_string(),
            kind: SymbolKind::Variable,
            file: file.clone(),
            start_line: 1,
            end_line: 1,
            start_col: 4,
            children: vec![],
            range_start_line: None,
            detail: None,
        };
        let parent = SymbolInfo {
            name: "my_fn".to_string(),
            name_path: "my_fn".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 2,
            start_col: 0,
            children: vec![child],
            range_start_line: None,
            detail: None,
        };
        MockLspClient::new().with_symbols(file, vec![parent])
    })
    .await;

    let result = FindSymbol
        .call(
            json!({
                "name_path": "my_fn",
                "path": "src/lib.rs"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let symbols = result["symbols"].as_array().unwrap();
    assert_eq!(
        symbols.len(),
        1,
        "name_path lookup must return exactly the matching symbol, not its Variable children; got: {symbols:?}"
    );
    assert_eq!(symbols[0]["name"], "my_fn");
}

// ── goto_definition: no-identifier fallback (BUG-012) ─────────────────────────

/// When `identifier` is omitted, `goto_definition` must use the first
/// non-whitespace column of the line, not error with "identifier not found".
///
/// Regression for BUG-012: the old code called `str::find(ident)` which returned
/// `None` for nearly every call, causing a 100% error rate.
#[tokio::test]
async fn goto_definition_unknown_identifier_falls_back_to_first_nonwhitespace() {
    // Line 0 (1-indexed: line 1) has 4 spaces of indent before "let".
    // First non-whitespace column = 4.
    let src = "    let foo = 1;\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let def_path = root.join("src/lib.rs");
        // Configure a definition at exactly (line=0, col=4) — the expected column.
        // If the tool uses any other column (e.g. 0), the mock returns [] and
        // the tool fails with "no definition found" instead of the expected result.
        MockLspClient::new().with_definitions(
            0,
            4,
            vec![lsp_types::Location {
                uri: url::Url::from_file_path(&def_path)
                    .unwrap()
                    .as_str()
                    .parse()
                    .unwrap(),
                range: lsp_types::Range {
                    start: lsp_types::Position {
                        line: 0,
                        character: 4,
                    },
                    end: lsp_types::Position {
                        line: 0,
                        character: 7,
                    },
                },
            }],
        )
    })
    .await;

    let result = GotoDefinition
        .call(
            json!({
                "path": "src/lib.rs",
                "line": 1
                // no "identifier" — must fall back to first-nonwhitespace column
            }),
            &ctx,
        )
        .await
        .expect("should succeed: omitting identifier must not cause 'identifier not found'");

    let defs = result["definitions"].as_array().unwrap();
    assert_eq!(
        defs.len(),
        1,
        "mock should return the pre-configured definition at col=4; \
         if col is wrong the mock returns [] and the tool errors instead"
    );
}

// ── replace_symbol: language-agnostic (BUG-019 regression) ───────────────────
//
// The old `is_valid_symbol_start_line` guard had a Rust-only keyword allowlist
// (`fn `, `pub `, `struct `, `impl `, …). Any LSP start_line whose content did not
// match was rejected as "symbol location appears stale", breaking replace_symbol
// for every non-Rust language.
//
// Fix: the guard was removed. `validate_symbol_range` (AST cross-check) is the
// canonical staleness defense and is language-agnostic.
//
// Each test below is a sandwich regression:
//   Baseline  — Rust `fn` continues to work (covered by replace_symbol_clean_start_line).
//   Stale     — the language's function keyword was NOT in the old Rust allowlist, so
//               is_valid_symbol_start_line would have returned false and ReplaceSymbol
//               would have returned Err("symbol location appears stale").
//   Fixed     — ReplaceSymbol now succeeds and the new body appears in the file.

/// Python: `def` was not in the Rust keyword allowlist → old code rejected it.
#[tokio::test]
async fn replace_symbol_works_for_python() {
    // 0: "def greet():"
    // 1: "    return 'old'"
    let src = "def greet():\n    return 'old'\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.py", src)], |root| {
        let file = root.join("greet.py");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 1, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.py", "name_path": "greet",
                    "new_body": "def greet():\n    return 'new'" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.py")).unwrap();
    assert!(
        result.contains("'new'"),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("'old'"),
        "old body must be gone; got:\n{result}"
    );
}

/// TypeScript: `function` was not in the Rust keyword allowlist → old code rejected it.
#[tokio::test]
async fn replace_symbol_works_for_typescript() {
    // 0: "function greet(): string {"
    // 1: "    return 'old';"
    // 2: "}"
    let src = "function greet(): string {\n    return 'old';\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.ts", src)], |root| {
        let file = root.join("greet.ts");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.ts", "name_path": "greet",
                    "new_body": "function greet(): string {\n    return 'new';\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.ts")).unwrap();
    assert!(
        result.contains("'new'"),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("'old'"),
        "old body must be gone; got:\n{result}"
    );
}

/// JavaScript: `function` keyword → same Rust-allowlist rejection as TypeScript.
#[tokio::test]
async fn replace_symbol_works_for_javascript() {
    let src = "function greet() {\n    return 'old';\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.js", src)], |root| {
        let file = root.join("greet.js");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.js", "name_path": "greet",
                    "new_body": "function greet() {\n    return 'new';\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.js")).unwrap();
    assert!(
        result.contains("'new'"),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("'old'"),
        "old body must be gone; got:\n{result}"
    );
}

/// Go: `func` was not in the Rust keyword allowlist → old code rejected it.
#[tokio::test]
async fn replace_symbol_works_for_go() {
    let src = "func Greet() string {\n\treturn \"old\"\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.go", src)], |root| {
        let file = root.join("greet.go");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("Greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.go", "name_path": "Greet",
                    "new_body": "func Greet() string {\n\treturn \"new\"\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.go")).unwrap();
    assert!(
        result.contains("\"new\""),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("\"old\""),
        "old body must be gone; got:\n{result}"
    );
}

/// Java: `public` at the start of a method was not in the allowlist → rejected.
/// (Note: `pub ` and `pub(` were in the allowlist but not `public `.)
#[tokio::test]
async fn replace_symbol_works_for_java() {
    let src = "public String greet() {\n    return \"old\";\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("Greet.java", src)], |root| {
        let file = root.join("Greet.java");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "Greet.java", "name_path": "greet",
                    "new_body": "public String greet() {\n    return \"new\";\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("Greet.java")).unwrap();
    assert!(
        result.contains("\"new\""),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("\"old\""),
        "old body must be gone; got:\n{result}"
    );
}

/// Kotlin: `fun` was not in the Rust keyword allowlist → old code rejected it.
#[tokio::test]
async fn replace_symbol_works_for_kotlin() {
    let src = "fun greet(): String {\n    return \"old\"\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("Greet.kt", src)], |root| {
        let file = root.join("Greet.kt");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "Greet.kt", "name_path": "greet",
                    "new_body": "fun greet(): String {\n    return \"new\"\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("Greet.kt")).unwrap();
    assert!(
        result.contains("\"new\""),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("\"old\""),
        "old body must be gone; got:\n{result}"
    );
}

/// C: return-type-first signatures were not in the Rust keyword allowlist → rejected.
#[tokio::test]
async fn replace_symbol_works_for_c() {
    let src = "int greet() {\n    return 0;\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.c", src)], |root| {
        let file = root.join("greet.c");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.c", "name_path": "greet",
                    "new_body": "int greet() {\n    return 1;\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.c")).unwrap();
    assert!(
        result.contains("return 1"),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("return 0"),
        "old body must be gone; got:\n{result}"
    );
}

/// C++: same as C — return-type-first → rejected by old allowlist.
#[tokio::test]
async fn replace_symbol_works_for_cpp() {
    let src = "std::string greet() {\n    return \"old\";\n}\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.cpp", src)], |root| {
        let file = root.join("greet.cpp");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.cpp", "name_path": "greet",
                    "new_body": "std::string greet() {\n    return \"new\";\n}" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.cpp")).unwrap();
    assert!(
        result.contains("\"new\""),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("\"old\""),
        "old body must be gone; got:\n{result}"
    );
}

/// Ruby: `def` (without parens) was not in the Rust keyword allowlist → rejected.
#[tokio::test]
async fn replace_symbol_works_for_ruby() {
    // Ruby methods end with `end`, not `}`
    // 0: "def greet"
    // 1: "  'old'"
    // 2: "end"
    let src = "def greet\n  'old'\nend\n";
    let (dir, ctx) = ctx_with_mock(&[("greet.rb", src)], |root| {
        let file = root.join("greet.rb");
        MockLspClient::new().with_symbols(file.clone(), vec![sym("greet", 0, 2, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({ "path": "greet.rb", "name_path": "greet",
                    "new_body": "def greet\n  'new'\nend" }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("greet.rb")).unwrap();
    assert!(
        result.contains("'new'"),
        "new body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("'old'"),
        "old body must be gone; got:\n{result}"
    );
}

/// BUG-024 regression: replace_symbol with over-extended LSP end range must not
/// consume the next function's opening line.
#[tokio::test]
async fn replace_symbol_caps_overextended_lsp_end() {
    // File layout (0-indexed):
    //  0: "fn target() {"
    //  1: "    old_body();"
    //  2: "}"
    //  3: "fn following() {"   <- LSP over-extends target's end_line to here
    //  4: "    inside();"
    //  5: "}"
    let src = "fn target() {\n    old_body();\n}\nfn following() {\n    inside();\n}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=3 (over-extended — true end is line 2)
        MockLspClient::new().with_symbols(file.clone(), vec![sym("target", 0, 3, file)])
    })
    .await;

    ReplaceSymbol
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "new_body": "fn target() {\n    new_body();\n}"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        result.contains("fn following()"),
        "fn following() must still be present; got:\n{result}"
    );
    assert!(
        result.contains("new_body()"),
        "replacement body must be present; got:\n{result}"
    );
    assert!(
        !result.contains("old_body()"),
        "old body must be gone; got:\n{result}"
    );
    // fn following() must appear after the replacement, not be eaten by it
    let replaced_pos = result.find("new_body()").unwrap();
    let following_pos = result.find("fn following()").unwrap();
    assert!(
        following_pos > replaced_pos,
        "fn following() must come after replacement; got:\n{result}"
    );
}
