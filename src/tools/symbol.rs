//! Symbol-level tools backed by the LSP client.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde_json::{json, Value};

use super::{Tool, ToolContext};
use crate::ast;
use crate::lsp::SymbolInfo;

/// Resolve a relative path against the project root.
async fn resolve_path(ctx: &ToolContext, relative_path: &str) -> anyhow::Result<PathBuf> {
    let root = ctx.agent.require_project_root().await?;
    let full = root.join(relative_path);
    if !full.exists() {
        anyhow::bail!("path not found: {}", full.display());
    }
    Ok(full)
}

/// Detect language from path and get an LSP client, or error if unavailable.
///
/// Returns `(client, lsp_language_id)` where `lsp_language_id` is the identifier
/// expected by `textDocument/didOpen` (e.g. `"typescriptreact"` for `.tsx` files).
async fn get_lsp_client(
    ctx: &ToolContext,
    path: &Path,
) -> anyhow::Result<(std::sync::Arc<crate::lsp::LspClient>, String)> {
    let lang = ast::detect_language(path)
        .ok_or_else(|| anyhow!("cannot detect language for: {:?}", path))?;
    let root = ctx.agent.require_project_root().await?;
    let client = ctx.lsp.get_or_start(lang, &root).await?;
    let language_id = crate::lsp::servers::lsp_language_id(lang);
    Ok((client, language_id.to_string()))
}

/// Convert a `SymbolInfo` tree to JSON with optional body inclusion.
fn symbol_to_json(
    sym: &SymbolInfo,
    include_body: bool,
    source: Option<&str>,
    depth: usize,
) -> Value {
    let mut obj = json!({
        "name": sym.name,
        "name_path": sym.name_path,
        "kind": format!("{:?}", sym.kind),
        "file": sym.file.display().to_string(),
        "start_line": sym.start_line,
        "end_line": sym.end_line,
    });

    if include_body {
        if let Some(src) = source {
            let lines: Vec<&str> = src.lines().collect();
            let start = sym.start_line as usize;
            let end = (sym.end_line as usize + 1).min(lines.len());
            if start < lines.len() {
                obj["body"] = json!(lines[start..end].join("\n"));
            }
        }
    }

    if depth > 0 && !sym.children.is_empty() {
        obj["children"] = json!(sym
            .children
            .iter()
            .map(|c| symbol_to_json(c, include_body, source, depth - 1))
            .collect::<Vec<_>>());
    }

    obj
}

// ── get_symbols_overview ───────────────────────────────────────────────────

pub struct GetSymbolsOverview;

