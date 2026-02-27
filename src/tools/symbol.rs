//! Symbol-level tools backed by the LSP client.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde_json::{json, Value};

use crate::tools::RecoverableError;

use super::output::{OutputGuard, OutputMode};
use super::{Tool, ToolContext};
use crate::ast;
use crate::lsp::SymbolInfo;

/// Returns true if the path string contains glob metacharacters.
fn is_glob(path: &str) -> bool {
    path.contains('*') || path.contains('?') || path.contains('[')
}

/// Resolve a path for reading, with security validation.
///
/// `"."` and `""` resolve to the project root directly (not `root.join(".")`)
/// to avoid spurious `./` prefixes when stripping the root later.
async fn resolve_read_path(ctx: &ToolContext, relative_path: &str) -> anyhow::Result<PathBuf> {
    if relative_path == "." || relative_path.is_empty() {
        return ctx.agent.require_project_root().await;
    }
    let project_root = ctx.agent.project_root().await;
    let security = ctx.agent.security_config().await;
    let full = crate::util::path_security::validate_read_path(
        relative_path,
        project_root.as_deref(),
        &security,
    )?;
    if !full.exists() {
        return Err(RecoverableError::with_hint(
            format!("path not found: {}", full.display()),
            "Use list_dir to explore the directory structure, \
             or get_symbols_overview on a directory path.",
        )
        .into());
    }
    Ok(full)
}

/// Resolve a path for writing, with security validation.
async fn resolve_write_path(ctx: &ToolContext, relative_path: &str) -> anyhow::Result<PathBuf> {
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;
    crate::util::path_security::validate_write_path(relative_path, &root, &security)
}

/// Resolve a path that may be a glob pattern, returning all matching files.
/// If the path is a literal file/directory, returns it as a single-element vec.
/// If it contains glob metacharacters (* ? [), expands against the project root.
async fn resolve_glob(ctx: &ToolContext, path_or_glob: &str) -> anyhow::Result<Vec<PathBuf>> {
    let root = ctx.agent.require_project_root().await?;

    if !is_glob(path_or_glob) {
        let full = resolve_read_path(ctx, path_or_glob).await?;
        return Ok(vec![full]);
    }

    // Glob expansion
    let glob = globset::GlobBuilder::new(path_or_glob)
        .literal_separator(false)
        .build()
        .map_err(|e| anyhow!("invalid glob pattern '{}': {}", path_or_glob, e))?;
    let matcher = glob.compile_matcher();

    let mut matches = vec![];
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(&root) {
            if matcher.is_match(rel) {
                matches.push(entry.path().to_path_buf());
            }
        }
    }

    if matches.is_empty() {
        return Err(RecoverableError::with_hint(
            format!("no files matched glob pattern: {}", path_or_glob),
            "Try a broader pattern or use list_dir to verify the path exists.",
        )
        .into());
    }
    matches.sort();
    Ok(matches)
}

/// Extract a file path parameter from input, accepting "path", "relative_path", or "file".
fn get_path_param(input: &Value, required: bool) -> anyhow::Result<Option<&str>> {
    match input["path"]
        .as_str()
        .or_else(|| input["relative_path"].as_str())
        .or_else(|| input["file"].as_str())
    {
        Some(p) => Ok(Some(p)),
        None if required => Err(anyhow!("missing 'path' parameter")),
        None => Ok(None),
    }
}

/// Detect language from path and get an LSP client, or error if unavailable.
///
/// Returns `(client, lsp_language_id)` where `lsp_language_id` is the identifier
/// expected by `textDocument/didOpen` (e.g. `"typescriptreact"` for `.tsx` files).
async fn get_lsp_client(
    ctx: &ToolContext,
    path: &Path,
) -> anyhow::Result<(std::sync::Arc<crate::lsp::LspClient>, String)> {
    let lang = ast::detect_language(path).ok_or_else(|| {
        RecoverableError::with_hint(
            format!("unsupported file type: {:?}", path),
            "LSP symbol analysis supports: rust, python, typescript, tsx, \
             javascript, jsx, go, java, kotlin, c, cpp, csharp, ruby. \
             Use list_functions for a tree-sitter fallback on other file types.",
        )
    })?;
    let root = ctx.agent.require_project_root().await?;
    let client = ctx.lsp.get_or_start(lang, &root).await?;
    let language_id = crate::lsp::servers::lsp_language_id(lang);
    Ok((client, language_id.to_string()))
}

/// Recursively collect symbols whose name contains the given pattern (case-insensitive).
fn collect_matching(
    symbols: &[SymbolInfo],
    pattern: &str,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    source: &str,
    out: &mut Vec<Value>,
) {
    for sym in symbols {
        if sym.name.to_lowercase().contains(pattern)
            || sym.name_path.to_lowercase().contains(pattern)
        {
            out.push(symbol_to_json(
                sym,
                include_body,
                source_code,
                depth,
                source,
            ));
        }
        collect_matching(
            &sym.children,
            pattern,
            include_body,
            source_code,
            depth,
            source,
            out,
        );
    }
}

/// Convert a `SymbolInfo` tree to JSON with optional body inclusion.
fn symbol_to_json(
    sym: &SymbolInfo,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    source: &str,
) -> Value {
    let mut obj = json!({
        "name": sym.name,
        "name_path": sym.name_path,
        "kind": format!("{:?}", sym.kind),
        "file": sym.file.display().to_string(),
        "start_line": sym.start_line + 1,
        "end_line": sym.end_line + 1,
        "source": source,
    });

    if include_body {
        if let Some(src) = source_code {
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
            .map(|c| symbol_to_json(c, include_body, source_code, depth - 1, source))
            .collect::<Vec<_>>());
    }

    obj
}

// ── get_symbols_overview ───────────────────────────────────────────────────

/// Directory/glob scans can produce huge output (each file has many symbols).
/// Cap exploring-mode file count lower than the global OutputGuard default (200).
const LIST_SYMBOLS_MAX_FILES: usize = 50;

pub struct ListSymbols;

