use crate::e2e::expectations::{load_expectations, LangExpectation};
use code_explorer::agent::Agent;
use code_explorer::lsp::manager::LspManager;
use code_explorer::tools::ast::ListFunctions;
use code_explorer::tools::file::SearchPattern;
use code_explorer::tools::symbol::{FindReferences, FindSymbol, ListSymbols};
use code_explorer::tools::{Tool, ToolContext};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Root of the test fixtures directory.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Get the fixture project directory for a language.
fn fixture_dir(language: &str) -> PathBuf {
    fixtures_root().join(format!("{language}-library"))
}

/// Create a fresh ToolContext for the given language.
///
/// Each `#[tokio::test]` gets its own Tokio runtime, so we must NOT cache
/// contexts in a static — LSP clients spawned on one runtime are dead once
/// that runtime shuts down. Agent::new + LspManager::new are cheap (just
/// struct construction); LSP servers start lazily on first tool call.
async fn fixture_context(language: &str) -> Arc<ToolContext> {
    let dir = fixture_dir(language);
    assert!(
        dir.exists(),
        "Fixture directory not found: {}",
        dir.display()
    );

    let agent = Agent::new(Some(dir.clone()))
        .await
        .unwrap_or_else(|e| panic!("Failed to create Agent for {language}: {e}"));

    let lsp = LspManager::new_arc();
    Arc::new(ToolContext {
        agent,
        lsp,
        output_buffer: Arc::new(code_explorer::tools::output_buffer::OutputBuffer::new(20)),
    })
}

