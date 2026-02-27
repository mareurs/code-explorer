//! AST tools backed by tree-sitter.

use super::{Tool, ToolContext};
use crate::ast;
use crate::lsp::symbols::SymbolKind;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ListFunctions;
pub struct ExtractDocstrings;

/// Resolve input path (relative to project root if not absolute).
async fn resolve_path(input: &Value, ctx: &ToolContext) -> anyhow::Result<PathBuf> {
    let path_str = input["path"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'path' parameter"))?;
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
         Works offline without a language server. Supports Rust, Python, TypeScript, Go."
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
            anyhow::bail!("File not found: {}", path.display());
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
}

fn collect_functions(symbols: &[crate::lsp::symbols::SymbolInfo], out: &mut Vec<Value>) {
    for sym in symbols {
        match sym.kind {
            SymbolKind::Function | SymbolKind::Method => {
                out.push(json!({
                    "name": sym.name,
                    "name_path": sym.name_path,
                    "kind": sym.kind,
                    "start_line": sym.start_line + 1,
                    "end_line": sym.end_line + 1,
                    "source": "project",
                }));
            }
            _ => {}
        }
        // Recurse into children (trait methods, class methods, etc.)
        collect_functions(&sym.children, out);
    }
}

#[async_trait::async_trait]
impl Tool for ExtractDocstrings {
    fn name(&self) -> &str {
        "list_docs"
    }
    fn description(&self) -> &str {
        "Extract all docstrings and top-level comments from a file using tree-sitter. \
         Returns doc comments with their associated symbol names. \
         Supports Rust (///), Python (triple-quoted), TypeScript (JSDoc), Go (//)."
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
            anyhow::bail!("File not found: {}", path.display());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::sync::Arc;
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
                lsp: Arc::new(LspManager::new()),
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

    #[tokio::test]
    async fn list_functions_line_numbers_are_1_indexed() {
        // Verify start_line/end_line are 1-indexed in output so they can be
        // passed directly to edit_lines (which is also 1-indexed).
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
            lsp: Arc::new(LspManager::new()),
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
        let result = ExtractDocstrings
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
        let result = ExtractDocstrings
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
        let result = ExtractDocstrings
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
}