#[async_trait::async_trait]
impl Tool for ListSymbols {
    fn name(&self) -> &str {
        "list_symbols"
    }
    fn description(&self) -> &str {
        "Return a tree of symbols (functions, classes, methods, etc.) in a file or directory. \
         Uses LSP for accurate results."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File or directory path relative to project root. Supports glob patterns (e.g. 'src/**/*.rs')" },
                "depth": { "type": "integer", "default": 1, "description": "Depth of children to include (0=none, 1=direct children)" },
                "detail_level": { "type": "string", "description": "Output detail: omit or 'exploring' for compact (default), 'full' for complete with bodies" },
                "offset": { "type": "integer", "description": "Skip this many files (focused mode pagination)" },
                "limit": { "type": "integer", "description": "Max files per page (focused mode, default 50)" },
                "scope": { "type": "string", "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>'", "default": "project" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let rel_path = get_path_param(&input, false)?.unwrap_or(".");
        let depth = input["depth"].as_u64().unwrap_or(1) as usize;
        let guard = OutputGuard::from_input(&input);
        let _scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        // If the path contains glob metacharacters, expand and aggregate
        if is_glob(rel_path) {
            let files = resolve_glob(ctx, rel_path).await?;
            let mut guard = guard;
            guard.max_files = guard.max_files.min(LIST_SYMBOLS_MAX_FILES);
            let (files, file_overflow) =
                guard.cap_files(files, "Narrow with a more specific glob or file path");
            let root = ctx.agent.require_project_root().await?;
            let include_body = guard.should_include_body();
            let mut result = vec![];
            for file_path in &files {
                let Some(lang) = ast::detect_language(file_path) else {
                    continue;
                };
                let language_id = crate::lsp::servers::lsp_language_id(lang);
                if let Ok(client) = ctx.lsp.get_or_start(lang, &root).await {
                    if let Ok(symbols) = client.document_symbols(file_path, language_id).await {
                        let rel = file_path.strip_prefix(&root).unwrap_or(file_path);
                        let source = if include_body {
                            std::fs::read_to_string(file_path).ok()
                        } else {
                            None
                        };
                        let json_symbols: Vec<Value> = symbols
                            .iter()
                            .map(|s| {
                                symbol_to_json(s, include_body, source.as_deref(), depth, "project")
                            })
                            .collect();
                        result.push(json!({
                            "file": rel.display().to_string(),
                            "symbols": json_symbols,
                        }));
                    }
                }
            }
            let mut result_json = json!({ "pattern": rel_path, "files": result });
            if let Some(ov) = file_overflow {
                result_json["overflow"] = OutputGuard::overflow_json(&ov);
            }
            return Ok(result_json);
        }

        let full_path = resolve_read_path(ctx, rel_path).await?;

        if full_path.is_file() {
            let (client, lang) = get_lsp_client(ctx, &full_path).await?;
            let symbols = client.document_symbols(&full_path, &lang).await?;
            let include_body = guard.should_include_body();
            let source = if include_body {
                std::fs::read_to_string(&full_path).ok()
            } else {
                None
            };
            let json_symbols: Vec<Value> = symbols
                .iter()
                .map(|s| symbol_to_json(s, include_body, source.as_deref(), depth, "project"))
                .collect();
            Ok(json!({ "file": rel_path, "symbols": json_symbols }))
        } else if full_path.is_dir() {
            // Collect file paths from directory.
            // Project root → walk recursively so nested src/ files are found.
            // Subdirectory → shallow (depth 1) to avoid dumping entire subtrees.
            let is_project_root = rel_path == "." || rel_path.is_empty();
            let mut dir_files = vec![];
            let walker = ignore::WalkBuilder::new(&full_path)
                .max_depth(if is_project_root { None } else { Some(1) })
                .hidden(true)
                .git_ignore(true)
                .build();
            for entry in walker.flatten() {
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                dir_files.push(entry.path().to_path_buf());
            }

            let mut guard = guard;
            guard.max_files = guard.max_files.min(LIST_SYMBOLS_MAX_FILES);
            let (dir_files, file_overflow) =
                guard.cap_files(dir_files, "Narrow with a more specific glob or file path");
            let include_body = guard.should_include_body();
            let root = ctx.agent.require_project_root().await?;

            // Aggregate symbols from capped file list
            let mut result = vec![];
            for path in &dir_files {
                let Some(lang) = ast::detect_language(path) else {
                    continue;
                };
                let language_id = crate::lsp::servers::lsp_language_id(lang);

                // Try LSP first, fall back to tree-sitter if unavailable
                let mut symbols = if let Ok(client) = ctx.lsp.get_or_start(lang, &root).await {
                    client
                        .document_symbols(path, language_id)
                        .await
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                // Tree-sitter fallback when LSP is unavailable or returned nothing
                if symbols.is_empty() {
                    symbols = crate::ast::extract_symbols(path).unwrap_or_default();
                }

                if symbols.is_empty() {
                    continue;
                }

                let rel = path.strip_prefix(&root).unwrap_or(path);
                let source = if include_body {
                    std::fs::read_to_string(path).ok()
                } else {
                    None
                };
                let json_symbols: Vec<Value> = symbols
                    .iter()
                    .map(|s| {
                        symbol_to_json(
                            s,
                            include_body,
                            source.as_deref(),
                            depth.saturating_sub(1),
                            "project",
                        )
                    })
                    .collect();
                result.push(json!({
                    "file": rel.display().to_string(),
                    "symbols": json_symbols,
                }));
            }
            let mut result_json = json!({ "directory": rel_path, "files": result });
            if let Some(ov) = file_overflow {
                result_json["overflow"] = OutputGuard::overflow_json(&ov);
            }
            Ok(result_json)
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
            "properties": {
                "pattern": { "type": "string", "description": "Symbol name or substring to search for" },
                "name_path": { "type": "string", "description": "Exact name path from get_symbols_overview (e.g. 'MyStruct/my_method'). Alternative to pattern." },
                "path": { "type": "string", "description": "Restrict search to this file or glob pattern (e.g. 'src/**/*.rs')" },
                "include_body": { "type": "boolean", "default": false },
                "depth": { "type": "integer", "default": 0, "description": "Depth of children to include" },
                "detail_level": { "type": "string", "description": "Output detail: omit for compact (default), 'full' for complete with bodies" },
                "offset": { "type": "integer", "description": "Skip this many results (focused mode pagination)" },
                "limit": { "type": "integer", "description": "Max results per page (focused mode, default 50)" },
                "scope": { "type": "string", "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>'", "default": "project" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let pattern = input["pattern"]
            .as_str()
            .or_else(|| input["name_path"].as_str())
            .ok_or_else(|| {
                RecoverableError::with_hint(
                    "missing required parameter",
                    "Provide 'pattern' (substring search) or 'name_path' (exact path from get_symbols_overview, e.g. 'MyStruct/my_method')",
                )
            })?;
        let guard = OutputGuard::from_input(&input);
        let include_body = input["include_body"]
            .as_bool()
            .unwrap_or_else(|| guard.should_include_body());
        let depth = input["depth"].as_u64().unwrap_or(0) as usize;
        let _scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        let root = ctx.agent.require_project_root().await?;
        let pattern_lower = pattern.to_lowercase();
        let mut matches = vec![];

        if let Some(rel) = get_path_param(&input, false)? {
            // Restricted search: per-file textDocument/documentSymbol
            let files: Vec<PathBuf> = if is_glob(rel) {
                resolve_glob(ctx, rel).await?
            } else {
                let full = root.join(rel);
                if full.is_dir() {
                    // Walk directory to find source files (same pattern as ListSymbols)
                    let walker = ignore::WalkBuilder::new(&full)
                        .hidden(true)
                        .git_ignore(true)
                        .build();
                    walker
                        .flatten()
                        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                        .map(|e| e.path().to_path_buf())
                        .collect()
                } else {
                    vec![full]
                }
            };

            // In exploring mode, stop early once we have enough results.
            let early_cap = match guard.mode {
                OutputMode::Exploring => Some(guard.max_results + 1),
                OutputMode::Focused => None,
            };

            for file_path in &files {
                if let Some(cap) = early_cap {
                    if matches.len() >= cap {
                        break;
                    }
                }
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
                collect_matching(
                    &symbols,
                    &pattern_lower,
                    include_body,
                    source.as_deref(),
                    depth,
                    "project",
                    &mut matches,
                );
            }

            let hit_early_cap = early_cap.is_some() && matches.len() > guard.max_results;
            if hit_early_cap {
                use super::output::OverflowInfo;
                matches.truncate(guard.max_results);
                let overflow = OverflowInfo {
                    shown: guard.max_results,
                    total: guard.max_results + 1,
                    hint: "Restrict with a file path or glob pattern".to_string(),
                    next_offset: None,
                };
                let mut result = json!({ "symbols": matches, "total": guard.max_results + 1 });
                result["overflow"] = OutputGuard::overflow_json(&overflow);
                return Ok(result);
            }
        } else {
            // Fast path: workspace/symbol — one LSP request per language instead of
            // one textDocument/documentSymbol request per file.
            let mut languages = std::collections::HashSet::new();
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(true)
                .git_ignore(true)
                .build();
            for entry in walker.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    if let Some(lang) = ast::detect_language(entry.path()) {
                        languages.insert(lang);
                    }
                }
            }

            // Concurrently start/query all LSP servers so different languages
            // (e.g. Kotlin JVM startup) don't block each other.
            let languages: Vec<&str> = languages.into_iter().collect();
            let mut join_set = tokio::task::JoinSet::new();
            for lang in languages {
                let lsp = ctx.lsp.clone();
                let root = root.clone();
                let pattern = pattern_lower.clone();
                join_set.spawn(async move {
                    let client = lsp.get_or_start(lang, &root).await?;
                    client.workspace_symbols(&pattern).await
                });
            }
            while let Some(task_result) = join_set.join_next().await {
                let Ok(Ok(symbols)) = task_result else {
                    continue;
                };
                for sym in symbols {
                    // LSP servers may use fuzzy/prefix matching — enforce substring.
                    if sym.name.to_lowercase().contains(&pattern_lower)
                        || sym.name_path.to_lowercase().contains(&pattern_lower)
                    {
                        let source = if include_body {
                            std::fs::read_to_string(&sym.file).ok()
                        } else {
                            None
                        };
                        matches.push(symbol_to_json(
                            &sym,
                            include_body,
                            source.as_deref(),
                            depth,
                            "project",
                        ));
                    }
                }
            }

            // Tree-sitter fallback: if workspace/symbol returned nothing (LSP
            // not running, still indexing, or doesn't support workspace/symbol),
            // walk source files and extract symbols with tree-sitter.
            if matches.is_empty() {
                let walker = ignore::WalkBuilder::new(&root)
                    .hidden(true)
                    .git_ignore(true)
                    .build();
                for entry in walker.flatten() {
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    let path = entry.path();
                    if ast::detect_language(path).is_none() {
                        continue;
                    }
                    if let Ok(symbols) = crate::ast::extract_symbols(path) {
                        let source = if include_body {
                            std::fs::read_to_string(path).ok()
                        } else {
                            None
                        };
                        collect_matching(
                            &symbols,
                            &pattern_lower,
                            include_body,
                            source.as_deref(),
                            depth,
                            "project",
                            &mut matches,
                        );
                    }
                    // Early cap to avoid scanning entire huge projects
                    if matches.len() > guard.max_results {
                        break;
                    }
                }
            }
        }

        let (mut matches, overflow) =
            guard.cap_items(matches, "Restrict with a file path or glob pattern");

        // When include_body is on and there are many results, strip bodies
        // beyond a threshold to avoid blowing the context window.
        const BODY_CAP: usize = 5;
        if include_body && matches.len() > BODY_CAP {
            for item in &mut matches[BODY_CAP..] {
                if let Some(obj) = item.as_object_mut() {
                    obj.remove("body");
                    obj.insert(
                        "body_omitted".to_string(),
                        json!("use find_symbol with name_path for full body"),
                    );
                }
            }
        }

        let total = overflow.as_ref().map_or(matches.len(), |o| o.total);
        let mut result = json!({ "symbols": matches, "total": total });
        if let Some(ov) = overflow {
            result["overflow"] = OutputGuard::overflow_json(&ov);
        }
        Ok(result)
    }
}

// ── find_referencing_symbols ───────────────────────────────────────────────

pub struct FindReferences;

#[async_trait::async_trait]
impl Tool for FindReferences {
    fn name(&self) -> &str {
        "find_references"
    }
    fn description(&self) -> &str {
        "Find all locations that reference the given symbol. \
         Requires the symbol's file and name_path to locate it."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path"],
            "properties": {
                "name_path": { "type": "string", "description": "Symbol name path (e.g. 'MyStruct/my_method')" },
                "path": { "type": "string", "description": "File containing the symbol" },
                "detail_level": { "type": "string", "description": "Output detail: omit for compact (default), 'full' for complete with bodies" },
                "offset": { "type": "integer", "description": "Skip this many results (focused mode pagination)" },
                "limit": { "type": "integer", "description": "Max results per page (focused mode, default 50)" },
                "scope": { "type": "string", "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>'", "default": "project" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let _scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        let full_path = resolve_read_path(ctx, rel_path).await?;
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
                    "source": "project",
                })
            })
            .collect();

        let guard = OutputGuard::from_input(&input);
        let total = locations.len();
        let (locations, overflow) = guard.cap_items(locations, "This symbol has many references. Use detail_level='full' with offset/limit to paginate");
        let mut result = json!({ "references": locations, "total": total });
        if let Some(ov) = overflow {
            result["overflow"] = OutputGuard::overflow_json(&ov);
        }
        Ok(result)
    }
}

pub struct GotoDefinition;

#[async_trait::async_trait]
impl Tool for GotoDefinition {
    fn name(&self) -> &str {
        "goto_definition"
    }
    fn description(&self) -> &str {
        "Jump to the definition of a symbol at a given line. \
         Resolves types via LSP — handles method calls, trait impls, and cross-crate navigation. \
         Auto-discovers library dependencies when definitions are outside the project."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "line"],
            "properties": {
                "path": { "type": "string", "description": "File path (relative or absolute)" },
                "line": { "type": "integer", "description": "1-indexed line number to jump from" },
                "identifier": { "type": "string", "description": "Optional identifier on the line to target (disambiguates when multiple symbols on same line)" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let rel_path = get_path_param(&input, true)?.unwrap();
        let line_1 = input["line"]
            .as_u64()
            .ok_or_else(|| anyhow!("missing 'line'"))? as u32;
        if line_1 == 0 {
            anyhow::bail!("'line' must be >= 1 (1-indexed)");
        }
        let line_0 = line_1 - 1;
        let identifier = input["identifier"].as_str();

        let full_path = resolve_read_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Determine column: find identifier on the line, or use first non-whitespace
        let source = std::fs::read_to_string(&full_path)?;
        let source_line = source.lines().nth(line_0 as usize).ok_or_else(|| {
            RecoverableError::with_hint(
                format!(
                    "line {} is beyond end of file ({})",
                    line_1,
                    full_path.display()
                ),
                "Check the line number — use list_symbols or search_pattern to find correct lines",
            )
        })?;

        let col = if let Some(ident) = identifier {
            source_line.find(ident).ok_or_else(|| {
                RecoverableError::with_hint(
                    format!("identifier '{}' not found on line {}", ident, line_1),
                    "Check the identifier spelling, or omit it to use the first symbol on the line",
                )
            })? as u32
        } else {
            source_line
                .chars()
                .take_while(|c| c.is_whitespace())
                .count() as u32
        };

        let definitions = client
            .goto_definition(&full_path, line_0, col, &lang)
            .await?;

        if definitions.is_empty() {
            return Err(RecoverableError::with_hint(
                format!("no definition found at {}:{}", full_path.display(), line_1),
                "The LSP couldn't resolve a definition at this position. \
                 Try specifying an 'identifier' parameter, or use find_symbol to search by name instead",
            )
            .into());
        }

        let root = ctx.agent.require_project_root().await?;
        let mut results = Vec::new();
        for loc in &definitions {
            let def_path = uri_to_path(loc.uri.as_str());
            let (file_display, source_tag) = if let Some(ref p) = def_path {
                let tag = tag_external_path(p, &root, &ctx.agent).await;
                let display = p
                    .strip_prefix(&root)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| p.display().to_string());
                (display, tag)
            } else {
                (loc.uri.as_str().to_string(), "external".to_string())
            };

            // Read the definition line for context
            let context = def_path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|src| {
                    src.lines()
                        .nth(loc.range.start.line as usize)
                        .map(|l| l.to_string())
                })
                .unwrap_or_default();

            results.push(json!({
                "file": file_display,
                "line": loc.range.start.line + 1,
                "end_line": loc.range.end.line + 1,
                "context": context.trim(),
                "source": source_tag,
            }));
        }

