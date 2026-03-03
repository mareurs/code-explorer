//! AST tools backed by tree-sitter.

use super::{Tool, ToolContext};
use crate::ast;
use crate::lsp::symbols::SymbolKind;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ListFunctions;
pub struct ListDocs;

/// Resolve input path (relative to project root if not absolute).
async fn resolve_path(input: &Value, ctx: &ToolContext) -> anyhow::Result<PathBuf> {
    let path_str = super::require_str_param(input, "path")?;
    let project_root = ctx.agent.project_root().await;
    let security = ctx.agent.security_config().await;
    crate::util::path_security::validate_read_path(path_str, project_root.as_deref(), &security)
}

#[async_trait::async_trait]
impl Tool for ListFunctions {
    fn name(&self) -> &str {
        "list_functions"
    }
    fn description(&self) -> &str {
        "List all function/method signatures in a file using tree-sitter. \
         Works offline without a language server. Supports Rust, Python, TypeScript, Go, Java, Kotlin."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (absolute or relative to project root)"
                },
                "scope": {
                    "type": "string",
                    "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>'",
                    "default": "project"
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let path = resolve_path(&input, ctx).await?;
        let _scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        if !path.exists() {
            return Err(super::RecoverableError::with_hint(
                format!("File not found: {}", path.display()),
                "Check the path with list_dir or find_file.",
            )
            .into());
        }

        let symbols = ast::extract_symbols(&path)?;

        // Filter to functions and methods, including nested ones
        let mut functions = Vec::new();
        collect_functions(&symbols, &mut functions);

        Ok(json!({
            "file": path.display().to_string(),
            "functions": functions,
            "total": functions.len(),
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_functions(result))
    }
}

fn collect_functions(symbols: &[crate::lsp::symbols::SymbolInfo], out: &mut Vec<Value>) {
    for sym in symbols {
        match sym.kind {
            SymbolKind::Function | SymbolKind::Method => {
                let mut map = serde_json::Map::new();
                map.insert("name".into(), json!(sym.name));
                map.insert("name_path".into(), json!(sym.name_path));
                map.insert("kind".into(), json!(sym.kind));
                if let Some(sig) = &sym.detail {
                    map.insert("signature".into(), json!(sig));
                }
                map.insert("start_line".into(), json!(sym.start_line + 1));
                map.insert("end_line".into(), json!(sym.end_line + 1));
                out.push(Value::Object(map));
            }
            _ => {}
        }
        // Recurse into children (trait methods, class methods, etc.)
        collect_functions(&sym.children, out);
    }
}

#[async_trait::async_trait]
impl Tool for ListDocs {
    fn name(&self) -> &str {
        "list_docs"
    }
    fn description(&self) -> &str {
        "Extract all docstrings and top-level comments from a file using tree-sitter. \
         Returns doc comments with their associated symbol names. \
         Supports Rust (///), Python (triple-quoted), TypeScript (JSDoc), Go (//), Java, Kotlin."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (absolute or relative to project root)"
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let path = resolve_path(&input, ctx).await?;

        if !path.exists() {
            return Err(super::RecoverableError::with_hint(
                format!("File not found: {}", path.display()),
                "Check the path with list_dir or find_file.",
            )
            .into());
        }

        let docstrings = ast::extract_docstrings(&path)?;

        let results: Vec<Value> = docstrings
            .iter()
            .map(|d| {
                json!({
                    "symbol_name": d.symbol_name,
                    "content": d.content,
                    "start_line": d.start_line + 1,
                    "end_line": d.end_line + 1,
                })
            })
            .collect();

        Ok(json!({
            "file": path.display().to_string(),
            "docstrings": results,
            "total": results.len(),
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_docs(result))
    }
}

fn format_list_functions(result: &Value) -> String {
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

fn format_list_docs(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let docs = match result["docstrings"].as_array() {
        Some(d) if !d.is_empty() => d,
        _ => return format!("{file} — 0 docstrings"),
    };
    const MAX_SHOW: usize = 3;
    let total = docs.len();
    let mut out = format!("{file} — {total} docstrings");
    for entry in docs.iter().take(MAX_SHOW) {
        let symbol = entry["symbol_name"].as_str().unwrap_or("?");
        let content = entry["content"].as_str().unwrap_or("");
        let first_line = content.lines().next().unwrap_or("").trim();
        let preview = if first_line.chars().count() > 72 {
            let truncated: String = first_line.chars().take(72).collect();
            format!("{truncated}…")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use tempfile::tempdir;

    async fn project_ctx_with_file(
        filename: &str,
        content: &str,
    ) -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(dir.path().join(filename), content).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: LspManager::new_arc(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
                progress: None,
            },
        )
    }

    #[tokio::test]
    async fn list_functions_rust() {
        let source = "fn hello() {}\nfn world() {}\nstruct Foo;\nimpl Foo { fn bar(&self) {} }\n";
        let (dir, ctx) = project_ctx_with_file("test.rs", source).await;
        let result = ListFunctions
            .call(json!({ "path": "test.rs" }), &ctx)
            .await
            .unwrap();
        let total = result["total"].as_u64().unwrap();
        assert_eq!(total, 3, "expected 3 functions: {:?}", result["functions"]);
        let names: Vec<&str> = result["functions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"world"));
        assert!(names.contains(&"bar"));
        drop(dir);
    }

    #[test]
    fn list_functions_omits_source_field() {
        use crate::lsp::{SymbolInfo, SymbolKind};
        use std::path::PathBuf;

        let syms = vec![SymbolInfo {
            name: "my_fn".to_string(),
            name_path: "my_fn".to_string(),
            kind: SymbolKind::Function,
            file: PathBuf::from("src/lib.rs"),
            start_line: 0,
            end_line: 5,
            start_col: 0,
            children: vec![],
            detail: None,
        }];

        let mut out = vec![];
        collect_functions(&syms, &mut out);

        assert_eq!(out.len(), 1);
        assert!(
            out[0].get("source").is_none(),
            "collect_functions must not emit 'source' field"
        );
    }

    #[tokio::test]
    async fn list_functions_line_numbers_are_1_indexed() {
        // Verify start_line/end_line are 1-indexed in output so they can be
        // passed directly to edit_file / read_file (which are also 1-indexed).
        let source = "fn hello() {}\nfn world() {}\n";
        let (dir, ctx) = project_ctx_with_file("test.rs", source).await;
        let result = ListFunctions
            .call(json!({ "path": "test.rs" }), &ctx)
            .await
            .unwrap();
        let functions = result["functions"].as_array().unwrap();
        let hello = functions
            .iter()
            .find(|f| f["name"] == "hello")
            .expect("should find hello");
        // "fn hello() {}" is on line 1 (1-indexed), not line 0
        assert_eq!(
            hello["start_line"].as_u64().unwrap(),
            1,
            "start_line must be 1-indexed (line 1, not 0): {:?}",
            hello
        );
        drop(dir);
    }

    #[tokio::test]
    async fn list_functions_python() {
        let source = "def greet():\n    pass\n\nclass Dog:\n    def speak(self):\n        pass\n";
        let (dir, ctx) = project_ctx_with_file("test.py", source).await;
        let result = ListFunctions
            .call(json!({ "path": "test.py" }), &ctx)
            .await
            .unwrap();
        let total = result["total"].as_u64().unwrap();
        assert_eq!(total, 2, "expected 2 functions: {:?}", result["functions"]);
        drop(dir);
    }

    #[tokio::test]
    async fn list_functions_file_not_found() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let result = ListFunctions
            .call(json!({ "path": "nonexistent.rs" }), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn extract_docstrings_rust() {
        let source = "/// A greeting.\nfn hello() {}\n\n/// A point.\nstruct Point {}\n";
        let (dir, ctx) = project_ctx_with_file("test.rs", source).await;
        let result = ListDocs
            .call(json!({ "path": "test.rs" }), &ctx)
            .await
            .unwrap();
        let total = result["total"].as_u64().unwrap();
        assert_eq!(
            total, 2,
            "expected 2 docstrings: {:?}",
            result["docstrings"]
        );
        let first = &result["docstrings"][0];
        assert_eq!(first["symbol_name"].as_str(), Some("hello"));
        assert!(first["content"].as_str().unwrap().contains("greeting"));
        drop(dir);
    }

    #[tokio::test]
    async fn extract_docstrings_line_numbers_are_1_indexed() {
        // "/// A greeting." is line 1, so start_line must be 1, not 0.
        let source = "/// A greeting.\nfn hello() {}\n";
        let (dir, ctx) = project_ctx_with_file("test.rs", source).await;
        let result = ListDocs
            .call(json!({ "path": "test.rs" }), &ctx)
            .await
            .unwrap();
        let first = &result["docstrings"][0];
        assert_eq!(
            first["start_line"].as_u64().unwrap(),
            1,
            "start_line must be 1-indexed: {:?}",
            first
        );
        drop(dir);
    }

    #[tokio::test]
    async fn extract_docstrings_python() {
        let source = "def greet():\n    \"\"\"Say hello.\"\"\"\n    pass\n";
        let (dir, ctx) = project_ctx_with_file("test.py", source).await;
        let result = ListDocs
            .call(json!({ "path": "test.py" }), &ctx)
            .await
            .unwrap();
        let total = result["total"].as_u64().unwrap();
        assert!(
            total >= 1,
            "expected at least 1 docstring: {:?}",
            result["docstrings"]
        );
        let docs = result["docstrings"].as_array().unwrap();
        let greet_doc = docs
            .iter()
            .find(|d| d["symbol_name"].as_str() == Some("greet"));
        assert!(greet_doc.is_some(), "missing greet docstring");
        drop(dir);
    }

    #[tokio::test]
    async fn list_functions_unsupported_language() {
        let (dir, ctx) = project_ctx_with_file("test.txt", "some text").await;
        let result = ListFunctions
            .call(json!({ "path": "test.txt" }), &ctx)
            .await;
        // Unsupported language should return an error
        assert!(result.is_err());
        drop(dir);
    }

    #[tokio::test]
    async fn list_functions_absolute_path() {
        let source = "fn hello() {}\n";
        let (dir, ctx) = project_ctx_with_file("test.rs", source).await;
        let abs_path = dir.path().join("test.rs");
        let result = ListFunctions
            .call(json!({ "path": abs_path.display().to_string() }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["total"].as_u64().unwrap(), 1);
        drop(dir);
    }

    #[tokio::test]
    async fn list_functions_schema_includes_scope() {
        let tool = ListFunctions;
        let schema = tool.input_schema();
        assert!(schema["properties"]["scope"].is_object());
    }

    #[test]
    fn list_functions_format_compact_shows_count() {
        use serde_json::json;
        let tool = ListFunctions;
        let result = json!({ "functions": [{"name":"foo"}, {"name":"bar"}], "file": "src/a.rs" });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("2"), "got: {text}");
    }

    #[test]
    fn list_docs_format_compact_shows_count() {
        use serde_json::json;
        let tool = ListDocs;
        let result = json!({ "docstrings": [{"symbol":"Foo"}], "file": "src/a.rs" });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("1"), "got: {text}");
    }

    // --- format_list_functions tests ---

    #[test]
    fn format_list_functions_shows_names() {
        let result = serde_json::json!({
            "file": "src/tools/symbol.rs",
            "functions": [
                {"name": "collect_matching", "start_line": 100, "end_line": 140},
                {"name": "build_by_file", "start_line": 150, "end_line": 180},
                {"name": "matches_kind_filter", "start_line": 190, "end_line": 200}
            ]
        });
        let out = format_list_functions(&result);
        assert!(out.contains("src/tools/symbol.rs"), "should show file");
        assert!(
            out.contains("collect_matching"),
            "should show function name"
        );
        assert!(out.contains("build_by_file"), "should show function name");
        assert!(out.contains('3'), "should show count");
    }

    #[test]
    fn format_list_functions_caps_at_eight() {
        let funcs: Vec<serde_json::Value> = (0..12)
            .map(|i| serde_json::json!({"name": format!("func_{i}"), "start_line": i, "end_line": i + 5}))
            .collect();
        let result = serde_json::json!({ "file": "src/big.rs", "functions": funcs });
        let out = format_list_functions(&result);
        assert!(out.contains("func_0"), "should show first func");
        assert!(!out.contains("func_8"), "should not show 9th func");
        assert!(out.contains("more"), "should show trailer");
    }

    // --- format_list_docs tests ---

    #[test]
    fn format_list_docs_shows_previews() {
        let result = serde_json::json!({
            "file": "src/tools/output.rs",
            "docstrings": [
                {"symbol_name": "OutputGuard", "content": "Enforces progressive disclosure across all tools."},
                {"symbol_name": "cap_items", "content": "Truncate to exploring-mode limit and produce OverflowInfo."},
                {"symbol_name": "cap_files", "content": "File-level capping for multi-file result sets."},
                {"symbol_name": "overflow_json", "content": "Build the overflow object to include in JSON response."}
            ]
        });
        let out = format_list_docs(&result);
        assert!(out.contains("src/tools/output.rs"), "should show file");
        assert!(out.contains("OutputGuard"), "should show symbol name");
        assert!(
            out.contains("Enforces progressive"),
            "should show doc preview"
        );
        assert!(out.contains("more"), "should cap at 3");
        assert!(!out.contains("overflow_json"), "4th entry should be hidden");
    }

    #[test]
    fn format_list_docs_handles_unicode_near_boundary() {
        let long_doc: String = "a".repeat(71) + "ñ world and more text here";
        let result = serde_json::json!({
            "file": "src/lib.rs",
            "docstrings": [
                {"symbol_name": "MyStruct", "content": long_doc}
            ]
        });
        let out = format_list_docs(&result);
        assert!(out.contains("MyStruct"), "should show symbol");
    }
}