#[async_trait::async_trait]
impl Tool for GetSymbolsOverview {
    fn name(&self) -> &str {
        "get_symbols_overview"
    }
    fn description(&self) -> &str {
        "Return a tree of symbols (functions, classes, methods, etc.) in a file or directory. \
         Uses LSP for accurate results."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "relative_path": { "type": "string", "description": "File or directory path relative to project root" },
                "depth": { "type": "integer", "default": 1, "description": "Depth of children to include (0=none, 1=direct children)" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let rel_path = input["relative_path"].as_str().unwrap_or(".");
        let depth = input["depth"].as_u64().unwrap_or(1) as usize;

        let full_path = resolve_path(ctx, rel_path).await?;

        if full_path.is_file() {
            let (client, lang) = get_lsp_client(ctx, &full_path).await?;
            let symbols = client.document_symbols(&full_path, &lang).await?;
            let source = std::fs::read_to_string(&full_path).ok();
            let json_symbols: Vec<Value> = symbols
                .iter()
                .map(|s| symbol_to_json(s, false, source.as_deref(), depth))
                .collect();
            Ok(json!({ "file": rel_path, "symbols": json_symbols }))
        } else if full_path.is_dir() {
            // Aggregate symbols from all files in directory
            let mut result = vec![];
            let walker = ignore::WalkBuilder::new(&full_path)
                .max_depth(Some(1))
                .build();
            for entry in walker.flatten() {
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let path = entry.path();
                let Some(lang) = ast::detect_language(path) else {
                    continue;
                };
                let root = ctx.agent.require_project_root().await?;
                let language_id = crate::lsp::servers::lsp_language_id(lang);
                if let Ok(client) = ctx.lsp.get_or_start(lang, &root).await {
                    if let Ok(symbols) = client.document_symbols(path, language_id).await {
                        let rel = path.strip_prefix(&root).unwrap_or(path);
                        let json_symbols: Vec<Value> = symbols
                            .iter()
                            .map(|s| symbol_to_json(s, false, None, depth.saturating_sub(1)))
                            .collect();
                        result.push(json!({
                            "file": rel.display().to_string(),
                            "symbols": json_symbols,
                        }));
                    }
                }
            }
            Ok(json!({ "directory": rel_path, "files": result }))
        } else {
            Err(anyhow!(
                "path is neither file nor directory: {}",
                full_path.display()
            ))
        }
    }
}

// ── find_symbol ────────────────────────────────────────────────────────────

pub struct FindSymbol;

#[async_trait::async_trait]
impl Tool for FindSymbol {
    fn name(&self) -> &str {
        "find_symbol"
    }
    fn description(&self) -> &str {
        "Find symbols by name pattern across the project. Returns matching symbols with location."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Symbol name or substring to search for" },
                "relative_path": { "type": "string", "description": "Restrict search to this file" },
                "include_body": { "type": "boolean", "default": false },
                "depth": { "type": "integer", "default": 0, "description": "Depth of children to include" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'pattern' parameter"))?;
        let include_body = input["include_body"].as_bool().unwrap_or(false);
        let depth = input["depth"].as_u64().unwrap_or(0) as usize;

        let root = ctx.agent.require_project_root().await?;

        // If a file path is given, search within that file only
        let files: Vec<PathBuf> = if let Some(rel) = input["relative_path"].as_str() {
            vec![root.join(rel)]
        } else {
            // Walk project for supported files
            let mut files = vec![];
            let walker = ignore::WalkBuilder::new(&root).build();
            for entry in walker.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && ast::detect_language(entry.path()).is_some()
                {
                    files.push(entry.path().to_path_buf());
                }
            }
            files
        };

        let pattern_lower = pattern.to_lowercase();
        let mut matches = vec![];

        for file_path in &files {
            let Some(lang) = ast::detect_language(file_path) else {
                continue;
            };
            let language_id = crate::lsp::servers::lsp_language_id(lang);
            let Ok(client) = ctx.lsp.get_or_start(lang, &root).await else {
                continue;
            };
            let Ok(symbols) = client.document_symbols(file_path, language_id).await else {
                continue;
            };

            let source = if include_body {
                std::fs::read_to_string(file_path).ok()
            } else {
                None
            };

            fn collect_matching(
                symbols: &[SymbolInfo],
                pattern: &str,
                include_body: bool,
                source: Option<&str>,
                depth: usize,
                out: &mut Vec<Value>,
            ) {
                for sym in symbols {
                    if sym.name.to_lowercase().contains(pattern) {
                        out.push(symbol_to_json(sym, include_body, source, depth));
                    }
                    collect_matching(&sym.children, pattern, include_body, source, depth, out);
                }
            }

            collect_matching(
                &symbols,
                &pattern_lower,
                include_body,
                source.as_deref(),
                depth,
                &mut matches,
            );
        }

        Ok(json!({ "symbols": matches, "total": matches.len() }))
    }
}

// ── find_referencing_symbols ───────────────────────────────────────────────

pub struct FindReferencingSymbols;

#[async_trait::async_trait]
impl Tool for FindReferencingSymbols {
    fn name(&self) -> &str {
        "find_referencing_symbols"
    }
    fn description(&self) -> &str {
        "Find all locations that reference the given symbol. \
         Requires the symbol's file and name_path to locate it."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path"],
            "properties": {
                "name_path": { "type": "string", "description": "Symbol name path (e.g. 'MyStruct/my_method')" },
                "relative_path": { "type": "string", "description": "File containing the symbol" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = input["relative_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'relative_path'"))?;

        let full_path = resolve_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Find the symbol's position by walking document symbols
        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        // Get references at the symbol's position
        let refs = client
            .references(&full_path, sym.start_line, sym.start_col, &lang)
            .await?;

        let root = ctx.agent.require_project_root().await?;
        let locations: Vec<Value> = refs
            .iter()
            .map(|loc| {
                let file = uri_to_path(loc.uri.as_str())
                    .and_then(|p| p.strip_prefix(&root).ok().map(|r| r.to_path_buf()))
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| loc.uri.as_str().to_string());

                // Read context lines around the reference
                let context = uri_to_path(loc.uri.as_str())
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|src| {
                        let lines: Vec<&str> = src.lines().collect();
                        let line = loc.range.start.line as usize;
                        lines.get(line).unwrap_or(&"").to_string()
                    })
                    .unwrap_or_default();

                json!({
                    "file": file,
                    "line": loc.range.start.line + 1,
                    "column": loc.range.start.character,
                    "context": context,
                })
            })
            .collect();