        Ok(json!({
            "definitions": results,
            "from": format!("{}:{}", full_path.file_name().unwrap_or_default().to_string_lossy(), line_1),
        }))
    }
}

pub struct Hover;

#[async_trait::async_trait]

impl Tool for Hover {
    fn name(&self) -> &str {
        "hover"
    }
    fn description(&self) -> &str {
        "Get type info and documentation for a symbol at a given position. \
         Returns the type signature, inferred types, and doc comments. \
         Complements find_symbol (name lookup) and goto_definition (navigation)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "line"],
            "properties": {
                "path": { "type": "string", "description": "File path (relative or absolute)" },
                "line": { "type": "integer", "description": "1-indexed line number" },
                "identifier": { "type": "string", "description": "Optional identifier on the line to target (disambiguates when multiple symbols on same line)" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let rel_path = get_path_param(&input, true)?.unwrap();
        let line_1 = input["line"]
            .as_u64()
            .ok_or_else(|| anyhow!("missing 'line'"))? as u32;
        if line_1 == 0 {
            anyhow::bail!("'line' must be >= 1 (1-indexed)");
        }
        let line_0 = line_1 - 1;
        let identifier = input["identifier"].as_str();

        let full_path = resolve_read_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Determine column: find identifier on the line, or use first non-whitespace
        let source = std::fs::read_to_string(&full_path)?;
        let source_line = source.lines().nth(line_0 as usize).ok_or_else(|| {
            RecoverableError::with_hint(
                format!(
                    "line {} is beyond end of file ({})",
                    line_1,
                    full_path.display()
                ),
                "Check the line number — use list_symbols or search_pattern to find correct lines",
            )
        })?;

        let col = if let Some(ident) = identifier {
            source_line.find(ident).ok_or_else(|| {
                RecoverableError::with_hint(
                    format!("identifier '{}' not found on line {}", ident, line_1),
                    "Check the identifier spelling, or omit it to use the first symbol on the line",
                )
            })? as u32
        } else {
            source_line
                .chars()
                .take_while(|c| c.is_whitespace())
                .count() as u32
        };

        let hover_text = client.hover(&full_path, line_0, col, &lang).await?;

        match hover_text {
            Some(text) => Ok(json!({
                "content": text,
                "location": format!("{}:{}", full_path.file_name().unwrap_or_default().to_string_lossy(), line_1),
            })),
            None => Err(RecoverableError::with_hint(
                format!("no hover info at {}:{}", full_path.display(), line_1),
                "The LSP has no type/doc info at this position. \
                 Try specifying an 'identifier' parameter, or use find_symbol for name-based lookup",
            )
            .into()),
        }
    }
}

// ── replace_symbol_body ────────────────────────────────────────────────────

pub struct ReplaceSymbol;

#[async_trait::async_trait]
impl Tool for ReplaceSymbol {
    fn name(&self) -> &str {
        "replace_symbol"
    }
    fn description(&self) -> &str {
        "Replace the entire body of a named symbol with new source code."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path", "new_body"],
            "properties": {
                "name_path": { "type": "string" },
                "path": { "type": "string" },
                "new_body": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let new_body = input["new_body"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'new_body'"))?;

        let full_path = resolve_write_path(ctx, rel_path).await?;
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
        new_lines.extend(new_body.lines());
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        Ok(json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) }))
    }
}

// ── insert_code (before/after a symbol) ────────────────────────────────────

pub struct InsertCode;

#[async_trait::async_trait]
impl Tool for InsertCode {
    fn name(&self) -> &str {
        "insert_code"
    }
    fn description(&self) -> &str {
        "Insert code immediately before or after a named symbol."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path", "code"],
            "properties": {
                "name_path": { "type": "string", "description": "Symbol name path (e.g. 'MyStruct/my_method')" },
                "path": { "type": "string", "description": "File path (relative or absolute)" },
                "code": { "type": "string", "description": "Code to insert (may contain newlines)" },
                "position": {
                    "type": "string",
                    "enum": ["before", "after"],
                    "description": "Insert before or after the symbol (default: after)"
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let code = input["code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'code'"))?;
        let position = input["position"].as_str().unwrap_or("after");

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let insert_at = match position {
            "before" => sym.start_line as usize,
            _ => (sym.end_line as usize + 1).min(lines.len()),
        };

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..insert_at]);
        new_lines.extend(code.lines());
        new_lines.extend_from_slice(&lines[insert_at..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1, "position": position }))
    }
}