/// Run expectations from multiple TOML files for a language, sharing one LSP context.
///
/// This is the primary entry point: each language runs as a single `#[tokio::test]`
/// so the LSP server only starts once (important for kotlin-lsp / jdtls which
/// take 30-60s to initialize).
pub async fn run_all_expectations(language: &str, toml_filenames: &[&str]) {
    let ctx = fixture_context(language).await;
    let mut total_pass = 0;
    let mut total_failures = Vec::new();

    for toml_filename in toml_filenames {
        let toml_path = fixtures_root().join(toml_filename);
        let expectations = load_expectations(&toml_path, language);

        if expectations.is_empty() {
            total_failures.push((
                format!("{toml_filename}::<no tests>"),
                format!(
                    "No expectations found for language '{language}' in {toml_filename}. \
                     Check TOML for [{language}] sub-tables or top-level path/file."
                ),
            ));
            continue;
        }

        eprintln!("\n--- {toml_filename} ---");
        let (pass, failures) = run_expectations_inner(&ctx, &expectations).await;
        total_pass += pass;
        for (name, err) in failures {
            total_failures.push((format!("{toml_filename}::{name}"), err));
        }
    }

    let total = total_pass + total_failures.len();
    eprintln!("\n{total_pass}/{total} passed for {language}");

    if !total_failures.is_empty() {
        panic!(
            "{} of {total} expectations failed for {language}:\n{}",
            total_failures.len(),
            total_failures
                .iter()
                .map(|(n, e)| format!("  - {n}: {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// Run all expectations with a shared context, returning (pass_count, failures).
///
/// Expectations are sorted so that `find_referencing_symbols` tests run last.
/// `textDocument/references` requires the LSP to complete workspace-wide analysis,
/// which happens in the background while earlier tool calls (documentSymbol, etc.)
/// warm up the server. Running symbol/overview tests first gives the LSP time
/// to finish indexing before reference lookups execute.
async fn run_expectations_inner(
    ctx: &ToolContext,
    expectations: &[(String, LangExpectation, String)],
) -> (usize, Vec<(String, String)>) {
    // Sort: non-reference tests first, reference tests last.
    let mut sorted: Vec<_> = expectations.iter().collect();
    sorted.sort_by_key(|(_, _, tool)| {
        if tool == "find_referencing_symbols" {
            1
        } else {
            0
        }
    });

    let mut pass = 0;
    let mut failures = Vec::new();

    for (name, expectation, tool) in sorted {
        match run_single(ctx, expectation, tool).await {
            Ok(()) => {
                pass += 1;
                eprintln!("  PASS  {name}");
            }
            Err(e) => {
                eprintln!("  FAIL  {name}: {e}");
                failures.push((name.clone(), e));
            }
        }
    }

    (pass, failures)
}

/// Run a single expectation and return Ok(()) or an error message.
async fn run_single(ctx: &ToolContext, exp: &LangExpectation, tool: &str) -> Result<(), String> {
    match tool {
        "get_symbols_overview" => run_symbols_overview(ctx, exp).await,
        "find_symbol" => run_find_symbol(ctx, exp).await,
        "find_referencing_symbols" => run_find_references(ctx, exp).await,
        "list_functions" => run_list_functions(ctx, exp).await,
        "search_for_pattern" => run_search_pattern(ctx, exp).await,
        other => Err(format!("Unknown tool: {other}")),
    }
}

async fn run_symbols_overview(ctx: &ToolContext, exp: &LangExpectation) -> Result<(), String> {
    let path = exp.path.as_deref().ok_or("Missing 'path'")?;
    let result = ListSymbols
        .call(json!({ "relative_path": path }), ctx)
        .await
        .map_err(|e| format!("Tool error: {e}"))?;

    if let Some(expected) = &exp.contains_symbols {
        assert_contains_symbols(&result, expected)?;
    }
    Ok(())
}

async fn run_find_symbol(ctx: &ToolContext, exp: &LangExpectation) -> Result<(), String> {
    let symbol = exp.symbol.as_deref().ok_or("Missing 'symbol'")?;
    let mut params = json!({ "pattern": symbol });

    if let Some(path) = &exp.path {
        params["relative_path"] = json!(path);
    }

    if exp.body_contains.is_some() {
        params["include_body"] = json!(true);
    }

    let result = FindSymbol
        .call(params, ctx)
        .await
        .map_err(|e| format!("Tool error: {e}"))?;

    // Check children/contains_symbols
    if let Some(expected) = &exp.contains_symbols {
        assert_contains_symbols(&result, expected)?;
    }

    // Check body content
    if let Some(expected_body) = &exp.body_contains {
        let result_str = serde_json::to_string(&result).unwrap_or_default();
        for needle in expected_body {
            if !result_str.contains(needle) {
                return Err(format!(
                    "find_symbol(\"{symbol}\") body missing \"{needle}\""
                ));
            }
        }
    }

    Ok(())
}

async fn run_find_references(ctx: &ToolContext, exp: &LangExpectation) -> Result<(), String> {
    let symbol = exp.symbol.as_deref().ok_or("Missing 'symbol'")?;
    let file = exp.path.as_deref().ok_or("Missing 'path'/'file'")?;

    // textDocument/references requires the LSP to finish workspace-wide analysis.
    // LSP servers index in the background — early calls may return only partial
    // results (e.g., just the definition site) before cross-file analysis completes.
    // Retry until ALL expected references are found or we exhaust attempts.
    let expected = exp.expected_refs_contain.as_deref().unwrap_or(&[]);
    let mut last_result_str = String::new();
    let mut last_err: Option<String> = None;

    for attempt in 0..8 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
        }
        match FindReferences
            .call(json!({ "name_path": symbol, "relative_path": file }), ctx)
            .await
        {
            Ok(result) => {
                last_err = None;
                last_result_str = serde_json::to_string(&result).unwrap_or_default();
                // Check if ALL expected needles are present.
                let all_found = expected
                    .iter()
                    .all(|n| last_result_str.contains(n.as_str()));
                if all_found {
                    return Ok(());
                }
                // Partial results — LSP still indexing cross-file refs. Retry.
            }
            Err(e) => {
                last_err = Some(format!("Tool error: {e}"));
                // Retry on tool errors (symbol might not be found until indexed).
            }
        }
    }

    if let Some(err) = last_err {
        return Err(err);
    }

    // Report which specific needles are missing.
    for needle in expected {
        if !last_result_str.contains(needle.as_str()) {
            return Err(format!(
                "find_referencing_symbols(\"{symbol}\") missing reference to \"{needle}\""
            ));
        }
    }

    Ok(())
}

async fn run_list_functions(ctx: &ToolContext, exp: &LangExpectation) -> Result<(), String> {
    let path = exp.path.as_deref().ok_or("Missing 'path'")?;
    let result = ListFunctions
        .call(json!({ "path": path }), ctx)
        .await
        .map_err(|e| format!("Tool error: {e}"))?;

    if let Some(expected) = &exp.contains_functions {
        let result_str = serde_json::to_string(&result).unwrap_or_default();
        for needle in expected {
            if !result_str.contains(needle) {
                return Err(format!("list_functions(\"{path}\") missing \"{needle}\""));
            }
        }
    }

    Ok(())
}

async fn run_search_pattern(ctx: &ToolContext, exp: &LangExpectation) -> Result<(), String> {
    let pattern = exp.pattern.as_deref().ok_or("Missing 'pattern'")?;
    let result = SearchPattern
        .call(json!({ "pattern": pattern }), ctx)
        .await
        .map_err(|e| format!("Tool error: {e}"))?;

    if let Some(expected_files) = &exp.expected_files {
        let result_str = serde_json::to_string(&result).unwrap_or_default();
        for needle in expected_files {
            if !result_str.contains(needle) {
                return Err(format!(
                    "search_for_pattern(\"{pattern}\") missing file \"{needle}\""
                ));
            }
        }
    }

    Ok(())
}

/// Check that expected symbol names appear somewhere in the JSON result.
fn assert_contains_symbols(result: &Value, expected: &[String]) -> Result<(), String> {
    let result_str = serde_json::to_string(result).unwrap_or_default();
    let mut missing = Vec::new();
    for name in expected {
        if !result_str.contains(name.as_str()) {
            missing.push(name.as_str());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("Missing symbols: {:?}", missing))
    }
}