        Ok(json!({ "references": locations, "total": locations.len() }))
    }
}

// ── replace_symbol_body ────────────────────────────────────────────────────

pub struct ReplaceSymbolBody;

#[async_trait::async_trait]
impl Tool for ReplaceSymbolBody {
    fn name(&self) -> &str {
        "replace_symbol_body"
    }
    fn description(&self) -> &str {
        "Replace the entire body of a named symbol with new source code."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "new_body"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "new_body": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = input["relative_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'relative_path'"))?;
        let new_body = input["new_body"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'new_body'"))?;

        let full_path = resolve_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let start = sym.start_line as usize;
        let end = (sym.end_line as usize + 1).min(lines.len());

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.push(new_body);
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        Ok(json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) }))
    }
}

// ── insert_before_symbol / insert_after_symbol ─────────────────────────────

pub struct InsertBeforeSymbol;

#[async_trait::async_trait]
impl Tool for InsertBeforeSymbol {
    fn name(&self) -> &str {
        "insert_before_symbol"
    }
    fn description(&self) -> &str {
        "Insert code immediately before a named symbol."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "code"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "code": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = input["relative_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'relative_path'"))?;
        let code = input["code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'code'"))?;

        let full_path = resolve_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let insert_at = sym.start_line as usize;

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..insert_at]);
        new_lines.push(code);
        new_lines.extend_from_slice(&lines[insert_at..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1 }))
    }
}

pub struct InsertAfterSymbol;

#[async_trait::async_trait]
impl Tool for InsertAfterSymbol {
    fn name(&self) -> &str {
        "insert_after_symbol"
    }
    fn description(&self) -> &str {
        "Insert code immediately after a named symbol."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "code"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "code": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = input["relative_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'relative_path'"))?;
        let code = input["code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'code'"))?;

        let full_path = resolve_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let insert_at = (sym.end_line as usize + 1).min(lines.len());

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..insert_at]);
        new_lines.push(code);
        new_lines.extend_from_slice(&lines[insert_at..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1 }))
    }
}

// ── rename_symbol ──────────────────────────────────────────────────────────

pub struct RenameSymbol;