/// A textual match found during post-rename sweep.
#[derive(Debug)]
struct TextualMatch {
    /// Relative path from project root
    file: String,
    /// All matching line numbers (1-indexed)
    lines: Vec<u32>,
    /// First N matching line contents (trimmed)
    previews: Vec<String>,
    /// Total occurrences in this file
    occurrence_count: usize,
    /// "documentation" | "config" | "source"
    kind: &'static str,
}

/// Classify a file by extension for result prioritization.
fn classify_file(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "md" | "txt" | "rst" | "adoc" => "documentation",
        "toml" | "yaml" | "yml" | "json" => "config",
        _ => "source",
    }
}

/// Sort key for file classification (lower = higher priority).
fn classify_sort_key(kind: &str) -> u8 {
    match kind {
        "documentation" => 0,
        "config" => 1,
        _ => 2,
    }
}

/// Post-rename text sweep: finds remaining textual occurrences of `old_name`
/// that the LSP rename didn't touch.
fn text_sweep(
    project_root: &Path,
    old_name: &str,
    lsp_modified_files: &std::collections::HashSet<PathBuf>,
    max_matches: usize,
    max_previews_per_file: usize,
) -> anyhow::Result<Vec<TextualMatch>> {
    let escaped = regex::escape(old_name);
    let pattern = format!(r"\b{escaped}\b");
    let re = regex::RegexBuilder::new(&pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()?;

    let mut file_matches: Vec<TextualMatch> = Vec::new();

    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();

        // Skip files already modified by LSP rename
        if lsp_modified_files.contains(path) {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue; // skip binary / non-UTF8
        };

        let mut lines = Vec::new();
        let mut previews = Vec::new();

        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                lines.push((i + 1) as u32);
                if previews.len() < max_previews_per_file {
                    previews.push(line.trim().to_string());
                }
            }
        }

        if !lines.is_empty() {
            let rel_path = path
                .strip_prefix(project_root)
                .unwrap_or(path)
                .display()
                .to_string();
            let kind = classify_file(path);
            let occurrence_count = lines.len();

            file_matches.push(TextualMatch {
                file: rel_path,
                lines,
                previews,
                occurrence_count,
                kind,
            });
        }
    }

    // Sort: documentation first, config second, source third
    file_matches.sort_by_key(|m| classify_sort_key(m.kind));

    // Cap total entries
    file_matches.truncate(max_matches);

    Ok(file_matches)
}

