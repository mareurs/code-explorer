//! Regression tests for LSP-backed symbol tools using a mock LSP client.
//!
//! These tests verify the "trust LSP" file-splice logic without requiring a live
//! language server. The mock returns pre-configured symbol positions that reproduce
//! LSP range quirks (over-extension, degenerate ranges, lead-in artifacts).

use code_explorer::agent::Agent;
use code_explorer::lsp::{MockLspClient, MockLspProvider, SymbolInfo, SymbolKind};
use code_explorer::tools::symbol::{InsertCode, RemoveSymbol, ReplaceSymbol};
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

// ── BUG-019: stale LSP line number guard ─────────────────────────────────────

/// When the file has been modified (e.g. lines inserted above) without notifying
/// the LSP, the LSP returns stale line numbers that point into the middle of a
/// function body rather than a declaration. `replace_symbol` must detect this and
/// return a RecoverableError instead of silently corrupting the file.
#[tokio::test]
async fn replace_symbol_rejects_stale_lsp_start_line() {
    // File layout (0-indexed):
    //  0: "fn preamble() {"
    //  1: "    let x = 1;"
    //  2: "    let result = compute();"   ← stale LSP start_line for "target"
    //  3: "}"
    //  4: ""
    //  5: "fn target() {"
    //  6: "    old_body();"
    //  7: "}"
    //
    // Simulates: "target" was originally at line 2 (before extra lines were
    // inserted above it), but the LSP was never notified of the change.
    let src = "fn preamble() {\n    let x = 1;\n    let result = compute();\n}\n\nfn target() {\n    old_body();\n}\n";

    let (_dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        MockLspClient::new().with_symbols(
            file.clone(),
            // Stale: "target" reported at line 2 (inside preamble's body)
            vec![sym("target", 2, 7, file)],
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
        msg.contains("stale"),
        "expected stale LSP error, got: {msg}"
    );

    // File must be untouched — the original content must survive
    let content = std::fs::read_to_string(_dir.path().join("src/lib.rs")).unwrap();
    assert!(
        content.contains("old_body()"),
        "file must be unmodified after stale guard; got:\n{content}"
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

/// With "trust LSP", end_line=3 means insert_code(after) inserts at line 4.
/// If LSP over-extends, the insertion lands after the overextended range.
#[tokio::test]
async fn insert_code_after_trusts_lsp_end() {
    // File layout (0-indexed):
    //  0: "    fn target() {"
    //  1: "        body();"
    //  2: "    }"
    //  3: "    fn following() {"  ← LSP over-extends target's end_line here
    //  4: "        inside();"
    //  5: "    }"
    let src =
        "    fn target() {\n        body();\n    }\n    fn following() {\n        inside();\n    }\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=3 (the `fn following() {` line) — trust LSP, insert after line 3
        MockLspClient::new().with_symbols(file.clone(), vec![sym("target", 0, 3, file)])
    })
    .await;

    InsertCode
        .call(
            json!({
                "path": "src/lib.rs",
                "name_path": "target",
                "position": "after",
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
    // With "trust LSP", insert after line 3 (end_line+1=4)
    // Insertion lands after `fn following() {` — past the overextended end
    let insert_pos = result.find("// inserted").unwrap();
    let following_fn = result.find("fn following()").unwrap();
    assert!(
        insert_pos > following_fn,
        "with trust LSP, insertion after overextended end lands past fn following; got:\n{result}"
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

/// With "trust LSP" design, when LSP over-extends `end_line` to include a
/// sibling `const`, we trust it — the const is removed along with the function.
/// This is an LSP inaccuracy, not a bug in remove_symbol. The agent can verify
/// via find_symbol(include_body=true) to see the same overextended range.
#[tokio::test]
async fn remove_symbol_trusts_lsp_range_even_when_overextended() {
    // File layout (0-indexed):
    //  0: "fn target() {"
    //  1: "    // body"
    //  2: "}"
    //  3: "const SENTINEL: &str = \"survives\";"  <- LSP over-extends end_line here
    let src = "fn target() {\n    // body\n}\nconst SENTINEL: &str = \"survives\";\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
        // LSP reports end_line=3 (the const line) — trust LSP, remove lines 0-3
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
    // With "trust LSP", the const is within the LSP range and is removed too
    assert!(
        !result.contains("fn target"),
        "function must be removed; got:\n{result}"
    );
    assert!(
        !result.contains("SENTINEL"),
        "const is within LSP range — removed too (trust LSP); got:\n{result}"
    );
}

/// With "trust LSP", a `const` item is removed using the raw LSP range (line 6 only).
/// Doc comments are NOT removed unless the LSP includes them in the range.
/// No more panic/corruption risk from missing closing brace.
#[tokio::test]
async fn remove_symbol_const_trusts_lsp_range() {
    // File layout (0-indexed):
    //  0: "fn preceding() {"
    //  1: "    // body"
    //  2: "}"
    //  3: "use std::fmt;"
    //  4: ""                                   <- blank line
    //  5: "/// A constant."
    //  6: "const TARGET: bool = false;"        <- LSP says start=6, end=6
    //  7: "fn following() {}"
    let src = "fn preceding() {\n    // body\n}\nuse std::fmt;\n\n/// A constant.\nconst TARGET: bool = false;\nfn following() {}\n";

    let (dir, ctx) = ctx_with_mock(&[("src/lib.rs", src)], |root| {
        let file = root.join("src/lib.rs");
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
        result.contains("A constant"),
        "doc comment is outside LSP range — survives (trust LSP); got:\n{result}"
    );
    assert!(
        result.contains("fn preceding()"),
        "preceding function must survive; got:\n{result}"
    );
    assert!(
        result.contains("fn following()"),
        "following function must survive; got:\n{result}"
    );
    // Count occurrences of "use std::fmt;" — must be exactly 1 (no duplication)
    let use_count = result.matches("use std::fmt;").count();
    assert_eq!(
        use_count, 1,
        "use import must not be duplicated; found {use_count} occurrences in:\n{result}"
    );
}