#[async_trait::async_trait]
impl Tool for RenameSymbol {
    fn name(&self) -> &str {
        "rename_symbol"
    }
    fn description(&self) -> &str {
        "Rename a symbol across the entire codebase using LSP."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "new_name"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "new_name": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = input["relative_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'relative_path'"))?;
        let new_name = input["new_name"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'new_name'"))?;

        let full_path = resolve_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Find the symbol to get its position
        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        // Request rename from LSP
        let edit = client
            .rename(&full_path, sym.start_line, sym.start_col, new_name, &lang)
            .await?;

        // Apply workspace edit
        let mut files_changed = 0;
        let mut total_edits = 0;

        if let Some(changes) = &edit.changes {
            for (uri, edits) in changes {
                let Some(path) = uri_to_path(uri.as_str()) else {
                    continue;
                };
                let content = std::fs::read_to_string(&path)?;
                let new_content = apply_text_edits(&content, edits);
                std::fs::write(&path, new_content)?;
                files_changed += 1;
                total_edits += edits.len();
            }
        }

        if let Some(doc_changes) = &edit.document_changes {
            let operations: Vec<&lsp_types::DocumentChangeOperation> = match doc_changes {
                lsp_types::DocumentChanges::Edits(edits) => {
                    // Convert TextDocumentEdits to DocumentChangeOperations for uniform handling
                    // Just process them directly instead
                    for text_edit in edits {
                        let Some(path) = uri_to_path(text_edit.text_document.uri.as_str()) else {
                            continue;
                        };
                        let content = std::fs::read_to_string(&path)?;
                        let plain_edits: Vec<lsp_types::TextEdit> = text_edit
                            .edits
                            .iter()
                            .map(|e| match e {
                                lsp_types::OneOf::Left(te) => te.clone(),
                                lsp_types::OneOf::Right(ate) => ate.text_edit.clone(),
                            })
                            .collect();
                        let new_content = apply_text_edits(&content, &plain_edits);
                        std::fs::write(&path, new_content)?;
                        files_changed += 1;
                        total_edits += text_edit.edits.len();
                    }
                    vec![]
                }
                lsp_types::DocumentChanges::Operations(ops) => ops.iter().collect(),
            };
            for change in operations {
                if let lsp_types::DocumentChangeOperation::Edit(text_edit) = change {
                    let Some(path) = uri_to_path(text_edit.text_document.uri.as_str()) else {
                        continue;
                    };
                    let content = std::fs::read_to_string(&path)?;
                    let plain_edits: Vec<lsp_types::TextEdit> = text_edit
                        .edits
                        .iter()
                        .map(|e| match e {
                            lsp_types::OneOf::Left(te) => te.clone(),
                            lsp_types::OneOf::Right(ate) => ate.text_edit.clone(),
                        })
                        .collect();
                    let new_content = apply_text_edits(&content, &plain_edits);
                    std::fs::write(&path, new_content)?;
                    files_changed += 1;
                    total_edits += text_edit.edits.len();
                }
            }
        }

        Ok(json!({
            "status": "ok",
            "old_name": name_path.rsplit('/').next().unwrap_or(name_path),
            "new_name": new_name,
            "files_changed": files_changed,
            "total_edits": total_edits,
        }))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Write lines back to a file, preserving a trailing newline if the original had one.
fn write_lines(
    path: &std::path::Path,
    lines: &[&str],
    had_trailing_newline: bool,
) -> std::io::Result<()> {
    let mut out = lines.join("\n");
    if had_trailing_newline {
        out.push('\n');
    }
    std::fs::write(path, out)
}

/// Walk the symbol tree to find a symbol by name_path (e.g. "MyStruct/my_method").
fn find_symbol_by_name_path<'a>(
    symbols: &'a [SymbolInfo],
    name_path: &str,
) -> Option<&'a SymbolInfo> {
    for sym in symbols {
        if sym.name_path == name_path || sym.name == name_path {
            return Some(sym);
        }
        if let Some(found) = find_symbol_by_name_path(&sym.children, name_path) {
            return Some(found);
        }
    }
    None
}

/// Convert a `file://` URI to a filesystem path.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