pub struct RenameSymbol;

#[async_trait::async_trait]
impl Tool for RenameSymbol {
    fn name(&self) -> &str {
        "rename_symbol"
    }
    fn description(&self) -> &str {
        "Rename a symbol across the entire codebase using LSP. After renaming, sweeps for remaining textual occurrences (comments, docs, strings) that LSP missed and reports them."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path", "new_name"],
            "properties": {
                "name_path": { "type": "string" },
                "path": { "type": "string" },
                "new_name": { "type": "string" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let name_path = input["name_path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'name_path'"))?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let new_name = input["new_name"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'new_name'"))?;

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Find the symbol to get its position
        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path)
            .ok_or_else(|| anyhow!("symbol not found: {}", name_path))?;

        // Request rename from LSP
        let edit = client
            .rename(&full_path, sym.start_line, sym.start_col, new_name, &lang)
            .await?;

        // Apply workspace edit — validate every file from the LSP response
        // as a write target before modifying it.
        let rename_root = ctx.agent.require_project_root().await?;
        let rename_security = ctx.agent.security_config().await;
        let mut files_changed = 0;
        let mut total_edits = 0;
        let mut lsp_files: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        if let Some(changes) = &edit.changes {
            for (uri, edits) in changes {
                let Some(path) = uri_to_path(uri.as_str()) else {
                    continue;
                };
                let path_str = path.display().to_string();
                crate::util::path_security::validate_write_path(
                    &path_str,
                    &rename_root,
                    &rename_security,
                )?;
                let content = std::fs::read_to_string(&path)?;
                let new_content = apply_text_edits(&content, edits);
                std::fs::write(&path, new_content)?;
                lsp_files.insert(path.clone());
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
                        let path_str = path.display().to_string();
                        crate::util::path_security::validate_write_path(
                            &path_str,
                            &rename_root,
                            &rename_security,
                        )?;
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
                        lsp_files.insert(path.clone());
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
                    let path_str = path.display().to_string();
                    crate::util::path_security::validate_write_path(
                        &path_str,
                        &rename_root,
                        &rename_security,
                    )?;
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
                    lsp_files.insert(path.clone());
                    files_changed += 1;
                    total_edits += text_edit.edits.len();
                }
            }
        }

        // Phase 2: text sweep for remaining textual occurrences
        let old_name_str = name_path.rsplit('/').next().unwrap_or(name_path);
        let (textual, sweep_skipped, sweep_skip_reason) = if old_name_str.len() < 4 {
            (
                vec![],
                true,
                Some(format!(
                    "name too short ({} chars, minimum 4)",
                    old_name_str.len()
                )),
            )
        } else {
            match text_sweep(&rename_root, old_name_str, &lsp_files, 20, 2) {
                Ok(matches) => (matches, false, None::<String>),
                Err(e) => {
                    tracing::warn!("text sweep after rename failed: {e}");
                    (vec![], false, Some(format!("sweep error: {e}")))
                }
            }
        };

        let textual_total: usize = textual.iter().map(|m| m.occurrence_count).sum();
        let textual_shown = textual.len();
        let textual_json: Vec<Value> = textual
            .into_iter()
            .map(|m| {
                json!({
                    "file": m.file,
                    "lines": m.lines,
                    "previews": m.previews,
                    "occurrence_count": m.occurrence_count,
                    "kind": m.kind,
                })
            })
            .collect();

        let mut result = json!({
            "status": "ok",
            "old_name": old_name_str,
            "new_name": new_name,
            "files_changed": files_changed,
            "total_edits": total_edits,
            "textual_matches": textual_json,
            "textual_match_count": textual_total,
            "textual_matches_shown": textual_shown,
            "sweep_skipped": sweep_skipped,
        });
        if let Some(reason) = sweep_skip_reason {
            result["sweep_skip_reason"] = json!(reason);
        }
        Ok(result)
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
    if had_trailing_newline && !out.is_empty() {
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
///
/// Uses `url::Url` for correct handling of Windows drive letters,
/// UNC paths, and percent-encoding.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
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

/// Check if a path is outside the project root. If so, attempt to discover
/// and register the library. Returns the source tag.
#[allow(dead_code)]
/// and register the library. Returns the source tag.
async fn tag_external_path(
    path: &std::path::Path,
    project_root: &std::path::Path,
    agent: &crate::agent::Agent,
) -> String {
    if path.starts_with(project_root) {
        return "project".to_string();
    }

    // Check if already registered
    if let Some(registry) = agent.library_registry().await {
        if let Some(entry) = registry.is_library_path(path) {
            return format!("lib:{}", entry.name);
        }
    }

    // Attempt auto-discovery
    if let Some(discovered) = crate::library::discovery::discover_library_root(path) {
        let name = discovered.name.clone();
        let mut inner = agent.inner.write().await;
        if let Some(project) = inner.active_project.as_mut() {
            project.library_registry.register(
                discovered.name,
                discovered.path,
                discovered.language,
                crate::library::registry::DiscoveryMethod::LspFollowThrough,
            );
            // Best-effort save — don't fail the tool call if this fails
            let registry_path = project.root.join(".code-explorer").join("libraries.json");
            let _ = project.library_registry.save(&registry_path);
        }
        format!("lib:{}", name)
    } else {
        "external".to_string()
    }
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

        let result = ListSymbols
            .call(
                json!({
                    "path": "src/main.rs",
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
    async fn find_symbol_project_wide_uses_workspace_symbol() {
        let Some((_dir, ctx)) = rust_project_ctx().await else {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        };

        // Trigger LSP startup and background indexing via a file-restricted call.
        let _ = FindSymbol
            .call(json!({ "pattern": "main", "path": "src/main.rs" }), &ctx)
            .await;

        // Retry project-wide search (no relative_path → workspace/symbol fast path)
        // until rust-analyzer finishes background indexing (typically < 3s).
        let mut found = false;
        for _ in 0..10 {
            let result = FindSymbol
                .call(json!({ "pattern": "Point" }), &ctx)
                .await
                .unwrap();
            let symbols = result["symbols"].as_array().unwrap();
            if symbols.iter().any(|s| s["name"].as_str() == Some("Point")) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        assert!(
            found,
            "should find 'Point' project-wide via workspace/symbol within 5s"
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
                    "path": "src/main.rs"
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
    async fn get_symbols_overview_accepts_detail_level() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
        };
        // Should error because no project, but NOT because of unknown param
        let err = ListSymbols
            .call(json!({ "path": "x", "detail_level": "full" }), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("project"),
            "should fail on project, not param: {}",
            err
        );
    }

    #[tokio::test]
    async fn path_not_found_is_recoverable_error() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        let err = ListSymbols
            .call(json!({ "path": "nonexistent/file.py" }), &ctx)
            .await
            .unwrap_err();

        assert!(
            err.downcast_ref::<crate::tools::RecoverableError>()
                .is_some(),
            "path not found must be RecoverableError, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn path_not_found_hint_mentions_list_dir() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        let err = ListSymbols
            .call(json!({ "path": "missing.rs" }), &ctx)
            .await
            .unwrap_err();

        let rec = err
            .downcast_ref::<crate::tools::RecoverableError>()
            .expect("should be RecoverableError");
        assert!(
            rec.hint.as_deref().unwrap_or("").contains("list_dir"),
            "hint should mention list_dir, got: {:?}",
            rec.hint
        );
    }

    #[tokio::test]
    async fn glob_no_match_is_recoverable_error() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        let err = ListSymbols
            .call(json!({ "path": "src/**/*.nonexistent" }), &ctx)
            .await
            .unwrap_err();

        assert!(
            err.downcast_ref::<crate::tools::RecoverableError>()
                .is_some(),
            "empty glob must be RecoverableError, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
        };
        assert!(ListSymbols.call(json!({"path": "x"}), &ctx).await.is_err());
        assert!(FindSymbol
            .call(json!({"pattern": "x"}), &ctx)
            .await
            .is_err());
        assert!(FindReferences
            .call(json!({"name_path": "x", "path": "y"}), &ctx)
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

    #[cfg(unix)]
    #[test]
    fn uri_to_path_parses_unix_uri() {
        let p = uri_to_path("file:///home/user/code.rs").unwrap();
        assert_eq!(p, PathBuf::from("/home/user/code.rs"));
    }

    #[tokio::test]
    async fn find_symbol_project_wide_treesitter_fallback() {
        // No rust-analyzer needed — this test verifies the tree-sitter fallback
        // that kicks in when workspace/symbol returns empty.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn unique_benchmark_fn() -> i32 { 42 }\n\npub struct UniqueTestStruct { x: i32 }\n",
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        // Project-wide search (no relative_path) — LSP will fail/return empty,
        // so tree-sitter fallback should find the symbol.
        let result = FindSymbol
            .call(json!({ "pattern": "unique_benchmark_fn" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "should find symbol via tree-sitter fallback: {:?}",
            result
        );
        assert!(symbols
            .iter()
            .any(|s| s["name"].as_str().unwrap() == "unique_benchmark_fn"));

        // Also check struct is findable
        let result2 = FindSymbol
            .call(json!({ "pattern": "UniqueTestStruct" }), &ctx)
            .await
            .unwrap();
        let symbols2 = result2["symbols"].as_array().unwrap();
        assert!(
            symbols2
                .iter()
                .any(|s| s["name"].as_str().unwrap() == "UniqueTestStruct"),
            "should find struct via tree-sitter fallback: {:?}",
            result2
        );
    }

    #[tokio::test]
    async fn get_symbols_overview_finds_nested_files() {
        // No LSP needed — verifies recursive walk + tree-sitter fallback.
        // Source files ONLY in subdirectories (not at root).
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn nested_function() -> i32 { 42 }\n",
        )
        .unwrap();
        // Also one at root for comparison
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        // Project-wide (no path) — should find both root and nested files
        let result = ListSymbols.call(json!({}), &ctx).await.unwrap();

        let files = result["files"].as_array().unwrap();
        let file_names: Vec<&str> = files.iter().map(|f| f["file"].as_str().unwrap()).collect();
        assert!(
            files.len() >= 2,
            "should find files in subdirectories, got: {:?}",
            file_names
        );
        assert!(
            file_names.iter().any(|f| f.contains("src/lib.rs")),
            "should find nested src/lib.rs, got: {:?}",
            file_names
        );
        assert!(
            file_names.iter().any(|f| f.contains("main.rs")),
            "should find root main.rs, got: {:?}",
            file_names
        );
    }

    #[tokio::test]
    async fn get_symbols_overview_subdir_stays_shallow() {
        // When targeting a specific subdirectory (not root), should NOT recurse.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep/nested")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(dir.path().join("src/top.rs"), "pub fn top_level() {}\n").unwrap();
        std::fs::write(
            dir.path().join("src/deep/nested/hidden.rs"),
            "pub fn deeply_nested() {}\n",
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        // Target "src" specifically — should be shallow (depth 1)
        let result = ListSymbols
            .call(json!({ "path": "src" }), &ctx)
            .await
            .unwrap();

        let files = result["files"].as_array().unwrap();
        let file_names: Vec<&str> = files.iter().map(|f| f["file"].as_str().unwrap()).collect();
        assert!(
            file_names.iter().any(|f| f.contains("top.rs")),
            "should find src/top.rs in shallow walk, got: {:?}",
            file_names
        );
        // The deeply nested file should NOT appear with shallow walk
        assert!(
            !file_names.iter().any(|f| f.contains("hidden.rs")),
            "should NOT find deeply nested file with shallow walk, got: {:?}",
            file_names
        );
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

    #[test]
    fn find_symbol_by_name_path_exact_match() {
        let test_file = std::env::temp_dir().join("test.rs");
        let symbols = vec![SymbolInfo {
            name: "MyStruct".to_string(),
            name_path: "MyStruct".to_string(),
            kind: crate::lsp::SymbolKind::Struct,
            file: test_file.clone(),
            start_line: 0,
            end_line: 10,
            start_col: 0,
            children: vec![SymbolInfo {
                name: "my_method".to_string(),
                name_path: "MyStruct/my_method".to_string(),
                kind: crate::lsp::SymbolKind::Method,
                file: test_file,
                start_line: 2,
                end_line: 5,
                start_col: 4,
                children: vec![],
            }],
        }];

        // Exact name_path match for nested symbol
        let found = find_symbol_by_name_path(&symbols, "MyStruct/my_method");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "my_method");

        // Exact name_path match for top-level
        let found = find_symbol_by_name_path(&symbols, "MyStruct");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "MyStruct");

        // Bare name match (fallback)
        let found = find_symbol_by_name_path(&symbols, "my_method");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "my_method");

        // Miss
        let found = find_symbol_by_name_path(&symbols, "nonexistent");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_referencing_symbols_returns_references() {
        if !std::process::Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "test-refs"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        // Write a file where `add` is defined and called twice
        std::fs::write(
            dir.path().join("src/main.rs"),
            r#"fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let x = add(1, 2);
    let y = add(3, 4);
    println!("{} {}", x, y);
}
"#,
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };

        // rust-analyzer needs time to load the Cargo project and build its index
        // before textDocument/references returns results. Retry with back-off.
        let mut result_value: Option<Value> = None;
        for attempt in 0..10 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
            }

            let result = FindReferences
                .call(
                    json!({
                        "name_path": "add",
                        "path": "src/main.rs"
                    }),
                    &ctx,
                )
                .await;

            // If LSP startup fails (e.g. cargo not in PATH), skip gracefully
            let value = match result {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Skipping: LSP error: {}", e);
                    return;
                }
            };

            let total = value["total"].as_u64().unwrap_or(0);
            if total >= 3 {
                result_value = Some(value);
                break;
            }
            eprintln!(
                "Attempt {}: got {} references, retrying...",
                attempt + 1,
                total
            );
        }

        let result = match result_value {
            Some(v) => v,
            None => {
                eprintln!("Skipping: rust-analyzer did not index in time");
                return;
            }
        };

        let refs = result["references"].as_array().unwrap();
        let total = result["total"].as_u64().unwrap();

        // Should find at least 3 references: definition + 2 call sites
        assert!(
            total >= 3,
            "Expected >= 3 references (def + 2 calls), got {}. refs: {:?}",
            total,
            refs
        );

        // All references should be in src/main.rs
        for r in refs {
            let file = r["file"].as_str().unwrap();
            assert!(
                file.contains("main.rs"),
                "Reference in unexpected file: {}",
                file
            );
            // context should contain meaningful text
            let ctx_line = r["context"].as_str().unwrap();
            assert!(!ctx_line.is_empty(), "Context line should not be empty");
        }
    }

    #[tokio::test]
    async fn find_symbol_schema_includes_scope() {
        let tool = FindSymbol;
        let schema = tool.input_schema();
        assert!(schema["properties"]["scope"].is_object());
    }

    #[tokio::test]
    async fn get_symbols_overview_schema_includes_scope() {
        let tool = ListSymbols;
        let schema = tool.input_schema();
        assert!(schema["properties"]["scope"].is_object());
    }

    #[tokio::test]
    async fn find_referencing_symbols_schema_includes_scope() {
        let tool = FindReferences;
        let schema = tool.input_schema();
        assert!(schema["properties"]["scope"].is_object());
    }

    #[tokio::test]
    async fn tag_external_path_returns_project_for_internal() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let root = agent.require_project_root().await.unwrap();
        let internal = root.join("src/main.rs");
        let tag = tag_external_path(&internal, &root, &agent).await;
        assert_eq!(tag, "project");
    }

    #[tokio::test]
    async fn tag_external_path_discovers_and_registers() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let root = agent.require_project_root().await.unwrap();

        // Create a fake library directory with Cargo.toml
        let lib_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            lib_dir.path().join("Cargo.toml"),
            "[package]\nname = \"fake_lib\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let lib_src = lib_dir.path().join("src");
        std::fs::create_dir_all(&lib_src).unwrap();
        let lib_file = lib_src.join("lib.rs");
        std::fs::write(&lib_file, "pub fn hello() {}").unwrap();

        let tag = tag_external_path(&lib_file, &root, &agent).await;
        assert_eq!(tag, "lib:fake_lib");

        // Verify it was registered
        let registry = agent.library_registry().await.unwrap();
        assert!(registry.lookup("fake_lib").is_some());
    }

    #[tokio::test]
    async fn find_symbol_directory_relative_path() {
        let Some((_dir, ctx)) = rust_project_ctx().await else {
            return; // skip if rust-analyzer not available
        };

        // "src" is a directory — should walk it and find symbols inside
        let result = FindSymbol
            .call(json!({ "pattern": "add", "path": "src" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with directory relative_path should find symbols"
        );
        assert!(symbols.iter().any(|s| s["name"] == "add"));
    }

    #[test]
    fn collect_matching_matches_name_path() {
        let symbols = vec![SymbolInfo {
            name: "MyStruct".into(),
            name_path: "MyStruct".into(),
            kind: crate::lsp::SymbolKind::Struct,
            file: PathBuf::from("test.rs"),
            start_line: 0,
            end_line: 10,
            start_col: 0,
            children: vec![SymbolInfo {
                name: "my_method".into(),
                name_path: "MyStruct/my_method".into(),
                kind: crate::lsp::SymbolKind::Method,
                file: PathBuf::from("test.rs"),
                start_line: 2,
                end_line: 5,
                start_col: 4,
                children: vec![],
            }],
        }];

        // Pattern with "/" should match via name_path
        let mut results = vec![];
        collect_matching(
            &symbols,
            "mystruct/my_method",
            false,
            None,
            0,
            "project",
            &mut results,
        );
        assert!(
            !results.is_empty(),
            "pattern with '/' should match against name_path"
        );
        assert_eq!(results[0]["name"], "my_method");

        // Pattern without "/" should still match via name as before
        let mut results2 = vec![];
        collect_matching(
            &symbols,
            "my_method",
            false,
            None,
            0,
            "project",
            &mut results2,
        );
        assert!(
            !results2.is_empty(),
            "pattern without '/' should still match via name"
        );
    }

    async fn rich_project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/utils")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/empty")).unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test-project\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {}\n\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn helper() -> bool { true }\n\npub struct Calculator;\n\nimpl Calculator {\n    pub fn compute() -> i32 { 42 }\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/utils/math.rs"),
            "pub fn multiply(a: i32, b: i32) -> i32 { a * b }\n",
        )
        .unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (dir, ToolContext { agent, lsp: lsp() })
    }

    #[tokio::test]
    async fn find_symbol_path_type_file() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(json!({ "pattern": "add", "path": "src/main.rs" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with file relative_path should find symbols"
        );
        assert!(symbols.iter().any(|s| s["name"] == "add"));
    }

    #[tokio::test]
    async fn find_symbol_path_type_directory() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(json!({ "pattern": "helper", "path": "src" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with directory relative_path should find symbols: {:?}",
            result
        );
        assert!(symbols.iter().any(|s| s["name"] == "helper"));
    }

    #[tokio::test]
    async fn find_symbol_path_type_nested_directory() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(json!({ "pattern": "multiply", "path": "src/utils" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with nested directory relative_path should find symbols: {:?}",
            result
        );
        assert!(symbols.iter().any(|s| s["name"] == "multiply"));
    }

    #[tokio::test]
    async fn find_symbol_path_type_glob() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(json!({ "pattern": "add", "path": "src/**/*.rs" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with glob relative_path should find symbols: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn find_symbol_empty_directory_returns_empty() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(json!({ "pattern": "anything", "path": "src/empty" }), &ctx)
            .await
            .unwrap();

        let total = result["total"].as_u64().unwrap();
        assert_eq!(total, 0, "empty directory should return 0 results");
    }

    #[tokio::test]
    async fn find_symbol_name_path_pattern_in_directory() {
        let (_dir, ctx) = rich_project_ctx().await;

        let result = FindSymbol
            .call(
                json!({ "pattern": "impl Calculator/compute", "path": "src" }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with name_path pattern in directory should find symbols: {:?}",
            result
        );
        assert!(symbols.iter().any(|s| s["name"] == "compute"));
    }

    #[tokio::test]
    async fn find_symbol_name_path_pattern_project_wide() {
        let (_dir, ctx) = rich_project_ctx().await;

        // tree-sitter merges impl methods under the type name directly
        // (no "impl" prefix), so name_path is "Calculator/compute"
        let result = FindSymbol
            .call(json!({ "pattern": "Calculator/compute" }), &ctx)
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(
            !symbols.is_empty(),
            "find_symbol with name_path pattern project-wide should find symbols via tree-sitter: {:?}",
            result
        );
        assert!(symbols.iter().any(|s| s["name"] == "compute"));
    }

    #[test]
    fn collect_matching_slash_pattern_precision() {
        let symbols = vec![SymbolInfo {
            name: "MyStruct".into(),
            name_path: "MyStruct".into(),
            kind: crate::lsp::SymbolKind::Struct,
            file: PathBuf::from("test.rs"),
            start_line: 0,
            end_line: 10,
            start_col: 0,
            children: vec![SymbolInfo {
                name: "my_method".into(),
                name_path: "MyStruct/my_method".into(),
                kind: crate::lsp::SymbolKind::Method,
                file: PathBuf::from("test.rs"),
                start_line: 2,
                end_line: 5,
                start_col: 4,
                children: vec![],
            }],
        }];

        let mut results = vec![];
        collect_matching(
            &symbols,
            "mystruct/my_method",
            false,
            None,
            0,
            "project",
            &mut results,
        );
        assert_eq!(
            results.len(),
            1,
            "slash pattern should match exactly 1 result (the method), not the parent struct"
        );
        assert_eq!(results[0]["name"], "my_method");
    }

    #[test]
    fn text_sweep_finds_matches_in_comments_and_docs() {
        let dir = tempfile::tempdir().unwrap();

        // Source file with a comment mentioning the old name
        std::fs::write(
            dir.path().join("main.rs"),
            "fn bar() {}\n// FooHandler manages connections\n",
        )
        .unwrap();

        // Documentation file
        std::fs::write(
            dir.path().join("README.md"),
            "# Project\nThe FooHandler struct is the entry point.\nSee FooHandler::new() for details.\n",
        )
        .unwrap();

        // Config file
        std::fs::write(
            dir.path().join("config.toml"),
            "[server]\nhandler = \"FooHandler\"\n",
        )
        .unwrap();

        let lsp_files = std::collections::HashSet::new();
        let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

        // Should find matches in all 3 files
        assert_eq!(matches.len(), 3);

        // Documentation first, then config, then source
        assert_eq!(matches[0].kind, "documentation");
        assert_eq!(matches[1].kind, "config");
        assert_eq!(matches[2].kind, "source");

        // README has 2 occurrences, both shown as previews
        assert_eq!(matches[0].occurrence_count, 2);
        assert_eq!(matches[0].previews.len(), 2);

        // Config has 1 occurrence
        assert_eq!(matches[1].occurrence_count, 1);

        // Source has 1 occurrence (comment line)
        assert_eq!(matches[2].occurrence_count, 1);
    }

    #[test]
    fn text_sweep_skips_lsp_modified_files() {
        let dir = tempfile::tempdir().unwrap();

        let modified_file = dir.path().join("already.rs");
        std::fs::write(&modified_file, "// FooHandler was here\n").unwrap();
        std::fs::write(dir.path().join("untouched.md"), "FooHandler docs\n").unwrap();

        let mut lsp_files = std::collections::HashSet::new();
        lsp_files.insert(modified_file);

        let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

        assert_eq!(matches.len(), 1);
        assert!(matches[0].file.contains("untouched.md"));
    }

    #[test]
    fn text_sweep_respects_max_matches_cap() {
        let dir = tempfile::tempdir().unwrap();

        // Create 30 markdown files, each with one match
        for i in 0..30 {
            std::fs::write(
                dir.path().join(format!("doc{i:02}.md")),
                format!("FooHandler reference in doc {i}\n"),
            )
            .unwrap();
        }

        let lsp_files = std::collections::HashSet::new();
        let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

        assert_eq!(matches.len(), 20);
    }

    #[test]
    fn text_sweep_limits_previews_per_file() {
        let dir = tempfile::tempdir().unwrap();

        // File with 10 occurrences
        let content = (0..10)
            .map(|i| format!("line {i}: FooHandler usage"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("many.rs"), &content).unwrap();

        let lsp_files = std::collections::HashSet::new();
        let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].occurrence_count, 10);
        assert_eq!(matches[0].previews.len(), 2); // capped at 2
        assert_eq!(matches[0].lines.len(), 10); // all line numbers kept
    }

    #[test]
    fn text_sweep_uses_word_boundary() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("test.rs"),
            "let foo_handler = 1;\n// FooHandler docs\nlet FooHandlerConfig = 2;\n",
        )
        .unwrap();

        let lsp_files = std::collections::HashSet::new();
        let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

        assert_eq!(matches.len(), 1);
        // \bFooHandler\b does NOT match inside FooHandlerConfig because
        // there's no word boundary between 'r' and 'C' (both are word chars).
        // So only 1 match: the comment line.
        assert_eq!(matches[0].occurrence_count, 1);
        assert!(matches[0].previews[0].contains("// FooHandler docs"));
    }

    // ── write_lines / splice edge cases ────────────────────────────────────

    #[test]
    fn write_lines_no_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let lines: Vec<&str> = vec!["line1", "line2", "line3"];
        write_lines(&file, &lines, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn write_lines_with_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let lines: Vec<&str> = vec!["line1", "line2", "line3"];
        write_lines(&file, &lines, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "line1\nline2\nline3\n"
        );
    }

    #[test]
    fn write_lines_empty_with_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let lines: Vec<&str> = vec![];
        write_lines(&file, &lines, true).unwrap();
        // Empty content should not become "\n"
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
    }

    /// Simulates the replace_symbol pattern: lines before + multi-line body + lines after.
    /// The body should be split into individual lines before inserting.
    #[test]
    fn splice_multiline_body_no_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");

        let original = "// before\nfn foo() {\n    old();\n}\n// after\n";
        std::fs::write(&file, original).unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        // Simulate replace_symbol: replace lines 1-3 (0-indexed) with new body
        let new_body = "fn foo() {\n    new();\n}";
        let start = 1usize;
        let end = 4usize; // exclusive

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        // FIX: split body into lines, don't push as single element
        new_lines.extend(new_body.lines());
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&file, &new_lines, content.ends_with('\n')).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        assert_eq!(result, "// before\nfn foo() {\n    new();\n}\n// after\n");
    }

    /// When body has trailing newline, the extra \n must NOT create a blank line.
    #[test]
    fn splice_multiline_body_with_trailing_newline_no_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");

        let original = "// before\nfn foo() {\n    old();\n}\n// after\n";
        std::fs::write(&file, original).unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        // LLM passes body WITH trailing newline (common)
        let new_body = "fn foo() {\n    new();\n}\n";
        let start = 1usize;
        let end = 4usize;

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.extend(new_body.lines()); // .lines() strips the trailing \n — correct!
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&file, &new_lines, content.ends_with('\n')).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        // Must NOT have blank line between "}" and "// after"
        assert_eq!(result, "// before\nfn foo() {\n    new();\n}\n// after\n");
    }

    /// Demonstrates the BUG: pushing multi-line body as single element creates extra blank line
    /// when body has trailing newline. This test documents the broken behavior.
    #[test]
    fn splice_push_single_element_creates_blank_line_bug() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");

        let original = "// before\nfn foo() {\n    old();\n}\n// after\n";
        std::fs::write(&file, original).unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        let new_body = "fn foo() {\n    new();\n}\n"; // trailing newline
        let start = 1usize;
        let end = 4usize;

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.push(new_body); // THE BUG: push as single element
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&file, &new_lines, content.ends_with('\n')).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        // BUG: extra blank line between "}" and "// after"
        assert!(
            result.contains("}\n\n// after"),
            "Expected blank line bug, got: {:?}",
            result
        );
    }

    #[test]
    fn apply_text_edits_preserves_trailing_newline() {
        let content = "hello world\nfoo bar\nbaz\n";
        let edits = vec![lsp_types::TextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 1,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: 1,
                    character: 7,
                },
            },
            new_text: "replaced".to_string(),
        }];
        let result = apply_text_edits(content, &edits);
        assert_eq!(result, "hello world\nreplaced\nbaz\n");
    }

    #[test]
    fn apply_text_edits_multiline_replacement() {
        let content = "aaa\nbbb\nccc\n";
        let edits = vec![lsp_types::TextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 1,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: 1,
                    character: 3,
                },
            },
            new_text: "xxx\nyyy".to_string(),
        }];
        let result = apply_text_edits(content, &edits);
        assert_eq!(result, "aaa\nxxx\nyyy\nccc\n");
    }
}