/// Apply LSP TextEdits to a source string, returning the modified version.
///
/// Edits are applied from bottom to top to preserve line numbers.
fn apply_text_edits(content: &str, edits: &[lsp_types::TextEdit]) -> String {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    // Ensure trailing newline is preserved
    if content.ends_with('\n') {
        lines.push(String::new());
    }

    // Sort edits bottom-to-top so earlier edits don't shift later positions
    let mut sorted: Vec<&lsp_types::TextEdit> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });

    for edit in sorted {
        let start_line = edit.range.start.line as usize;
        let start_char = edit.range.start.character as usize;
        let end_line = edit.range.end.line as usize;
        let end_char = edit.range.end.character as usize;

        if start_line >= lines.len() {
            continue;
        }

        // Build the new content: prefix + new_text + suffix
        let prefix = if start_char <= lines[start_line].len() {
            &lines[start_line][..start_char]
        } else {
            &lines[start_line]
        };

        let suffix = if end_line < lines.len() {
            if end_char <= lines[end_line].len() {
                &lines[end_line][end_char..]
            } else {
                ""
            }
        } else {
            ""
        };

        let replacement = format!("{}{}{}", prefix, edit.new_text, suffix);
        let replacement_lines: Vec<String> = replacement.lines().map(|s| s.to_string()).collect();

        // Remove old lines and insert new ones
        let remove_end = (end_line + 1).min(lines.len());
        lines.splice(start_line..remove_end, replacement_lines);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use crate::tools::ToolContext;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn lsp() -> Arc<LspManager> {
        Arc::new(LspManager::new())
    }

    /// Create a test Cargo project and return the context.
    async fn rust_project_ctx() -> Option<(tempfile::TempDir, ToolContext)> {
        if !std::process::Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return None;
        }

        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            r#"fn main() {
    println!("hello");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}
"#,
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        Some((dir, ToolContext { agent, lsp: lsp() }))
    }

    #[tokio::test]
    async fn get_symbols_overview_returns_symbols() {
        let Some((_dir, ctx)) = rust_project_ctx().await else {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        };

        let result = GetSymbolsOverview
            .call(
                json!({
                    "relative_path": "src/main.rs",
                    "depth": 1
                }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(!symbols.is_empty(), "should find at least one symbol");

        // Should find main, add, Point
        let names: Vec<&str> = symbols
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"main"),
            "should find main function, got: {:?}",
            names
        );
        assert!(
            names.contains(&"add"),
            "should find add function, got: {:?}",
            names
        );

        ctx.lsp.shutdown_all().await;
    }

    #[tokio::test]
    async fn find_symbol_by_name() {
        let Some((_dir, ctx)) = rust_project_ctx().await else {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        };

        let result = FindSymbol
            .call(
                json!({
                    "pattern": "add",
                    "relative_path": "src/main.rs"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(!symbols.is_empty(), "should find 'add' symbol");
        assert!(symbols.iter().any(|s| s["name"].as_str() == Some("add")));

        ctx.lsp.shutdown_all().await;
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
        };
        assert!(GetSymbolsOverview
            .call(json!({"relative_path": "x"}), &ctx)
            .await
            .is_err());
        assert!(FindSymbol
            .call(json!({"pattern": "x"}), &ctx)
            .await
            .is_err());
        assert!(FindReferencingSymbols
            .call(json!({"name_path": "x", "relative_path": "y"}), &ctx)
            .await
            .is_err());
    }

    #[test]
    fn apply_text_edits_simple_replacement() {
        let content = "hello world\nfoo bar\nbaz\n";
        let edits = vec![lsp_types::TextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 6,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 11,
                },
            },
            new_text: "rust".to_string(),
        }];
        let result = apply_text_edits(content, &edits);
        assert!(result.starts_with("hello rust"), "got: {}", result);
    }

    #[test]
    fn uri_to_path_strips_prefix() {
        let p = uri_to_path("file:///home/user/code.rs").unwrap();
        assert_eq!(p, PathBuf::from("/home/user/code.rs"));
    }

    #[test]
    fn find_symbol_in_tree() {
        let symbols = vec![SymbolInfo {
            name: "Foo".into(),
            name_path: "Foo".into(),
            kind: crate::lsp::SymbolKind::Struct,
            file: PathBuf::from("test.rs"),
            start_line: 0,
            end_line: 5,
            start_col: 0,
            children: vec![SymbolInfo {
                name: "bar".into(),
                name_path: "Foo/bar".into(),
                kind: crate::lsp::SymbolKind::Method,
                file: PathBuf::from("test.rs"),
                start_line: 2,
                end_line: 4,
                start_col: 4,
                children: vec![],
            }],
        }];

        assert!(find_symbol_by_name_path(&symbols, "Foo").is_some());
        assert!(find_symbol_by_name_path(&symbols, "Foo/bar").is_some());
        assert!(find_symbol_by_name_path(&symbols, "nonexistent").is_none());
    }
}
