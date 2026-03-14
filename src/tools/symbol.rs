//! Symbol-level tools backed by the LSP client.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::tools::RecoverableError;

use super::format::{format_line_range, format_overflow};
use super::output::{OutputGuard, OutputMode, OverflowInfo};
use super::{parse_bool_param, Tool, ToolContext};
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

/// Resolve which library directories to search for a given scope.
/// Returns `(library_name, absolute_root_path)` pairs.
async fn resolve_library_roots(
    scope: &crate::library::scope::Scope,
    agent: &crate::agent::Agent,
) -> Vec<(String, PathBuf)> {
    let registry = match agent.library_registry().await {
        Some(r) => r,
        None => return vec![],
    };
    registry
        .all()
        .iter()
        .filter(|entry| scope.includes_library(&entry.name))
        .map(|entry| (entry.name.clone(), entry.path.clone()))
        .collect()
}

/// Format a file path relative to a library root for display.
/// Returns `lib:<name>/<relative_path>` or the absolute path as fallback.
fn format_library_path(lib_name: &str, lib_root: &Path, file_path: &Path) -> String {
    file_path
        .strip_prefix(lib_root)
        .map(|rel| format!("lib:{}/{}", lib_name, rel.display()))
        .unwrap_or_else(|_| file_path.display().to_string())
}

/// Classify a reference path as project, library, or external.
/// Returns (classification_tag, display_path).
fn classify_reference_path(
    path: &Path,
    project_root: &Path,
    library_roots: &[(String, PathBuf)],
) -> (String, String) {
    if path.starts_with(project_root) {
        let rel = path.strip_prefix(project_root).unwrap_or(path);
        ("project".to_string(), rel.display().to_string())
    } else if let Some((name, lib_root)) = library_roots.iter().find(|(_, r)| path.starts_with(r)) {
        (
            "lib:".to_string() + name,
            format_library_path(name, lib_root, path),
        )
    } else {
        ("external".to_string(), path.display().to_string())
    }
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
        .map_err(|e| {
            RecoverableError::with_hint(
                format!("invalid glob pattern '{}': {}", path_or_glob, e),
                "Check glob syntax: use * for any segment, ** for recursive, ? for single char.",
            )
        })?;
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
        None if required => Err(RecoverableError::with_hint(
            "missing 'path' parameter",
            "Add the required 'path' parameter to the tool call.",
        )
        .into()),
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
) -> anyhow::Result<(std::sync::Arc<dyn crate::lsp::LspClientOps>, String)> {
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
/// Returns true if the symbol's kind matches the given filter string.
/// Unknown filter values return true (no filtering).
fn matches_kind_filter(kind: &crate::lsp::SymbolKind, filter: &str) -> bool {
    use crate::lsp::SymbolKind as K;
    match filter {
        "function" => matches!(kind, K::Function | K::Method | K::Constructor),
        "class" => matches!(kind, K::Class),
        "struct" => matches!(kind, K::Struct),
        "interface" => matches!(kind, K::Interface),
        "type" => matches!(kind, K::TypeParameter),
        "enum" => matches!(kind, K::Enum | K::EnumMember),
        "module" => matches!(kind, K::Module | K::Namespace | K::Package),
        "constant" => matches!(kind, K::Constant),
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_matching(
    symbols: &[SymbolInfo],
    name_ok: &dyn Fn(&SymbolInfo) -> bool,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    show_file: bool,
    out: &mut Vec<Value>,
    kind_filter: Option<&str>,
) {
    for sym in symbols {
        let kind_ok = kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
        if name_ok(sym) && kind_ok {
            out.push(symbol_to_json(
                sym,
                include_body,
                source_code,
                depth,
                show_file,
            ));
        }
        // Always recurse so nested matches inside filtered-out parents are still found.
        collect_matching(
            &sym.children,
            name_ok,
            include_body,
            source_code,
            depth,
            show_file,
            out,
            kind_filter,
        );
    }
}

fn symbol_to_json(
    sym: &SymbolInfo,
    include_body: bool,
    source_code: Option<&str>,
    depth: usize,
    show_file: bool,
) -> Value {
    // Build fields in a reader-friendly order:
    //   identity  → name, name_path, kind
    //   location  → file (optional)
    //   detail    → signature, body (optional)
    //   structure → children (optional)
    //   metadata  → start_line, end_line (last — positional info, not primary identity)
    let mut map = serde_json::Map::new();

    map.insert("name".into(), json!(sym.name));
    map.insert("name_path".into(), json!(sym.name_path));
    map.insert("kind".into(), json!(format!("{:?}", sym.kind)));

    if show_file {
        map.insert("file".into(), json!(sym.file.display().to_string()));
    }

    if let Some(sig) = &sym.detail {
        map.insert("signature".into(), json!(sig));
    }

    if include_body {
        if let Some(src) = source_code {
            let lines: Vec<&str> = src.lines().collect();
            // Use the full range (including attributes and doc comments) so
            // the body matches what replace_symbol would replace.
            let body_start = editing_start_line(sym, &lines);
            let end = (sym.end_line as usize + 1).min(lines.len());
            if body_start < lines.len() {
                map.insert("body".into(), json!(lines[body_start..end].join("\n")));
                // 1-indexed line where body begins — may differ from start_line
                // when attributes or doc comments precede the declaration.
                map.insert("body_start_line".into(), json!(body_start + 1));
            }
        }
    }

    if depth > 0 && !sym.children.is_empty() {
        map.insert(
            "children".into(),
            json!(sym
                .children
                .iter()
                .map(|c| symbol_to_json(c, include_body, source_code, depth - 1, show_file))
                .collect::<Vec<_>>()),
        );
    }

    // Line numbers last — positional metadata, not primary identity.
    map.insert("start_line".into(), json!(sym.start_line + 1));
    map.insert("end_line".into(), json!(sym.end_line + 1));

    Value::Object(map)
}

/// When the LSP `workspace/symbol` response returns a degenerate range
/// (`start_line == end_line`, i.e. only the name position), look up the
/// true declaration end from tree-sitter and return an updated `SymbolInfo`.
/// If `start_line != end_line` the symbol is returned unchanged.
/// Detect degenerate LSP ranges where start_line == end_line but tree-sitter
/// shows the symbol spans multiple lines. Returns RecoverableError instead of
/// silently fixing — consistent with "trust LSP, validate, fail loudly".
fn validate_symbol_range(sym: &SymbolInfo) -> anyhow::Result<()> {
    let Ok(ast_syms) = crate::ast::extract_symbols(&sym.file) else {
        return Ok(());
    };
    if let Some(ast_end) = find_ast_end_line_in(&ast_syms, &sym.name, sym.start_line) {
        if ast_end > sym.end_line {
            anyhow::bail!(RecoverableError::with_hint(
                format!(
                    "LSP returned suspicious range for '{}' (lines {}-{}, but AST shows it spans to line {})",
                    sym.name,
                    sym.start_line + 1,
                    sym.end_line + 1,
                    ast_end + 1,
                ),
                "The LSP server may have returned a selection range instead of the full symbol range. \
                 Try edit_file for this symbol, or check list_symbols to verify the range.",
            ));
        }
    }
    Ok(())
}

/// Recursively search `symbols` for a symbol with the given name whose
/// `start_line` is within 1 of `lsp_start`. Returns its `end_line`.
fn find_ast_end_line_in(symbols: &[SymbolInfo], name: &str, lsp_start: u32) -> Option<u32> {
    for sym in symbols {
        if sym.name == name && sym.start_line.abs_diff(lsp_start) <= 1 {
            return Some(sym.end_line);
        }
        if let Some(end) = find_ast_end_line_in(&sym.children, name, lsp_start) {
            return Some(end);
        }
    }
    None
}

/// When `workspace/symbol` returns a degenerate range, attempt to resolve the
/// correct range by querying `textDocument/documentSymbol` for the symbol's file.
/// Returns the corrected SymbolInfo if found, None otherwise.
async fn resolve_range_via_document_symbols(
    sym: &SymbolInfo,
    ctx: &ToolContext,
) -> Option<SymbolInfo> {
    let lang = crate::ast::detect_language(&sym.file)?;
    let language_id = crate::lsp::servers::lsp_language_id(lang);
    let root = ctx.agent.require_project_root().await.ok()?;
    let client = ctx.lsp.get_or_start(lang, &root).await.ok()?;
    let doc_symbols = client.document_symbols(&sym.file, language_id).await.ok()?;
    find_matching_symbol(&doc_symbols, &sym.name, sym.start_line)
}

/// Recursively search a document symbol tree for a symbol matching `name`
/// within ±1 line of `lsp_start`. Returns a clone of the matching SymbolInfo.
fn find_matching_symbol(symbols: &[SymbolInfo], name: &str, lsp_start: u32) -> Option<SymbolInfo> {
    for sym in symbols {
        if sym.name == name && sym.start_line.abs_diff(lsp_start) <= 1 {
            return Some(sym.clone());
        }
        if let Some(found) = find_matching_symbol(&sym.children, name, lsp_start) {
            return Some(found);
        }
    }
    None
}

// ── get_symbols_overview ───────────────────────────────────────────────────

/// Directory/glob scans can produce huge output (each file has many symbols).
/// Cap exploring-mode file count lower than the global OutputGuard default (200).
const LIST_SYMBOLS_MAX_FILES: usize = 50;
/// Hard cap on top-level symbols (fallback when flat count is within budget).
const LIST_SYMBOLS_SINGLE_FILE_CAP: usize = 100;
/// Cap on *total* symbol entries including depth-1 children.
/// A single `impl` block with 10 methods counts as 11 flat entries, so the
/// flat budget prevents depth-1 output from ballooning even on rich files.
const LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP: usize = 150;

/// Count top-level symbols plus their direct children (depth-1 children).
fn flat_symbol_count(symbols: &[Value]) -> usize {
    symbols
        .iter()
        .map(|s| 1 + s["children"].as_array().map(|c| c.len()).unwrap_or(0))
        .sum()
}

pub struct ListSymbols;

#[async_trait::async_trait]
impl Tool for ListSymbols {
    fn name(&self) -> &str {
        "list_symbols"
    }
    fn description(&self) -> &str {
        "Return a tree of symbols (functions, classes, methods, etc.) in a file or directory. \
         Uses LSP for accurate results. Pass include_docs=true to also return docstrings \
         (replaces list_docs). Signatures are always included (replaces list_functions)."
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
                "scope": { "type": "string", "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>'", "default": "project" },
                "include_docs": {
                    "type": "boolean",
                    "default": false,
                    "description": "When true, include docstrings for each file alongside symbols (tree-sitter). Replaces list_docs."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let rel_path = get_path_param(&input, false)?.unwrap_or(".");
        let depth = input["depth"].as_u64().unwrap_or(1) as usize;
        let guard = OutputGuard::from_input(&input);
        let include_docs = parse_bool_param(&input["include_docs"]);
        let scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        // Helper: collect docstrings for a file path as a JSON array
        let collect_docstrings = |path: &std::path::Path| -> Vec<Value> {
            crate::ast::extract_docstrings(path)
                .unwrap_or_default()
                .iter()
                .map(|d| {
                    json!({
                        "symbol_name": d.symbol_name,
                        "content": d.content,
                        "start_line": d.start_line + 1,
                        "end_line": d.end_line + 1,
                    })
                })
                .collect()
        };

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
                                symbol_to_json(s, include_body, source.as_deref(), depth, false)
                            })
                            .collect();
                        let mut entry = json!({
                            "file": rel.display().to_string(),
                            "symbols": json_symbols,
                        });
                        if include_docs {
                            entry["docstrings"] = json!(collect_docstrings(file_path));
                        }
                        result.push(entry);
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
                .map(|s| symbol_to_json(s, include_body, source.as_deref(), depth, false))
                .collect();

            // Cap single-file results to prevent large files blowing the context window.
            // Primary check: flat count (top-level + depth-1 children combined).
            // A file with 50 impl blocks each containing 5 methods has 300 flat entries
            // even though it shows "50 symbols" — the flat cap catches that case.
            let total = json_symbols.len();
            let flat_total = flat_symbol_count(&json_symbols);
            let (json_symbols, overflow) = if flat_total > LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP {
                // Greedily include top-level symbols within the flat budget.
                let mut budget = LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP;
                let mut capped: Vec<Value> = Vec::new();
                for sym in json_symbols {
                    let cost = 1 + sym["children"].as_array().map(|c| c.len()).unwrap_or(0);
                    if cost <= budget {
                        budget -= cost;
                        capped.push(sym);
                    } else {
                        break;
                    }
                }
                let shown = capped.len();
                let hint = format!(
                    "File has {total} top-level symbols ({flat_total} total including children). \
                     Use depth=0 for a top-level-only overview, or \
                     find_symbol(name_path='...', include_body=true) for a specific symbol."
                );
                let ov = OverflowInfo {
                    shown,
                    total,
                    hint,
                    next_offset: None,
                    by_file: None,
                    by_file_overflow: 0,
                };
                (capped, Some(ov))
            } else {
                let mut file_guard = guard;
                file_guard.max_results = LIST_SYMBOLS_SINGLE_FILE_CAP;
                let hint = format!(
                    "File has {total} symbols. Use depth=0 for top-level overview, \
                     or find_symbol(name_path='ClassName/methodName', include_body=true) for a specific symbol."
                );
                file_guard.cap_items(json_symbols, &hint)
            };
            if let Some(ov) = overflow {
                let total = ov.total;
                let mut result =
                    json!({ "file": rel_path, "symbols": json_symbols, "total": total });
                result["overflow"] = OutputGuard::overflow_json(&ov);
                if include_docs {
                    result["docstrings"] = json!(collect_docstrings(&full_path));
                }
                return Ok(result);
            }
            let mut result = json!({ "file": rel_path, "symbols": json_symbols });
            if include_docs {
                result["docstrings"] = json!(collect_docstrings(&full_path));
            }
            Ok(result)
        } else if full_path.is_dir() {
            // Collect (display_path, abs_path) pairs from directory + libraries.
            // Project root → walk recursively so nested src/ files are found.
            // Subdirectory → shallow (depth 1) to avoid dumping entire subtrees.
            let root = ctx.agent.require_project_root().await?;
            let mut dir_files: Vec<(String, PathBuf)> = vec![];

            // Walk project directory when scope includes project
            if scope.includes_project() {
                let is_project_root = rel_path == "." || rel_path.is_empty();
                let walker = ignore::WalkBuilder::new(&full_path)
                    .max_depth(if is_project_root { None } else { Some(1) })
                    .hidden(true)
                    .git_ignore(true)
                    .build();
                for entry in walker.flatten() {
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    // Only include files with recognized source languages so that
                    // non-source files (configs, build outputs) don't consume the
                    // file cap before any real source files are seen.
                    if ast::detect_language(entry.path()).is_none() {
                        continue;
                    }
                    let abs = entry.path().to_path_buf();
                    let display = abs
                        .strip_prefix(&root)
                        .unwrap_or(&abs)
                        .display()
                        .to_string();
                    dir_files.push((display, abs));
                }
            }

            // Walk library directories when scope includes libraries
            let lib_roots = resolve_library_roots(&scope, &ctx.agent).await;
            for (lib_name, lib_root) in &lib_roots {
                // Library directories are external — don't apply the project's
                // .gitignore (e.g. .venv/ would hide pip-installed packages).
                // Still skip hidden dirs (.git/, __pycache__/, etc.).
                let walker = ignore::WalkBuilder::new(lib_root)
                    .max_depth(None)
                    .hidden(true)
                    .git_ignore(false)
                    .build();
                for entry in walker.flatten() {
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    if ast::detect_language(entry.path()).is_none() {
                        continue;
                    }
                    let abs = entry.path().to_path_buf();
                    let display = format_library_path(lib_name, lib_root, &abs);
                    dir_files.push((display, abs));
                }
            }

            let mut guard = guard;
            guard.max_files = guard.max_files.min(LIST_SYMBOLS_MAX_FILES);
            let (dir_files, file_overflow) =
                guard.cap_files(dir_files, "Narrow with a more specific glob or file path");
            let include_body = guard.should_include_body();

            // Aggregate symbols from capped file list
            let mut result = vec![];
            for (display_path, abs_path) in &dir_files {
                let Some(lang) = ast::detect_language(abs_path) else {
                    continue;
                };
                let language_id = crate::lsp::servers::lsp_language_id(lang);

                // Try LSP first, fall back to tree-sitter if unavailable
                let mut symbols = if let Ok(client) = ctx.lsp.get_or_start(lang, &root).await {
                    client
                        .document_symbols(abs_path, language_id)
                        .await
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                // Tree-sitter fallback when LSP is unavailable or returned nothing
                if symbols.is_empty() {
                    symbols = crate::ast::extract_symbols(abs_path).unwrap_or_default();
                }

                if symbols.is_empty() {
                    continue;
                }

                let source = if include_body {
                    std::fs::read_to_string(abs_path).ok()
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
                            false,
                        )
                    })
                    .collect();
                let mut entry = json!({
                    "file": display_path,
                    "symbols": json_symbols,
                });
                if include_docs {
                    entry["docstrings"] = json!(collect_docstrings(abs_path));
                }
                result.push(entry);
            }
            let mut result_json = json!({ "directory": rel_path, "files": result });
            if let Some(ov) = file_overflow {
                result_json["overflow"] = OutputGuard::overflow_json(&ov);
            }
            Ok(result_json)
        } else {
            Err(RecoverableError::with_hint(
                format!(
                    "path is neither file nor directory: {}",
                    full_path.display()
                ),
                "Verify the path exists with list_dir.",
            )
            .into())
        }
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_symbols(result))
    }
}

// ── find_symbol ────────────────────────────────────────────────────────────

pub struct FindSymbol;

const FIND_SYMBOL_MAX_RESULTS: usize = 50;
const BY_FILE_CAP: usize = 15;

/// Build a per-file distribution from a list of symbol JSON objects.
/// Returns (entries sorted by count desc, number of files omitted by cap).
fn build_by_file(matches: &[Value]) -> (Vec<(String, usize)>, usize) {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in matches {
        if let Some(file) = m["file"].as_str() {
            *counts.entry(file.to_string()).or_default() += 1;
        }
    }
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let overflow = sorted.len().saturating_sub(BY_FILE_CAP);
    sorted.truncate(BY_FILE_CAP);
    (sorted, overflow)
}

/// Build the actionable overflow hint for find_symbol. Uses the top file from by_file
/// as the concrete example path so the hint is copy-paste ready.
fn make_find_symbol_hint(shown: usize, by_file: &[(String, usize)]) -> String {
    let top_file = by_file
        .first()
        .map(|(f, _)| f.as_str())
        .unwrap_or("path/to/file.rs");
    format!(
        "Showing {shown} of total. To narrow down:\n\
         \u{2022} paginate:       add offset={shown}, limit=50\n\
         \u{2022} filter by file: add path=\"{top_file}\"\n\
         \u{2022} filter by kind: add kind=\"function\" (also: class, struct, interface, type, enum, module, constant)"
    )
}

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
                "kind": {
                    "type": "string",
                    "description": "Filter by symbol kind. Only applied when using 'pattern' — ignored with 'name_path'. Note: 'interface' matches Rust traits.",
                    "enum": ["function", "class", "struct", "interface", "type", "enum", "module", "constant"]
                },
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
        let mut guard = OutputGuard::from_input(&input);
        // find_symbol uses a tighter exploring cap than the default 200.
        if matches!(guard.mode, OutputMode::Exploring) {
            guard.max_results = FIND_SYMBOL_MAX_RESULTS;
        }

        // kind filter only applies to pattern-based searches, not exact name_path lookups.
        let is_name_path = input["name_path"].is_string();
        let kind_filter: Option<&str> = if is_name_path {
            None
        } else {
            input["kind"].as_str()
        };

        let include_body = input["include_body"]
            .as_bool()
            .or_else(|| input["include_body"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or_else(|| guard.should_include_body());
        let depth = input["depth"].as_u64().unwrap_or(0) as usize;
        let scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        let root = ctx.agent.require_project_root().await?;
        let pattern_lower = pattern.to_lowercase();
        // Build the name predicate once: exact matching for name_path lookups,
        // case-insensitive substring matching for pattern searches.
        // Box<dyn Fn>: two different closure types must be held under one variable across a conditional; generics cannot express this at runtime.
        let name_ok: Box<dyn Fn(&SymbolInfo) -> bool + Send> = if is_name_path {
            let p = pattern.to_owned();
            Box::new(move |sym: &SymbolInfo| symbol_name_matches(sym, &p))
        } else {
            let p = pattern_lower.clone();
            Box::new(move |sym: &SymbolInfo| {
                sym.name.to_lowercase().contains(&p) || sym.name_path.to_lowercase().contains(&p)
            })
        };
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
                collect_matching(
                    &symbols,
                    name_ok.as_ref(),
                    include_body,
                    source.as_deref(),
                    depth,
                    true,
                    &mut matches,
                    kind_filter,
                );
            }
        } else {
            if scope.includes_project() {
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
                        let name_ok = sym.name.to_lowercase().contains(&pattern_lower)
                            || sym.name_path.to_lowercase().contains(&pattern_lower);
                        let kind_ok =
                            kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f));
                        if name_ok && kind_ok {
                            // When include_body is requested, validate the range. If
                            // workspace/symbol returned a degenerate range, fall back to
                            // document_symbols for the file to get the correct range.
                            let sym = if include_body {
                                match validate_symbol_range(&sym) {
                                    Ok(()) => sym,
                                    Err(validation_err) => {
                                        match resolve_range_via_document_symbols(&sym, ctx).await {
                                            Some(resolved) => resolved,
                                            None => {
                                                // document_symbols fallback failed too — propagate
                                                // the original validation error captured above.
                                                return Err(validation_err);
                                            }
                                        }
                                    }
                                }
                            } else {
                                sym
                            };
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
                                true,
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
                                name_ok.as_ref(),
                                include_body,
                                source.as_deref(),
                                depth,
                                true,
                                &mut matches,
                                kind_filter,
                            );
                        }
                        // Early cap to avoid scanning entire huge projects
                        if matches.len() > guard.max_results {
                            break;
                        }
                    }
                }
            }

            // Search library directories when scope includes them
            let lib_roots = resolve_library_roots(&scope, &ctx.agent).await;
            for (lib_name, lib_root) in &lib_roots {
                if !lib_root.exists() {
                    continue;
                }
                // Library directories are external — don't apply the project's
                // .gitignore (e.g. .venv/ would hide pip-installed packages).
                let walker = ignore::WalkBuilder::new(lib_root)
                    .hidden(true)
                    .git_ignore(false)
                    .build();
                for entry in walker.flatten() {
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    let path = entry.path();
                    let Some(lang) = ast::detect_language(path) else {
                        continue;
                    };

                    // Tree-sitter first for library files: it's fast and avoids blocking
                    // on slow LSP startup (e.g. JVM-based Kotlin LSP). Only fall back to
                    // LSP document_symbols if tree-sitter returns nothing.
                    let mut symbols = crate::ast::extract_symbols(path).unwrap_or_default();
                    if symbols.is_empty() {
                        // INVARIANT: Always use project root as workspace_root, not the
                        // library root. LspManager caches one client per language; passing
                        // a different root kills and restarts the server.
                        if let Ok(client) = ctx.lsp.get_or_start(lang, &root).await {
                            let language_id = crate::lsp::servers::lsp_language_id(lang);
                            symbols = client
                                .document_symbols(path, language_id)
                                .await
                                .unwrap_or_default();
                        }
                    }

                    let source = if include_body {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    };

                    // Collect matching symbols, rewriting file paths to lib: prefix
                    for sym in &symbols {
                        if name_ok(sym)
                            && kind_filter.map_or(true, |f| matches_kind_filter(&sym.kind, f))
                        {
                            let mut json_val =
                                symbol_to_json(sym, include_body, source.as_deref(), depth, true);
                            if let Some(obj) = json_val.as_object_mut() {
                                obj.insert(
                                    "file".to_string(),
                                    json!(format_library_path(lib_name, lib_root, path)),
                                );
                            }
                            matches.push(json_val);
                        }
                    }

                    if matches.len() > guard.max_results * 2 {
                        break;
                    }
                }
            }
        }

        // Build by_file distribution from the full result set BEFORE truncation.
        let (by_file_entries, by_file_overflow_count) = build_by_file(&matches);
        let hint = if matches.len() > guard.max_results {
            make_find_symbol_hint(guard.max_results, &by_file_entries)
        } else {
            String::from("Restrict with a file path or glob pattern")
        };
        let (mut matches, mut overflow) = guard.cap_items(matches, &hint);
        // Patch by_file into the overflow object (RF6 resolution: mutate after cap_items).
        if let Some(ref mut ov) = overflow {
            if !by_file_entries.is_empty() {
                ov.by_file = Some(by_file_entries);
                ov.by_file_overflow = by_file_overflow_count;
                // Rewrite hint with the real `shown` value now we know it.
                ov.hint = make_find_symbol_hint(ov.shown, ov.by_file.as_deref().unwrap_or(&[]));
            }
        }

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

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_find_symbol(result))
    }

    fn json_path_hint(&self, val: &Value) -> String {
        let has_body = val["symbols"]
            .as_array()
            .and_then(|a| a.first())
            .map(|s| s["body"].is_string())
            .unwrap_or(false);
        if has_body {
            "$.symbols[0].body".to_string()
        } else {
            "$.symbols".to_string()
        }
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
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        let full_path = resolve_read_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Find the symbol's position by walking document symbols
        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_unique_symbol_by_name_path(&symbols, name_path)?;

        // Get references at the symbol's position
        let refs = client
            .references(&full_path, sym.start_line, sym.start_col, &lang)
            .await?;

        let root = ctx.agent.require_project_root().await?;

        // Resolve all library roots for classification (Scope::All to get every lib).
        let lib_roots = resolve_library_roots(&crate::library::scope::Scope::All, &ctx.agent).await;

        // Filter out references inside build-artifact directories. LSP servers often
        // index generated files (target/, node_modules/, dist/, …) and including them
        // creates noise without actionable information.
        let total_raw = refs.len();
        let refs: Vec<_> = refs
            .into_iter()
            .filter(|loc| {
                uri_to_path(loc.uri.as_str())
                    .map(|p| !path_in_excluded_dir(&p))
                    .unwrap_or(true)
            })
            .collect();
        let excluded = total_raw - refs.len();

        // Scope-filter references
        let refs: Vec<_> = refs
            .into_iter()
            .filter(|loc| {
                let Some(path) = uri_to_path(loc.uri.as_str()) else {
                    return true; // keep references we can't resolve
                };
                let (classification, _) = classify_reference_path(&path, &root, &lib_roots);
                match &scope {
                    crate::library::scope::Scope::Project => classification == "project",
                    crate::library::scope::Scope::Libraries => classification.starts_with("lib:"),
                    crate::library::scope::Scope::All => true,
                    crate::library::scope::Scope::Library(name) => {
                        classification == format!("lib:{}", name)
                    }
                }
            })
            .collect();

        let locations: Vec<Value> = refs
            .iter()
            .map(|loc| {
                let file = uri_to_path(loc.uri.as_str())
                    .map(|p| {
                        let (_, display) = classify_reference_path(&p, &root, &lib_roots);
                        display
                    })
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

        let guard = OutputGuard::from_input(&input);
        let total = locations.len();
        let (locations, overflow) = guard.cap_items(locations, "This symbol has many references. Use detail_level='full' with offset/limit to paginate");
        let mut result = json!({ "references": locations, "total": total });
        if excluded > 0 {
            result["excluded_from_build_dirs"] = json!(excluded);
        }
        if let Some(ov) = overflow {
            result["overflow"] = OutputGuard::overflow_json(&ov);
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_find_references(result))
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
        let line_1 = super::require_u64_param(&input, "line")? as u32;
        if line_1 == 0 {
            return Err(RecoverableError::with_hint(
                "'line' must be >= 1 (1-indexed)",
                "Line numbers are 1-indexed. Use line: 1 for the first line.",
            )
            .into());
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

            let mut def = json!({
                "file": file_display,
                "line": loc.range.start.line + 1,
                "end_line": loc.range.end.line + 1,
                "context": context.trim(),
            });
            if source_tag != "project" {
                def["source"] = json!(source_tag);
            }
            // Nudge if library was discovered but not indexed
            if let Some(lib_name) = source_tag.strip_prefix("lib:") {
                if ctx.agent.should_nudge(lib_name).await {
                    def["library_hint"] = json!({
                        "name": lib_name,
                        "status": "not_indexed",
                        "hint": format!("Library '{}' discovered but not indexed. Run index_project(scope='lib:{}') to enable semantic search.", lib_name, lib_name)
                    });
                }
                ctx.agent.maybe_auto_index_library(lib_name).await;
            }
            results.push(def);
        }

        Ok(json!({
            "definitions": results,
            "from": format!("{}:{}", full_path.file_name().unwrap_or_default().to_string_lossy(), line_1),
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_goto_definition(result))
    }
}

const HOVER_SKIP_TOKENS: &[&str] = &[
    "pub", "async", "unsafe", "extern", "default", "override", "fn", "struct", "enum", "trait",
    "impl", "type", "const", "static", "mod", "use", "dyn", "for", "where", "mut", "ref", "let",
];

/// Find the byte column of the first non-keyword identifier on `line`.
/// Skips Rust visibility and declaration keywords (`pub`, `fn`, `struct`, …)
/// so that `hover` on a declaration line lands on the symbol name, not `pub`.
fn find_first_symbol_col(line: &str) -> u32 {
    let bytes = line.as_bytes();
    let mut pos = 0usize;
    loop {
        // Skip non-identifier characters (whitespace, parens, angle brackets, etc.)
        while pos < bytes.len() && !bytes[pos].is_ascii_alphabetic() && bytes[pos] != b'_' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let start = pos;
        while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
            pos += 1;
        }
        let token = &line[start..pos];
        if !HOVER_SKIP_TOKENS.contains(&token) {
            return start as u32;
        }
    }
    // Fallback: first non-whitespace character
    line.chars().take_while(|c| c.is_whitespace()).count() as u32
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
        let line_1 = super::require_u64_param(&input, "line")? as u32;
        if line_1 == 0 {
            return Err(RecoverableError::with_hint(
                "'line' must be >= 1 (1-indexed)",
                "Line numbers are 1-indexed. Use line: 1 for the first line.",
            )
            .into());
        }
        let line_0 = line_1 - 1;
        let identifier = input["identifier"].as_str();

        let full_path = resolve_read_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Determine column: find identifier on the line, or skip modifiers to find first symbol
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
            find_first_symbol_col(source_line)
        };

        let hover_text = client.hover(&full_path, line_0, col, &lang).await?;

        match hover_text {
            Some(text) => {
                let root = ctx.agent.require_project_root().await?;
                let source_tag = tag_external_path(&full_path, &root, &ctx.agent).await;
                let mut result = json!({
                    "content": text,
                    "location": format!("{}:{}", full_path.file_name().unwrap_or_default().to_string_lossy(), line_1),
                });
                if source_tag != "project" {
                    result["source"] = json!(source_tag);
                }
                // Nudge if library was discovered but not indexed
                if let Some(lib_name) = source_tag.strip_prefix("lib:") {
                    if ctx.agent.should_nudge(lib_name).await {
                        result["library_hint"] = json!({
                            "name": lib_name,
                            "status": "not_indexed",
                            "hint": format!("Library '{}' discovered but not indexed. Run index_project(scope='lib:{}') to enable semantic search.", lib_name, lib_name)
                        });
                    }
                    ctx.agent.maybe_auto_index_library(lib_name).await;
                }
                Ok(result)
            }
            None => Err(RecoverableError::with_hint(
                format!("no hover info at {}:{}", full_path.display(), line_1),
                "The LSP has no type/doc info at this position. \
             Try specifying an 'identifier' parameter, or use find_symbol for name-based lookup",
            )
            .into()),
        }
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_hover(result))
    }
}

/// Walk upward from a symbol’s start line to find the insertion point that
/// lands before any doc comments and attributes. Used ONLY for insert_code(before)
/// positioning — never for modifying a symbol’s own range.
///
/// Krait-style: language-agnostic, recognizes doc comments and attributes from
/// Rust (#[...]), Python/Java/TS (@decorator), JSDoc/JavaDoc (/** ... */),
/// and Rust module-level doc comments (//!). Does NOT consume blank lines —
/// blank lines are structural separators and are left in place.
/// Compute the true start of a symbol declaration for editing (remove/replace).
///
/// Uses the LSP `range.start` (which includes attributes, doc comments, decorators)
/// when available. Falls back to the heuristic `find_insert_before_line` when the
/// LSP doesn't provide a separate full range (workspace/symbol, tree-sitter).
fn editing_start_line(sym: &crate::lsp::SymbolInfo, lines: &[&str]) -> usize {
    sym.range_start_line
        .map(|r| r as usize)
        .unwrap_or_else(|| find_insert_before_line(lines, sym.start_line as usize))
}
/// Get the true end line for write operations (insert_code after, replace_symbol).
///
/// Uses AST to cap `sym.end_line` when the LSP over-extends the symbol range
/// into the next symbol's opening line — a common rust-analyzer pattern where
/// `range.end.line` points to `fn following() {` instead of the current `}`.
///
/// When AST finds the symbol and reports `ast_end < sym.end_line`, we trust the
/// AST (it always terminates at the real `}`). Otherwise we trust the LSP.
/// This is the symmetric counterpart to `editing_start_line`.
fn editing_end_line(sym: &crate::lsp::SymbolInfo) -> u32 {
    let Ok(ast_syms) = crate::ast::extract_symbols(&sym.file) else {
        return sym.end_line;
    };
    if let Some(ast_end) = find_ast_end_line_in(&ast_syms, &sym.name, sym.start_line) {
        if ast_end < sym.end_line {
            return ast_end; // LSP over-extends; trust AST
        }
    }
    sym.end_line
}

/// Walk backwards from `symbol_start` past attributes, decorators, and doc comments.
///
/// This is the **fallback** heuristic used when the LSP doesn't provide a separate
/// `range.start` (workspace/symbol, tree-sitter). The primary mechanism is
/// `editing_start_line` which uses `range_start_line` from `documentSymbol`.
///
/// Handles:
/// - Single-line attributes: `#[test]`, `#[derive(Debug)]`
/// - Multi-line attributes: `#[cfg(\n    ...\n)]` (tracks bracket nesting)
/// - Python/Java decorators: `@decorator`, `@app.route("/path")`
/// - Doc comments: `///`, `//!`, `/** ... */`
/// - Block comments: `/* ... */` (multi-line)
fn find_insert_before_line(lines: &[&str], symbol_start: usize) -> usize {
    let mut cursor = symbol_start;
    // Track unclosed brackets when scanning upward through multi-line attributes.
    // When we see `)` or `]` without a matching opener on the same line, we know
    // we're inside a multi-line attribute and must keep scanning up.
    let mut pending_open_brackets: usize = 0;

    while cursor > 0 {
        let trimmed = lines[cursor - 1].trim();

        // If we're inside a multi-line attribute (have pending brackets to close),
        // keep scanning upward regardless of what the line looks like.
        if pending_open_brackets > 0 {
            // Count bracket balance on this line (scanning left-to-right)
            for ch in trimmed.chars() {
                match ch {
                    '(' | '[' => {
                        pending_open_brackets = pending_open_brackets.saturating_sub(1);
                    }
                    ')' | ']' => pending_open_brackets += 1,
                    _ => {}
                }
            }
            cursor -= 1;
            continue;
        }

        let is_attr_or_doc = trimmed.starts_with("#[")
            || trimmed.starts_with('@')
            || trimmed.starts_with("///")
            || trimmed.starts_with("//!")
            || trimmed.starts_with("/**")
            || trimmed.starts_with("* ")
            || trimmed == "*/"
            || trimmed.starts_with("/*");

        // Lines consisting purely of closing brackets (e.g. `)]`, `)`, `])`)
        // are continuations of multi-line attributes — they close the bracket
        // opened on a `#[attr(` line above.
        let is_bracket_closer =
            !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, ')' | ']'));

        if is_attr_or_doc || is_bracket_closer {
            // Check if this line has unmatched close brackets — indicates the
            // start of a multi-line attribute above this line.
            let mut depth: isize = 0;
            for ch in trimmed.chars() {
                match ch {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    _ => {}
                }
            }
            // Negative depth means more closers than openers — multi-line continues up
            if depth < 0 {
                pending_open_brackets = (-depth) as usize;
            }
            cursor -= 1;
        } else {
            break;
        }
    }
    cursor
}

pub struct ReplaceSymbol;

#[async_trait::async_trait]
impl Tool for ReplaceSymbol {
    fn name(&self) -> &str {
        "replace_symbol"
    }
    fn description(&self) -> &str {
        "Replace the entire body of a named symbol with new source code. \
         new_body should include the full declaration: attributes, doc comments, \
         signature, and body — matching what find_symbol(include_body=true) returns."
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
        super::guard_worktree_write(ctx).await?;
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let new_body = super::require_str_param(&input, "new_body")?;

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_unique_symbol_by_name_path(&symbols, name_path)?;

        // Validate: catch degenerate LSP ranges (start == end for multi-line symbols)
        validate_symbol_range(sym)?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let start = editing_start_line(sym, &lines);
        let end = (editing_end_line(sym) as usize + 1).min(lines.len());

        if start >= lines.len() {
            return Err(RecoverableError::with_hint(
                format!(
                    "symbol range out of bounds: start line {} but file has {} lines",
                    start + 1,
                    lines.len(),
                ),
                "The LSP may have stale data. Try list_symbols(path) to refresh.",
            )
            .into());
        }

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.extend(new_body.lines());
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        ctx.lsp.notify_file_changed(&full_path).await;
        ctx.agent.mark_file_dirty(full_path).await;
        Ok(json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_replace_symbol(result))
    }
}

// ── remove_symbol ──────────────────────────────────────────────────────────

pub struct RemoveSymbol;

#[async_trait::async_trait]
impl Tool for RemoveSymbol {
    fn name(&self) -> &str {
        "remove_symbol"
    }

    fn description(&self) -> &str {
        "Delete a symbol (function, struct, impl block, test, etc.) by name. Removes the lines covered by the LSP symbol range."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path"],
            "properties": {
                "name_path": { "type": "string", "description": "Symbol name path (e.g. 'MyStruct/my_method', 'tests/old_test')" },
                "path": { "type": "string", "description": "File path" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        super::guard_worktree_write(ctx).await?;
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_unique_symbol_by_name_path(&symbols, name_path)?;

        // Validate: catch degenerate LSP ranges
        validate_symbol_range(sym)?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let start = editing_start_line(sym, &lines);
        let end = (editing_end_line(sym) as usize + 1).min(lines.len());

        if start >= lines.len() {
            return Err(RecoverableError::with_hint(
                format!(
                    "symbol range out of bounds: start line {} but file has {} lines",
                    start + 1,
                    lines.len(),
                ),
                "The LSP may have stale data. Try list_symbols(path) to refresh.",
            )
            .into());
        }

        let mut new_lines: Vec<&str> = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.extend_from_slice(&lines[end..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        ctx.lsp.notify_file_changed(&full_path).await;
        ctx.agent.mark_file_dirty(full_path).await;
        let line_count = end - start;
        let removed_range = format!("{}-{}", start + 1, end);
        Ok(json!({
            "status": "ok",
            "removed_lines": removed_range,
            "line_count": line_count,
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_remove_symbol(result))
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
        super::guard_worktree_write(ctx).await?;
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let code = super::require_str_param(&input, "code")?;
        let position = input["position"].as_str().unwrap_or("after");

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_unique_symbol_by_name_path(&symbols, name_path)?;

        validate_symbol_range(sym)?;
        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let code_lines: Vec<&str> = code.lines().collect();
        let insert_at = match position {
            "before" => editing_start_line(sym, &lines),
            _ => (editing_end_line(sym) as usize + 1).min(lines.len()),
        };

        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&lines[..insert_at]);
        new_lines.extend(code_lines.iter().copied());
        if position == "before" {
            new_lines.push("");
        } else {
            let needs_blank = lines.get(insert_at).is_some_and(|l| !l.trim().is_empty());
            if needs_blank {
                new_lines.push("");
            }
        }
        new_lines.extend_from_slice(&lines[insert_at..]);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        ctx.lsp.notify_file_changed(&full_path).await;
        ctx.agent.mark_file_dirty(full_path).await;
        Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1, "position": position }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_insert_code(result))
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
        super::guard_worktree_write(ctx).await?;
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();
        let new_name = super::require_str_param(&input, "new_name")?;

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        // Find the symbol to get its position
        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_unique_symbol_by_name_path(&symbols, name_path)?;

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

        // Notify LSP of all changed files so its symbol state is refreshed.
        // Without this, list_symbols can still return old names even though the
        // file on disk is correct (stale textDocument cache in the LSP server).
        for path in &lsp_files {
            ctx.lsp.notify_file_changed(path).await;
            ctx.agent.mark_file_dirty(path.clone()).await;
        }

        // Phase 1.5: post-edit corruption scan.
        // If the LSP produced a wrong edit range (e.g. rust-analyzer off-by-N column), the
        // new name can end up embedded inside an existing token: "assertmy_new_fn()" instead
        // of "assert!(my_new_fn())". Detect this by checking whether any occurrence of
        // new_name in a changed file is immediately preceded by an alphanumeric character —
        // a separator (`_`, `(`, ` `, `:`, etc.) should always appear at a call/use site.
        let mut corruption_hints: Vec<Value> = vec![];
        if new_name.len() >= 4 {
            if let Ok(embedded_re) =
                regex::Regex::new(&format!(r"[a-zA-Z0-9]{}", regex::escape(new_name)))
            {
                for path in &lsp_files {
                    let Ok(content) = std::fs::read_to_string(path) else {
                        continue;
                    };
                    let rel = path
                        .strip_prefix(&rename_root)
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    let mut flagged_lines: Vec<u32> = vec![];
                    let mut previews: Vec<String> = vec![];
                    for (i, line) in content.lines().enumerate() {
                        if embedded_re.is_match(line) {
                            flagged_lines.push((i + 1) as u32);
                            if previews.len() < 3 {
                                previews.push(line.trim().to_string());
                            }
                        }
                    }
                    if !flagged_lines.is_empty() {
                        corruption_hints.push(json!({
                            "file": rel,
                            "lines": flagged_lines,
                            "previews": previews,
                        }));
                    }
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
            "verify_hint": "LSP rename may match occurrences inside string literals, comments, or macro arguments. Verify each changed file is still valid (e.g. cargo check / tsc --noEmit).",
        });
        if !corruption_hints.is_empty() {
            result["corruption_warning"] = json!(
                "new_name appears immediately after an alphanumeric character in the files \
                 below — the LSP may have applied an edit at the wrong column. Inspect \
                 these lines and run a build check (e.g. cargo check) before proceeding."
            );
            result["corruption_hints"] = json!(corruption_hints);
        }
        if let Some(reason) = sweep_skip_reason {
            result["sweep_skip_reason"] = json!(reason);
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_rename_symbol(result))
    }
}

// ── format_compact helpers ────────────────────────────────────────────────────

fn format_goto_definition(val: &Value) -> String {
    let defs = match val["definitions"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if defs.is_empty() {
        return String::new();
    }

    if defs.len() == 1 {
        let d = &defs[0];
        let file = d["file"].as_str().unwrap_or("?");
        let line = d["line"].as_u64().unwrap_or(0);
        let context = d["context"].as_str().unwrap_or("");
        let source = d["source"].as_str().unwrap_or("project");

        let mut out = if source != "project" {
            format!("{}:{} ({})", file, line, source)
        } else {
            format!("{}:{}", file, line)
        };

        if !context.is_empty() {
            out.push_str("\n\n  ");
            out.push_str(context);
        }
        return out;
    }

    let mut out = format!("{} definitions\n", defs.len());
    for d in defs {
        let file = d["file"].as_str().unwrap_or("?");
        let line = d["line"].as_u64().unwrap_or(0);
        let context = d["context"].as_str().unwrap_or("");
        let source = d["source"].as_str().unwrap_or("project");

        out.push_str("\n  ");
        out.push_str(&format!("{}:{}", file, line));
        if source != "project" {
            out.push_str(&format!(" ({})", source));
        }
        if !context.is_empty() {
            out.push_str(&format!("   {}", context));
        }
    }
    out
}

fn format_hover(val: &Value) -> String {
    let content = match val["content"].as_str() {
        Some(s) => s,
        None => return String::new(),
    };
    let location = val["location"].as_str().unwrap_or("");

    let mut out = String::new();
    if !location.is_empty() {
        out.push_str(location);
        out.push_str("\n\n");
    }

    let mut in_code_block = false;
    let mut first_content_line = true;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if !first_content_line {
            out.push('\n');
        }
        out.push_str("  ");
        out.push_str(line);
        first_content_line = false;
    }
    out
}

fn format_find_symbol(val: &Value) -> String {
    let symbols = match val["symbols"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let total = val["total"].as_u64().unwrap_or(symbols.len() as u64);

    if symbols.is_empty() {
        return "0 matches".to_string();
    }

    struct SymRow {
        kind: String,
        location: String,
        name_path: String,
        body: Option<String>,
    }

    let rows: Vec<SymRow> = symbols
        .iter()
        .map(|s| {
            let kind = s["kind"].as_str().unwrap_or("?").to_string();
            let file = s["file"].as_str().unwrap_or("?");
            let start = s["start_line"].as_u64().unwrap_or(0);
            let end = s["end_line"].as_u64().unwrap_or(0);
            let location = if end > start {
                format!("{file}:{start}-{end}")
            } else {
                format!("{file}:{start}")
            };
            let name_path = s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("?")
                .to_string();
            let body = s["body"].as_str().map(|b| b.to_string());
            SymRow {
                kind,
                location,
                name_path,
                body,
            }
        })
        .collect();

    let max_kind_len = rows.iter().map(|r| r.kind.len()).max().unwrap_or(0);
    let max_loc_len = rows.iter().map(|r| r.location.len()).max().unwrap_or(0);

    let match_word = if total == 1 { "match" } else { "matches" };
    let header = if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        let shown = overflow["shown"].as_u64().unwrap_or(symbols.len() as u64);
        format!("{shown} {match_word} ({total} total)")
    } else {
        format!("{total} {match_word}")
    };
    let mut out = format!("{header}\n");

    for row in &rows {
        let kind_pad = max_kind_len - row.kind.len();
        let loc_pad = max_loc_len - row.location.len();
        out.push_str("\n  ");
        out.push_str(&row.kind);
        for _ in 0..kind_pad {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(&row.location);
        for _ in 0..loc_pad {
            out.push(' ');
        }
        out.push_str("   ");
        out.push_str(&row.name_path);

        if let Some(body) = &row.body {
            // Short bodies are shown inline. Long bodies are replaced with a
            // navigation hint — embedding a 300-line function in the compact
            // summary only causes truncation mid-body, which misleads agents
            // into thinking the body is incomplete rather than available via
            // json_path. The threshold is intentionally well below the
            // COMPACT_SUMMARY_MAX_BYTES (2000) so even a single long function
            // leaves room for the rest of the summary.
            const INLINE_BODY_LIMIT: usize = 500;
            if body.len() <= INLINE_BODY_LIMIT {
                out.push('\n');
                for line in body.lines() {
                    out.push_str("\n      ");
                    out.push_str(line);
                }
            } else {
                let line_count = body.lines().count();
                out.push_str(&format!(
                    "\n      ({line_count}-line body — use json_path=\"$.symbols[0].body\" to extract)"
                ));
            }
        }
    }

    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

fn format_list_symbols(val: &Value) -> String {
    // File mode
    if let Some(file) = val["file"].as_str() {
        let symbols = match val["symbols"].as_array() {
            Some(arr) => arr,
            None => return String::new(),
        };
        let count = symbols.len();
        let sym_word = if count == 1 { "symbol" } else { "symbols" };
        let mut out = format!("{file} — {count} {sym_word}\n");
        format_symbol_tree(&mut out, symbols, 2);

        if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
        return out;
    }

    // Directory or pattern mode
    let dir = val["directory"]
        .as_str()
        .or_else(|| val["pattern"].as_str())
        .unwrap_or(".");
    let files = match val["files"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if files.is_empty() {
        return format!("{dir} — 0 symbols");
    }

    let mut out = format!("{dir}\n");

    for file_entry in files {
        let file = file_entry["file"].as_str().unwrap_or("?");
        let symbols = match file_entry["symbols"].as_array() {
            Some(arr) => arr,
            None => continue,
        };
        let count = symbols.len();
        let sym_word = if count == 1 { "symbol" } else { "symbols" };
        out.push_str(&format!("\n  {file} — {count} {sym_word}\n"));
        format_symbol_tree(&mut out, symbols, 4);
    }

    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

fn format_symbol_tree(out: &mut String, symbols: &[Value], indent: usize) {
    let max_kind_len = symbols
        .iter()
        .map(|s| s["kind"].as_str().unwrap_or("").len())
        .max()
        .unwrap_or(0);
    let max_name_len = symbols
        .iter()
        .map(|s| {
            s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("")
                .len()
        })
        .max()
        .unwrap_or(0);

    let pad = " ".repeat(indent);

    for sym in symbols {
        let kind = sym["kind"].as_str().unwrap_or("?");
        let name = sym["name_path"]
            .as_str()
            .or_else(|| sym["name"].as_str())
            .unwrap_or("?");
        let start = sym["start_line"].as_u64().unwrap_or(0);
        let end = sym["end_line"].as_u64().unwrap_or(0);
        let line_range = format_line_range(start, end);

        let kind_pad = max_kind_len - kind.len();
        let name_pad = max_name_len.saturating_sub(name.len());
        out.push('\n');
        out.push_str(&pad);
        out.push_str(kind);
        for _ in 0..kind_pad {
            out.push(' ');
        }
        out.push_str("   ");
        out.push_str(name);
        for _ in 0..name_pad {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(&line_range);

        if let Some(children) = sym["children"].as_array() {
            let child_indent = indent + 5;
            let child_pad = " ".repeat(child_indent);
            let max_child_name = children
                .iter()
                .map(|c| c["name"].as_str().unwrap_or("").len())
                .max()
                .unwrap_or(0);

            for child in children {
                let child_kind = child["kind"].as_str().unwrap_or("?");
                let child_name = child["name"].as_str().unwrap_or("?");
                let cs = child["start_line"].as_u64().unwrap_or(0);
                let ce = child["end_line"].as_u64().unwrap_or(0);
                let child_lr = format_line_range(cs, ce);
                let child_name_pad = max_child_name.saturating_sub(child_name.len());

                out.push('\n');
                out.push_str(&child_pad);

                if child_kind == "EnumMember" || child_kind == "Field" {
                    out.push_str(child_name);
                    for _ in 0..child_name_pad {
                        out.push(' ');
                    }
                } else {
                    out.push_str(child_kind);
                    out.push_str("  ");
                    out.push_str(child_name);
                    for _ in 0..child_name_pad {
                        out.push(' ');
                    }
                }
                out.push_str("  ");
                out.push_str(&child_lr);
            }
        }
    }
}

fn format_find_references(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or_else(|| {
        result["references"]
            .as_array()
            .map(|a| a.len() as u64)
            .unwrap_or(0)
    });

    if total == 0 {
        return "No references found.".to_string();
    }

    let refs = match result["references"].as_array() {
        Some(r) => r,
        None => return format!("{total} refs"),
    };

    const MAX_SHOW: usize = 5;
    let mut out = format!("{total} refs");
    for r in refs.iter().take(MAX_SHOW) {
        let file = r["file"].as_str().unwrap_or("?");
        let line = r["line"].as_u64().unwrap_or(0);
        out.push_str(&format!("\n  {file}:{line}"));
    }
    let shown = refs.len().min(MAX_SHOW);
    let hidden = (total as usize).saturating_sub(shown);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more"));
    }
    out
}

fn format_replace_symbol(result: &Value) -> String {
    let lines = result["replaced_lines"].as_str().unwrap_or("?");
    format!("replaced · L{lines}")
}

fn format_remove_symbol(result: &Value) -> String {
    let lines = result["removed_lines"].as_str().unwrap_or("?");
    let count = result["line_count"].as_u64().unwrap_or(0);
    format!("removed · L{lines} ({count} lines)")
}

fn format_insert_code(result: &Value) -> String {
    let line = result["inserted_at_line"].as_u64().unwrap_or(0);
    let pos = result["position"].as_str().unwrap_or("after");
    format!("inserted {pos} L{line}")
}

fn format_rename_symbol(result: &Value) -> String {
    let total_edits = result["total_edits"].as_u64().unwrap_or(0);
    let textual = result["textual_match_count"].as_u64().unwrap_or(0);
    let total = total_edits + textual;
    let new_name = result["new_name"].as_str().unwrap_or("?");
    let files = result["files_changed"].as_u64().unwrap_or(0);
    if files <= 1 {
        format!("→ {new_name} · {total} sites")
    } else {
        format!("→ {new_name} · {total} sites · {files} files")
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
    // Atomic write: write to a sibling .tmp file then rename so a crash or
    // disk-full condition mid-write can't leave the file in a corrupt state.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e
    })
}

/// Walk the symbol tree to find a symbol by name_path (e.g. "MyStruct/my_method").
/// Check if a symbol matches a query by name or name_path.
///
/// Exact match takes priority. Falls back to a prefix check for generic types
/// so that e.g. `IRepository<T, ID>` matches query `IRepository`, and
/// `impl Tool for MyStruct<T>` matches query `MyStruct<T>` or `MyStruct`.
fn symbol_name_matches(sym: &SymbolInfo, query: &str) -> bool {
    if sym.name_path == query || sym.name == query {
        return true;
    }
    // Generic prefix: "Foo<T>" matches query "Foo" when followed by '<', '(', or ' '
    for candidate in [sym.name.as_str(), sym.name_path.as_str()] {
        if candidate.starts_with(query) {
            if let Some(&next) = candidate.as_bytes().get(query.len()) {
                if matches!(next, b'<' | b'(' | b' ') {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
fn find_symbol_by_name_path<'a>(
    symbols: &'a [SymbolInfo],
    name_path: &str,
) -> Option<&'a SymbolInfo> {
    for sym in symbols {
        if symbol_name_matches(sym, name_path) {
            return Some(sym);
        }
        if let Some(found) = find_symbol_by_name_path(&sym.children, name_path) {
            return Some(found);
        }
    }
    None
}

/// Like [`find_symbol_by_name_path`] but errors on ambiguous matches.
///
/// Returns `Ok(&SymbolInfo)` when exactly one symbol matches `name_path`.
/// Returns `Err(RecoverableError)` when:
/// - No symbol matches (not found)
/// - Multiple symbols match (ambiguous bare name) — the error lists all
///   full `name_path`s so the caller can supply a more specific query.
fn find_unique_symbol_by_name_path<'a>(
    symbols: &'a [SymbolInfo],
    name_path: &str,
) -> anyhow::Result<&'a SymbolInfo> {
    let mut matches = collect_matching_symbols(symbols, name_path);
    match matches.len() {
        0 => Err(RecoverableError::with_hint(
            format!("symbol not found: {name_path}"),
            "Use list_symbols(path) to see available symbols, or check the name_path spelling.",
        )
        .into()),
        1 => Ok(matches.remove(0)),
        _ => {
            let paths: Vec<String> = matches.iter().map(|s| s.name_path.clone()).collect();
            Err(RecoverableError::with_hint(
                format!(
                    "ambiguous name_path \"{name_path}\" matches {} symbols: {}",
                    paths.len(),
                    paths.join(", ")
                ),
                "Provide the full name_path (e.g. \"StructName/method_name\") to disambiguate.",
            )
            .into())
        }
    }
}

/// Collect all symbols matching `name_path` (depth-first, including children).
fn collect_matching_symbols<'a>(symbols: &'a [SymbolInfo], name_path: &str) -> Vec<&'a SymbolInfo> {
    let mut results = Vec::new();
    for sym in symbols {
        if symbol_name_matches(sym, name_path) {
            results.push(sym);
        }
        results.extend(collect_matching_symbols(&sym.children, name_path));
    }
    results
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

/// Returns `true` if any component of `path` is a well-known build-artifact directory.
/// Used by `find_references` to suppress noise from generated/vendored code.
fn path_in_excluded_dir(path: &std::path::Path) -> bool {
    const EXCLUDED: &[&str] = &[
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "out",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        "vendor",
        ".gradle",
        ".idea",
        ".vscode",
    ];
    path.components().any(|c| {
        if let std::path::Component::Normal(name) = c {
            EXCLUDED.iter().any(|&ex| name == std::ffi::OsStr::new(ex))
        } else {
            false
        }
    })
}

/// Apply LSP TextEdits to a source string, returning the modified version.
///
/// Edits are applied from bottom to top to preserve line numbers.
/// Convert a UTF-16 code-unit offset (as returned by LSP) to a UTF-8 byte offset.
/// LSP specifies all `character` positions in UTF-16 code units; Rust's str uses UTF-8.
/// For ASCII-only lines these are equal, but any non-ASCII character causes divergence.
fn utf16_to_byte_offset(s: &str, utf16_offset: usize) -> usize {
    let mut byte_pos = 0;
    let mut utf16_pos = 0usize;
    for ch in s.chars() {
        if utf16_pos >= utf16_offset {
            break;
        }
        byte_pos += ch.len_utf8();
        utf16_pos += ch.len_utf16();
    }
    byte_pos.min(s.len())
}

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

        // LSP character offsets are UTF-16 code units; convert to byte offsets.
        let start_byte = utf16_to_byte_offset(&lines[start_line], start_char);
        let prefix = &lines[start_line][..start_byte];

        let suffix = if end_line < lines.len() {
            let end_byte = utf16_to_byte_offset(&lines[end_line], end_char);
            &lines[end_line][end_byte..]
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
        if let Some(project) = inner.active_project_mut() {
            project.library_registry.register(
                discovered.name,
                discovered.path,
                discovered.language,
                crate::library::registry::DiscoveryMethod::LspFollowThrough,
            );
            // Best-effort save — don't fail the tool call if this fails
            let registry_path = project.root.join(".codescout").join("libraries.json");
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
    use crate::tools::ToolContext;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn lsp() -> Arc<dyn crate::lsp::LspProvider> {
        crate::lsp::LspManager::new_arc()
    }

    fn buf() -> Arc<crate::tools::output_buffer::OutputBuffer> {
        Arc::new(crate::tools::output_buffer::OutputBuffer::new(20))
    }

    /// Substring predicate for `collect_matching` tests: case-insensitive match on name or name_path.
    fn substr_pred(pat: &'static str) -> impl Fn(&SymbolInfo) -> bool {
        move |sym: &SymbolInfo| {
            sym.name.to_lowercase().contains(pat) || sym.name_path.to_lowercase().contains(pat)
        }
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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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
        Some((
            dir,
            ToolContext {
                agent,
                lsp: lsp(),
                output_buffer: buf(),
                progress: None,
            },
        ))
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

    // ── validate_symbol_range tests ──────────────────────────────────────────

    /// Degenerate range (start == end) where tree-sitter confirms multi-line →
    /// validate_symbol_range must return Err with "suspicious range".
    #[test]
    fn validate_symbol_range_rejects_degenerate_range() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        // 3-line function (0-indexed lines 0..2)
        std::fs::write(&file, "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();

        let sym = SymbolInfo {
            name: "add".to_string(),
            name_path: "add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0, // degenerate — only the fn-name line
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(result.is_err(), "degenerate range should be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("suspicious range"),
            "error should mention suspicious range; got: {msg}"
        );
    }

    /// Non-degenerate range (start != end) → validate_symbol_range accepts it.
    #[test]
    fn validate_symbol_range_accepts_good_range() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();

        // When start != end (LSP returned a real range), accept it.
        let sym = SymbolInfo {
            name: "add".to_string(),
            name_path: "add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 5, // already a real range
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(result.is_ok(), "good range should be accepted");
    }

    /// Truncated end_line (end inside body, not at closing `}`) must be rejected.
    /// This is the BUG-018 pattern: start != end but end < AST end.
    #[test]
    fn validate_symbol_range_rejects_truncated_end_line() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        // 3-line function (0-indexed lines 0..2)
        std::fs::write(&file, "fn target() {\n    old_body();\n}\n").unwrap();

        let sym = SymbolInfo {
            name: "target".to_string(),
            name_path: "target".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 1, // truncated — inside body, misses closing `}` at line 2
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "truncated end_line should be rejected; got Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("suspicious range"),
            "error should mention suspicious range; got: {msg}"
        );
    }

    // ── validate_symbol_range: multi-language coverage ────────────────────────

    #[test]
    fn validate_symbol_range_rejects_degenerate_python() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.py");
        std::fs::write(
            &file,
            "def add(a, b):\n    result = a + b\n    return result\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "add".to_string(),
            name_path: "add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0, // degenerate
            start_col: 4,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "Python degenerate range should be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suspicious range"), "got: {msg}");
    }

    #[test]
    fn validate_symbol_range_rejects_degenerate_typescript() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.ts");
        std::fs::write(
            &file,
            "function add(a: number, b: number): number {\n    const result = a + b;\n    return result;\n}\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "add".to_string(),
            name_path: "add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0, // degenerate
            start_col: 9,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "TypeScript degenerate range should be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suspicious range"), "got: {msg}");
    }

    #[test]
    fn validate_symbol_range_rejects_degenerate_go() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.go");
        std::fs::write(
            &file,
            "package main\n\nfunc Add(a int, b int) int {\n\tresult := a + b\n\treturn result\n}\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "Add".to_string(),
            name_path: "Add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 2, // "func Add..." is line 2 (0-indexed)
            end_line: 2,   // degenerate
            start_col: 5,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(result.is_err(), "Go degenerate range should be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suspicious range"), "got: {msg}");
    }

    #[test]
    fn validate_symbol_range_rejects_degenerate_rust_with_doc() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        // Doc comment on line 0; fn keyword on line 1.
        std::fs::write(
            &file,
            "/// Adds two numbers.\nfn add(a: i32, b: i32) -> i32 {\n    let r = a + b;\n    r\n}\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "add".to_string(),
            name_path: "add".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 1, // fn keyword, not the doc comment
            end_line: 1,   // degenerate
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "Rust+doc comment degenerate range should be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suspicious range"), "got: {msg}");
    }

    #[test]
    fn validate_symbol_range_picks_correct_function() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        // `add` at lines 0-2, `multiply` at lines 4-6.
        std::fs::write(
            &file,
            "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn multiply(a: i32, b: i32) -> i32 {\n    a * b\n}\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "multiply".to_string(),
            name_path: "multiply".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 4,
            end_line: 4, // degenerate
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "degenerate multiply range should be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("multiply"),
            "error should name the symbol; got: {msg}"
        );
        assert!(msg.contains("suspicious range"), "got: {msg}");
    }

    #[test]
    fn validate_symbol_range_accepts_when_ast_unavailable() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();

        let sym = SymbolInfo {
            name: "nonexistent_fn".to_string(),
            name_path: "nonexistent_fn".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0, // degenerate, but name not in AST
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        // Name not in file — AST can't confirm anything, so we accept the range
        let result = validate_symbol_range(&sym);
        assert!(
            result.is_ok(),
            "unknown name: range should be accepted (no AST confirmation to the contrary)"
        );
    }

    #[test]
    fn validate_symbol_range_recurses_into_children() {
        use crate::lsp::SymbolKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        // `distance` is a method inside `impl Point` — it will be a child symbol.
        std::fs::write(
            &file,
            "struct Point { x: f64, y: f64 }\nimpl Point {\n    fn distance(&self) -> f64 {\n        (self.x * self.x + self.y * self.y).sqrt()\n    }\n}\n",
        )
        .unwrap();

        let sym = SymbolInfo {
            name: "distance".to_string(),
            name_path: "Point/distance".to_string(),
            kind: SymbolKind::Method,
            file: file.clone(),
            start_line: 2, // fn distance line (0-indexed)
            end_line: 2,   // degenerate
            start_col: 7,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let result = validate_symbol_range(&sym);
        assert!(
            result.is_err(),
            "method in impl with degenerate range should be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suspicious range"), "got: {msg}");
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
            output_buffer: buf(),
            progress: None,
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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
            output_buffer: buf(),
            progress: None,
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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn unique_benchmark_fn() -> i32 { 42 }\n\npub struct UniqueTestStruct { x: i32 }\n",
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn nested_function() -> i32 { 42 }\n",
        )
        .unwrap();
        // Also one at root for comparison
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        std::fs::write(dir.path().join("src/top.rs"), "pub fn top_level() {}\n").unwrap();
        std::fs::write(
            dir.path().join("src/deep/nested/hidden.rs"),
            "pub fn deeply_nested() {}\n",
        )
        .unwrap();

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
                range_start_line: None,
                detail: None,
            }],
            range_start_line: None,
            detail: None,
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
                range_start_line: None,
                detail: None,
            }],
            range_start_line: None,
            detail: None,
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

    #[test]
    fn symbol_name_matches_generic_types() {
        let make_sym = |name: &str, name_path: &str| SymbolInfo {
            name: name.to_string(),
            name_path: name_path.to_string(),
            kind: crate::lsp::SymbolKind::Struct,
            file: std::env::temp_dir().join("test.ts"),
            start_line: 0,
            end_line: 10,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let sym = make_sym("IRepository<T, ID>", "IRepository<T, ID>");
        // Exact match
        assert!(symbol_name_matches(&sym, "IRepository<T, ID>"));
        // Generic prefix match
        assert!(symbol_name_matches(&sym, "IRepository"));
        // Partial prefix must NOT match (would be "IRepo" → 's' next, not '<'/'('/' ')
        assert!(!symbol_name_matches(&sym, "IRepo"));

        // Parenthesis suffix (callable generic)
        let sym2 = make_sym("createStore()", "createStore()");
        assert!(symbol_name_matches(&sym2, "createStore"));
        assert!(!symbol_name_matches(&sym2, "create"));

        // Space suffix (e.g. "impl Trait for Struct<T>")
        let sym3 = make_sym("impl Tool for MyStruct<T>", "impl Tool for MyStruct<T>");
        assert!(symbol_name_matches(&sym3, "impl Tool for MyStruct<T>"));

        // Exact name still works with no suffix
        let sym4 = make_sym("PlainStruct", "PlainStruct");
        assert!(symbol_name_matches(&sym4, "PlainStruct"));
        assert!(!symbol_name_matches(&sym4, "Plain"));
    }

    #[test]
    fn find_symbol_by_name_path_generic_types() {
        let test_file = std::env::temp_dir().join("test.ts");
        let symbols = vec![
            SymbolInfo {
                name: "IRepository<T, ID>".to_string(),
                name_path: "IRepository<T, ID>".to_string(),
                kind: crate::lsp::SymbolKind::Interface,
                file: test_file.clone(),
                start_line: 0,
                end_line: 20,
                start_col: 0,
                children: vec![SymbolInfo {
                    name: "findById".to_string(),
                    name_path: "IRepository<T, ID>/findById".to_string(),
                    kind: crate::lsp::SymbolKind::Method,
                    file: test_file.clone(),
                    start_line: 2,
                    end_line: 3,
                    start_col: 4,
                    children: vec![],
                    range_start_line: None,
                    detail: None,
                }],
                range_start_line: None,
                detail: None,
            },
            SymbolInfo {
                name: "IRepositoryExtended".to_string(),
                name_path: "IRepositoryExtended".to_string(),
                kind: crate::lsp::SymbolKind::Interface,
                file: test_file,
                start_line: 22,
                end_line: 30,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
        ];

        // Bare query matches the generic type, not the similarly-named one
        let found = find_symbol_by_name_path(&symbols, "IRepository");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "IRepository<T, ID>");

        // "IRepositoryExtended" should NOT match query "IRepository" (different suffix char)
        let found_ext = find_symbol_by_name_path(&symbols, "IRepositoryExtended");
        assert!(found_ext.is_some());
        assert_eq!(found_ext.unwrap().name, "IRepositoryExtended");

        // Child method still reachable through generic parent
        let found_method = find_symbol_by_name_path(&symbols, "findById");
        assert!(found_method.is_some());
        assert_eq!(found_method.unwrap().name, "findById");
    }

    #[test]
    fn find_unique_symbol_by_name_path_errors_on_ambiguous_name() {
        let test_file = std::env::temp_dir().join("test.rs");
        let make_method = |parent: &str, name: &str| SymbolInfo {
            name: name.to_string(),
            name_path: format!("{}/{}", parent, name),
            kind: crate::lsp::SymbolKind::Function,
            file: test_file.clone(),
            start_line: 0,
            end_line: 5,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        };
        let symbols = vec![
            SymbolInfo {
                name: "ToolA".to_string(),
                name_path: "ToolA".to_string(),
                kind: crate::lsp::SymbolKind::Struct,
                file: test_file.clone(),
                start_line: 0,
                end_line: 20,
                start_col: 0,
                children: vec![make_method("ToolA", "call")],
                range_start_line: None,
                detail: None,
            },
            SymbolInfo {
                name: "ToolB".to_string(),
                name_path: "ToolB".to_string(),
                kind: crate::lsp::SymbolKind::Struct,
                file: test_file.clone(),
                start_line: 25,
                end_line: 45,
                start_col: 0,
                children: vec![make_method("ToolB", "call")],
                range_start_line: None,
                detail: None,
            },
        ];

        // Baseline (the bug): old find_symbol_by_name_path silently returns the first
        // depth-first match for a bare name — caller has no way to know it was ambiguous.
        let old_result = find_symbol_by_name_path(&symbols, "call");
        assert!(
            old_result.is_some(),
            "old function returns Some for ambiguous name — no error, caller is unaware"
        );
        assert_eq!(
            old_result.unwrap().name_path,
            "ToolA/call",
            "old function returns first depth-first match, silently ignoring ToolB/call"
        );

        // Stale → Fixed: find_unique_symbol_by_name_path detects ambiguity and errors,
        // listing all matching name_paths so the caller can supply a more specific query.
        let result = find_unique_symbol_by_name_path(&symbols, "call");
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("ToolA/call"),
            "expected ToolA/call in error, got: {err_str}"
        );
        assert!(
            err_str.contains("ToolB/call"),
            "expected ToolB/call in error, got: {err_str}"
        );

        // Fresh: supplying the full name_path resolves the ambiguity unambiguously
        let result = find_unique_symbol_by_name_path(&symbols, "ToolA/call");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name_path, "ToolA/call");

        // Not found → RecoverableError mentioning the query
        let result = find_unique_symbol_by_name_path(&symbols, "nonexistent");
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("nonexistent"),
            "expected symbol name in error, got: {err_str}"
        );
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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        };

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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let root = agent.require_project_root().await.unwrap();
        let internal = root.join("src/main.rs");
        let tag = tag_external_path(&internal, &root, &agent).await;
        assert_eq!(tag, "project");
    }

    #[tokio::test]
    async fn tag_external_path_discovers_and_registers() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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
                range_start_line: None,
                detail: None,
            }],
            range_start_line: None,
            detail: None,
        }];

        // Pattern with "/" should match via name_path
        let mut results = vec![];
        collect_matching(
            &symbols,
            &substr_pred("mystruct/my_method"),
            false,
            None,
            0,
            true,
            &mut results,
            None,
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
            &substr_pred("my_method"),
            false,
            None,
            0,
            true,
            &mut results2,
            None,
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
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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
        (
            dir,
            ToolContext {
                agent,
                lsp: lsp(),
                output_buffer: buf(),
                progress: None,
            },
        )
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
                range_start_line: None,
                detail: None,
            }],
            range_start_line: None,
            detail: None,
        }];

        let mut results = vec![];
        collect_matching(
            &symbols,
            &substr_pred("mystruct/my_method"),
            false,
            None,
            0,
            true,
            &mut results,
            None,
        );
        assert_eq!(
            results.len(),
            1,
            "slash pattern should match exactly 1 result (the method), not the parent struct"
        );
        assert_eq!(results[0]["name"], "my_method");
    }

    #[test]
    fn matches_kind_filter_function_group() {
        use crate::lsp::SymbolKind;
        assert!(matches_kind_filter(&SymbolKind::Function, "function"));
        assert!(matches_kind_filter(&SymbolKind::Method, "function"));
        assert!(matches_kind_filter(&SymbolKind::Constructor, "function"));
        assert!(!matches_kind_filter(&SymbolKind::Variable, "function"));
        assert!(!matches_kind_filter(&SymbolKind::Class, "function"));
    }

    #[test]
    fn matches_kind_filter_struct_vs_class() {
        use crate::lsp::SymbolKind;
        assert!(matches_kind_filter(&SymbolKind::Class, "class"));
        assert!(!matches_kind_filter(&SymbolKind::Struct, "class"));
        assert!(matches_kind_filter(&SymbolKind::Struct, "struct"));
        assert!(!matches_kind_filter(&SymbolKind::Class, "struct"));
    }

    #[test]
    fn matches_kind_filter_module_group() {
        use crate::lsp::SymbolKind;
        assert!(matches_kind_filter(&SymbolKind::Module, "module"));
        assert!(matches_kind_filter(&SymbolKind::Namespace, "module"));
        assert!(matches_kind_filter(&SymbolKind::Package, "module"));
        assert!(!matches_kind_filter(&SymbolKind::Function, "module"));
    }

    #[test]
    fn collect_matching_with_kind_filter_class_only() {
        use crate::lsp::SymbolKind;
        let symbols = vec![
            SymbolInfo {
                name: "WeeklyGrid".into(),
                name_path: "WeeklyGrid".into(),
                kind: SymbolKind::Class,
                file: PathBuf::from("test.ts"),
                start_line: 0,
                end_line: 10,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
            SymbolInfo {
                name: "weeklyGrid".into(),
                name_path: "weeklyGrid".into(),
                kind: SymbolKind::Variable,
                file: PathBuf::from("test.ts"),
                start_line: 12,
                end_line: 12,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
            SymbolInfo {
                name: "renderWeeklyGrid".into(),
                name_path: "renderWeeklyGrid".into(),
                kind: SymbolKind::Function,
                file: PathBuf::from("test.ts"),
                start_line: 14,
                end_line: 20,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
        ];

        let mut out = vec![];
        collect_matching(
            &symbols,
            &substr_pred("weeklygrid"),
            false,
            None,
            0,
            true,
            &mut out,
            Some("class"),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "WeeklyGrid");
    }

    #[test]
    fn collect_matching_kind_filter_none_returns_all_matching() {
        use crate::lsp::SymbolKind;
        let symbols = vec![
            SymbolInfo {
                name: "foo".into(),
                name_path: "foo".into(),
                kind: SymbolKind::Function,
                file: PathBuf::from("test.rs"),
                start_line: 0,
                end_line: 5,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
            SymbolInfo {
                name: "FOO".into(),
                name_path: "FOO".into(),
                kind: SymbolKind::Constant,
                file: PathBuf::from("test.rs"),
                start_line: 7,
                end_line: 7,
                start_col: 0,
                children: vec![],
                range_start_line: None,
                detail: None,
            },
        ];

        let mut out = vec![];
        collect_matching(
            &symbols,
            &substr_pred("foo"),
            false,
            None,
            0,
            true,
            &mut out,
            None,
        );
        assert_eq!(
            out.len(),
            2,
            "no filter → all name-matching symbols returned"
        );
    }

    #[test]
    fn build_by_file_sorts_desc_and_caps_at_15() {
        // 20 distinct files, file_i has (20 - i) matches
        let mut matches: Vec<Value> = vec![];
        for i in 0usize..20 {
            for _ in 0..(20 - i) {
                matches.push(json!({ "file": format!("src/file{i}.rs") }));
            }
        }
        let (by_file, overflow) = build_by_file(&matches);
        assert_eq!(by_file.len(), 15, "cap at 15");
        assert_eq!(overflow, 5, "20 files - 15 = 5 overflow");
        // First entry has highest count
        assert_eq!(by_file[0].0, "src/file0.rs");
        assert_eq!(by_file[0].1, 20);
        // Sorted descending
        for w in by_file.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn build_by_file_no_overflow_under_cap() {
        let matches: Vec<Value> = (0..3)
            .flat_map(|i| vec![json!({ "file": format!("src/f{i}.rs") }); 5])
            .collect();
        let (by_file, overflow) = build_by_file(&matches);
        assert_eq!(by_file.len(), 3);
        assert_eq!(overflow, 0);
    }

    #[test]
    fn make_find_symbol_hint_contains_top_file_and_kind_and_offset() {
        let by_file = vec![
            ("src/components/WeeklyGrid.tsx".to_string(), 12usize),
            ("src/screens/Home.tsx".to_string(), 3),
        ];
        let hint = make_find_symbol_hint(50, &by_file);
        assert!(
            hint.contains("src/components/WeeklyGrid.tsx"),
            "should show top file path"
        );
        assert!(hint.contains("kind="), "should mention kind filter");
        assert!(
            hint.contains("offset=50"),
            "should show next pagination offset"
        );
    }

    #[test]
    fn kind_filter_skipped_when_using_name_path() {
        // Verify the logic: if name_path is set, kind_filter is None.
        let input = json!({ "name_path": "Foo", "kind": "function" });
        let is_name_path = input["name_path"].is_string();
        let kind_filter: Option<&str> = if is_name_path {
            None
        } else {
            input["kind"].as_str()
        };
        assert!(kind_filter.is_none());
    }

    // ── symbol_to_json field contract ────────────────────────────────────────

    fn make_test_sym(name: &str, detail: Option<&str>) -> crate::lsp::SymbolInfo {
        crate::lsp::SymbolInfo {
            name: name.to_string(),
            name_path: name.to_string(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/foo.rs"),
            start_line: 0,
            end_line: 5,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: detail.map(|s| s.to_string()),
        }
    }

    #[test]
    fn symbol_to_json_omits_file_when_show_file_false() {
        let sym = make_test_sym("foo", None);
        let result = symbol_to_json(&sym, false, None, 0, false);
        assert!(
            result.get("file").is_none(),
            "file must be absent when show_file=false, got: {result}"
        );
        assert_eq!(result["name"], "foo");
    }

    #[test]
    fn symbol_to_json_field_order_name_kind_before_line_numbers() {
        // Regression: without preserve_order, serde_json used BTreeMap and sorted keys
        // alphabetically, putting end_line before kind/name. Line numbers must come last
        // as positional metadata, with identity fields (name, kind) first.
        let sym = make_test_sym("my_fn", Some("fn my_fn() -> u32"));
        let result = symbol_to_json(&sym, false, None, 0, false);

        let keys: Vec<&str> = result
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();

        // name and name_path come before start_line / end_line
        let name_pos = keys.iter().position(|k| *k == "name").unwrap();
        let start_pos = keys.iter().position(|k| *k == "start_line").unwrap();
        let end_pos = keys.iter().position(|k| *k == "end_line").unwrap();
        assert!(
            name_pos < start_pos,
            "name must appear before start_line, got key order: {keys:?}"
        );
        // start_line comes immediately before end_line
        assert_eq!(
            start_pos + 1,
            end_pos,
            "start_line must immediately precede end_line, got key order: {keys:?}"
        );
        // end_line is the final field
        assert_eq!(
            end_pos,
            keys.len() - 1,
            "end_line must be the last field, got key order: {keys:?}"
        );
    }

    #[test]
    fn symbol_to_json_includes_file_when_show_file_true() {
        let sym = make_test_sym("foo", None);
        let result = symbol_to_json(&sym, false, None, 0, true);
        assert_eq!(result["file"], "src/foo.rs");
    }

    #[test]
    fn symbol_to_json_includes_signature_when_detail_present() {
        let sym = make_test_sym("foo", Some("(x: i32) -> bool"));
        let result = symbol_to_json(&sym, false, None, 0, false);
        assert_eq!(result["signature"], "(x: i32) -> bool");
    }

    #[test]
    fn symbol_to_json_omits_signature_when_detail_absent() {
        let sym = make_test_sym("foo", None);
        let result = symbol_to_json(&sym, false, None, 0, false);
        assert!(
            result.get("signature").is_none(),
            "signature must be absent when detail=None"
        );
    }

    #[test]
    fn symbol_to_json_never_includes_source_field() {
        let sym = make_test_sym("foo", None);
        for show_file in [false, true] {
            let result = symbol_to_json(&sym, false, None, 0, show_file);
            assert!(
                result.get("source").is_none(),
                "source field must never appear (show_file={show_file})"
            );
        }
    }

    #[test]
    fn list_symbols_flat_cap_triggers_on_symbol_with_many_children() {
        // 20 top-level symbols each with 10 children = 220 flat entries > FLAT_CAP(150).
        // Greedy take: each symbol costs 11 flat entries; 150/11 = 13 symbols fit.
        let symbols: Vec<Value> = (0..20)
            .map(|i| {
                let children: Vec<Value> = (0..10)
                    .map(|j| json!({ "name": format!("child_{i}_{j}") }))
                    .collect();
                json!({ "name": format!("sym{i}"), "children": children })
            })
            .collect();

        let flat = super::flat_symbol_count(&symbols);
        assert_eq!(flat, 220); // 20 * (1 + 10)

        // Greedy capping within FLAT_CAP=150
        let budget = super::LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP;
        let mut remaining = budget;
        let mut capped: Vec<Value> = Vec::new();
        for sym in symbols {
            let cost = 1 + sym["children"].as_array().map(|c| c.len()).unwrap_or(0);
            if cost <= remaining {
                remaining -= cost;
                capped.push(sym);
            } else {
                break;
            }
        }
        // Each symbol costs 11; 13 symbols = 143 flat entries ≤ 150; 14th would be 154.
        assert_eq!(capped.len(), 13);
    }

    #[test]
    fn list_symbols_flat_cap_not_triggered_for_leaf_heavy_symbols() {
        // 50 top-level leaf symbols (no children) = 50 flat entries — under FLAT_CAP.
        let symbols: Vec<Value> = (0..50)
            .map(|i| json!({ "name": format!("fn{i}") }))
            .collect();
        let flat = super::flat_symbol_count(&symbols);
        assert_eq!(flat, 50);
        assert!(flat <= super::LIST_SYMBOLS_SINGLE_FILE_FLAT_CAP);
    }

    #[test]
    fn list_symbols_single_file_cap_unit() {
        // Unit test: simulate the cap logic on a Vec<Value> of 150 symbol entries.
        use super::OutputGuard;
        let symbols: Vec<Value> = (0..150)
            .map(|i| json!({ "name": format!("sym{i}"), "start_line": i + 1 }))
            .collect();

        const SINGLE_FILE_CAP: usize = 100;
        let total = symbols.len();
        let hint = format!(
            "File has {total} symbols. Use depth=1 for top-level overview, \
             or find_symbol(name_path='ClassName/methodName', include_body=true) for a specific symbol."
        );
        let mut g = OutputGuard::default();
        g.max_results = SINGLE_FILE_CAP;
        let (kept, overflow) = g.cap_items(symbols, &hint);

        assert_eq!(kept.len(), 100);
        let ov = overflow.expect("overflow must be present");
        assert_eq!(ov.total, 150);
        assert_eq!(ov.shown, 100);
        assert!(ov.hint.contains("find_symbol"));
        assert!(ov.hint.contains("name_path"));
        assert!(
            ov.by_file.is_none(),
            "single-file overflow must not include by_file"
        );
    }

    #[test]
    fn list_symbols_single_file_no_overflow_under_cap_unit() {
        use super::OutputGuard;
        let symbols: Vec<Value> = (0..40)
            .map(|i| json!({ "name": format!("sym{i}") }))
            .collect();

        let mut g = OutputGuard::default();
        g.max_results = 100;
        let (kept, overflow) = g.cap_items(symbols, "hint");

        assert_eq!(kept.len(), 40);
        assert!(
            overflow.is_none(),
            "no overflow for 40 symbols under cap of 100"
        );
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

    // ── BUG-002: apply_text_edits uses UTF-16 offsets correctly ─────────────

    /// LSP character positions are UTF-16 code units.  A line like
    /// `// α foo` has `α` (U+03B1) at byte 3, but at UTF-16 offset 3.
    /// `foo` starts at byte 6 but UTF-16 offset 5.
    /// The old byte-index code would slice at byte 5, landing mid-codepoint
    /// and either panicking or producing garbled text.
    #[test]
    fn apply_text_edits_utf16_offset() {
        // Line 0: "// α: foo"
        //   byte offsets:  0='/', 1='/', 2=' ', 3..5='α'(2 bytes), 5=':', 6=' ', 7='f', 8='o', 9='o'
        //   UTF-16 offsets: 0='/', 1='/', 2=' ', 3='α'(1 unit),    4=':', 5=' ', 6='f', 7='o', 8='o'
        // Replace "foo" (UTF-16 chars 6..9) with "bar"
        let content = "// \u{03B1}: foo\n";
        let edits = vec![lsp_types::TextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 6,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 9,
                },
            },
            new_text: "bar".to_string(),
        }];
        let result = apply_text_edits(content, &edits);
        assert_eq!(result, "// \u{03B1}: bar\n");
    }

    /// Surrogate pair: emoji (U+1F600) is 4 UTF-8 bytes but 2 UTF-16 code units.
    /// Text after the emoji has a higher UTF-16 offset than byte offset.
    #[test]
    fn apply_text_edits_utf16_surrogate_pair() {
        // Line: "😀 foo"
        //   bytes: 0..3=😀(4 bytes), 4=' ', 5='f', 6='o', 7='o'
        //   UTF-16: 0..1=😀(2 units), 2=' ', 3='f', 4='o', 5='o'
        // Replace "foo" (UTF-16 3..6) with "bar"
        let content = "\u{1F600} foo\n";
        let edits = vec![lsp_types::TextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 3,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 6,
                },
            },
            new_text: "bar".to_string(),
        }];
        let result = apply_text_edits(content, &edits);
        assert_eq!(result, "\u{1F600} bar\n");
    }

    // ── find_insert_before_line tests ──────────────────────────────────────

    #[test]
    fn find_insert_before_line_walks_past_doc_comments() {
        // Blank line between code and docs is NOT consumed (stops at blank line)
        let lines = vec![
            "other code",
            "",
            "/// Doc line 1",
            "/// Doc line 2",
            "fn foo() {}",
        ];
        assert_eq!(find_insert_before_line(&lines, 4), 2);
    }

    #[test]
    fn find_insert_before_line_walks_past_attributes() {
        let lines = vec!["other code", "#[test]", "#[ignore]", "fn foo() {}"];
        assert_eq!(find_insert_before_line(&lines, 3), 1);
    }

    #[test]
    fn find_insert_before_line_stops_at_code() {
        let lines = vec!["let x = 1;", "fn foo() {}"];
        assert_eq!(find_insert_before_line(&lines, 1), 1);
    }

    #[test]
    fn find_insert_before_line_at_start_of_file() {
        let lines = vec!["/// Doc", "fn foo() {}"];
        assert_eq!(find_insert_before_line(&lines, 1), 0);
    }

    #[test]
    fn editing_start_line_uses_range_start_line_when_present() {
        let sym = crate::lsp::SymbolInfo {
            name: "foo".to_string(),
            name_path: "foo".to_string(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("test.rs"),
            start_line: 8,
            end_line: 12,
            start_col: 0,
            children: vec![],
            range_start_line: Some(5),
            detail: None,
        };
        let lines = vec![
            "other code",
            "",
            "/// doc1",
            "/// doc2",
            "#[test]",
            "#[ignore]", // line 5 — range_start_line
            "// between",
            "// gap",
            "fn foo() {", // line 8 — start_line (selectionRange)
            "    body",
            "}",
        ];
        // Should use range_start_line (5), NOT heuristic or start_line
        assert_eq!(editing_start_line(&sym, &lines), 5);
    }

    #[test]
    fn editing_start_line_falls_back_to_heuristic_when_none() {
        let sym = crate::lsp::SymbolInfo {
            name: "foo".to_string(),
            name_path: "foo".to_string(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("test.rs"),
            start_line: 3,
            end_line: 5,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        };
        let lines = vec![
            "other code",
            "#[test]",
            "#[ignore]",
            "fn foo() {", // line 3
            "    body",
            "}",
        ];
        // No range_start_line → heuristic walks back past #[test] #[ignore]
        assert_eq!(editing_start_line(&sym, &lines), 1);
    }

    // ── symbol_to_json body extraction: full-range (includes attributes) ─────

    #[test]
    fn symbol_to_json_body_includes_attributes_when_range_start_line_set() {
        let source = "#[test]\n/// A doc comment\nfn foo() {\n    body();\n}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 2, // fn keyword (0-indexed)
            end_line: 4,   // closing }
            start_col: 0,
            children: vec![],
            range_start_line: Some(0), // #[test] line
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        let body = json["body"].as_str().unwrap();
        assert!(
            body.contains("#[test]"),
            "body should include #[test] attribute; got:\n{body}"
        );
        assert!(
            body.contains("/// A doc comment"),
            "body should include doc comment; got:\n{body}"
        );
        assert!(
            body.contains("fn foo()"),
            "body should include fn declaration; got:\n{body}"
        );
    }

    #[test]
    fn symbol_to_json_includes_body_start_line() {
        let source = "#[test]\nfn foo() {}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            children: vec![],
            range_start_line: Some(0),
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        // body_start_line should be 1 (1-indexed, the #[test] line)
        assert_eq!(
            json["body_start_line"].as_u64(),
            Some(1),
            "body_start_line should be 1-indexed line where body begins (the attribute line)"
        );
    }

    #[test]
    fn symbol_to_json_body_uses_heuristic_when_range_start_line_none() {
        let source = "#[test]\nfn foo() {\n    body();\n}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 1, // fn keyword
            end_line: 3,
            start_col: 0,
            children: vec![],
            range_start_line: None, // tree-sitter / workspace/symbol path
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        let body = json["body"].as_str().unwrap();
        assert!(
            body.contains("#[test]"),
            "body should include #[test] via heuristic fallback; got:\n{body}"
        );
    }

    #[test]
    fn symbol_to_json_body_start_line_equals_start_line_when_no_attributes() {
        let source = "fn foo() {\n    body();\n}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 0,
            end_line: 2,
            start_col: 0,
            children: vec![],
            range_start_line: Some(0), // same as start_line — no attributes
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        assert_eq!(
            json["body_start_line"].as_u64(),
            Some(1),
            "body_start_line should equal start_line when no attributes"
        );
        assert_eq!(
            json["start_line"].as_u64(),
            Some(1),
            "start_line should be 1 (1-indexed)"
        );
    }

    #[test]
    fn symbol_to_json_no_body_start_line_when_include_body_false() {
        let source = "#[test]\nfn foo() {}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            children: vec![],
            range_start_line: Some(0),
            detail: None,
        };
        let json = symbol_to_json(&sym, false, Some(source), 0, false);
        assert!(
            json.get("body").is_none(),
            "body should not be present when include_body=false"
        );
        assert!(
            json.get("body_start_line").is_none(),
            "body_start_line should not be present when include_body=false"
        );
    }

    #[test]
    fn symbol_to_json_body_includes_only_doc_comments() {
        // Symbol with only doc comments (no attributes)
        let source = "/// Doc line 1\n/// Doc line 2\nfn foo() {}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 2, // fn keyword
            end_line: 2,
            start_col: 0,
            children: vec![],
            range_start_line: Some(0), // includes doc comments
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        let body = json["body"].as_str().unwrap();
        assert!(
            body.contains("/// Doc line 1"),
            "body should include first doc line; got:\n{body}"
        );
        assert!(
            body.contains("/// Doc line 2"),
            "body should include second doc line; got:\n{body}"
        );
        assert!(
            body.contains("fn foo()"),
            "body should include fn declaration; got:\n{body}"
        );
        assert_eq!(json["body_start_line"].as_u64(), Some(1));
        assert_eq!(json["start_line"].as_u64(), Some(3)); // fn keyword is line 3 (1-indexed)
    }

    #[test]
    fn symbol_to_json_body_includes_multiline_attribute() {
        let source = "#[cfg(\n    target_os = \"linux\"\n)]\nfn foo() {}\n";
        let sym = crate::lsp::SymbolInfo {
            name: "foo".into(),
            name_path: "foo".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 3, // fn keyword
            end_line: 3,
            start_col: 0,
            children: vec![],
            range_start_line: Some(0), // includes #[cfg(
            detail: None,
        };
        let json = symbol_to_json(&sym, true, Some(source), 0, false);
        let body = json["body"].as_str().unwrap();
        assert!(
            body.contains("#[cfg("),
            "body should include multiline attribute opener; got:\n{body}"
        );
        assert!(
            body.contains("target_os"),
            "body should include attribute content; got:\n{body}"
        );
        assert!(
            body.contains(")]"),
            "body should include attribute closer; got:\n{body}"
        );
        assert_eq!(json["body_start_line"].as_u64(), Some(1));
    }

    #[test]
    fn symbol_to_json_child_body_also_uses_full_range() {
        // Parent with a child that has its own attributes
        let source = "impl Foo {\n    #[test]\n    fn bar() {}\n}\n";
        let child = crate::lsp::SymbolInfo {
            name: "bar".into(),
            name_path: "Foo/bar".into(),
            kind: crate::lsp::SymbolKind::Function,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 2, // fn bar
            end_line: 2,
            start_col: 0,
            children: vec![],
            range_start_line: Some(1), // #[test]
            detail: None,
        };
        let parent = crate::lsp::SymbolInfo {
            name: "Foo".into(),
            name_path: "Foo".into(),
            kind: crate::lsp::SymbolKind::Struct,
            file: std::path::PathBuf::from("src/lib.rs"),
            start_line: 0,
            end_line: 3,
            start_col: 0,
            children: vec![child],
            range_start_line: Some(0),
            detail: None,
        };
        // depth=1 to include children
        let json = symbol_to_json(&parent, true, Some(source), 1, false);
        let child_body = json["children"][0]["body"].as_str().unwrap();
        assert!(
            child_body.contains("#[test]"),
            "child body should include its attribute; got:\n{child_body}"
        );
        assert!(
            child_body.contains("fn bar()"),
            "child body should include fn declaration; got:\n{child_body}"
        );
    }

    #[test]
    fn find_insert_before_line_walks_past_multiline_attribute() {
        // #[cfg(
        //     target_os = "linux"
        // )]
        // fn foo() {}
        let lines = vec![
            "other code",
            "#[cfg(",
            "    target_os = \"linux\"",
            ")]",
            "fn foo() {}",
        ];
        assert_eq!(find_insert_before_line(&lines, 4), 1);
    }

    #[test]
    fn find_insert_before_line_walks_past_nested_multiline_attributes() {
        // #[cfg(all(
        //     target_os = "linux",
        //     feature = "nightly"
        // ))]
        // #[inline]
        // fn foo() {}
        let lines = vec![
            "other code",
            "#[cfg(all(",
            "    target_os = \"linux\",",
            "    feature = \"nightly\"",
            "))]",
            "#[inline]",
            "fn foo() {}",
        ];
        assert_eq!(find_insert_before_line(&lines, 6), 1);
    }

    #[test]
    fn find_insert_before_line_walks_past_python_multiline_decorator() {
        // @app.route(
        //     "/api/v1/users",
        //     methods=["GET"]
        // )
        // def get_users():
        let lines = vec![
            "other code",
            "@app.route(",
            "    \"/api/v1/users\",",
            "    methods=[\"GET\"]",
            ")",
            "def get_users():",
        ];
        // The `)` on line 4 is recognized as a bracket closer, triggering
        // upward scanning through the multi-line decorator.
        assert_eq!(find_insert_before_line(&lines, 5), 1);
    }

    #[test]
    fn find_references_format_compact_shows_count() {
        use serde_json::json;
        let tool = FindReferences;
        let result = json!({ "references": [{"file":"a.rs","line":10}], "total": 1 });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("1 ref"), "got: {text}");
    }

    #[test]
    fn rename_symbol_format_compact_shows_sites() {
        use serde_json::json;
        let tool = RenameSymbol;
        let result = json!({ "total_edits": 5, "textual_match_count": 1, "files_changed": 2, "new_name": "bar" });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("bar"), "got: {text}");
    }

    #[test]
    fn insert_code_format_compact_shows_location() {
        use serde_json::json;
        let tool = InsertCode;
        let result = json!({ "status": "ok", "inserted_at_line": 42, "position": "after" });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("42"), "got: {text}");
    }

    #[test]
    fn replace_symbol_format_compact_shows_range() {
        let tool = ReplaceSymbol;
        let r = json!({ "status": "ok", "replaced_lines": "124-145" });
        let t = tool.format_compact(&r).unwrap();
        assert!(t.contains("L124"), "got: {t}");
    }

    #[test]
    fn remove_symbol_format_compact_shows_range() {
        let tool = RemoveSymbol;
        let r = json!({ "status": "ok", "removed_lines": "201-215", "line_count": 14 });
        let t = tool.format_compact(&r).unwrap();
        assert!(t.contains("201"), "got: {t}");
        assert!(t.contains("14"), "got: {t}");
    }

    // --- format_goto_definition tests ---

    #[test]
    fn goto_single_project_definition() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "src/tools/output.rs",
                "line": 35,
                "end_line": 41,
                "context": "pub struct OutputGuard {",
                "source": "project"
            }],
            "from": "symbol.rs:120"
        });
        let result = format_goto_definition(&val);
        assert_eq!(
            result,
            "src/tools/output.rs:35\n\n  pub struct OutputGuard {"
        );
    }

    #[test]
    fn goto_single_external_definition() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "/home/user/.rustup/toolchains/stable/lib.rs",
                "line": 100,
                "end_line": 110,
                "context": "pub enum Option<T> {",
                "source": "external"
            }],
            "from": "main.rs:5"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("(external)"));
        assert!(result.contains(":100"));
        assert!(result.contains("pub enum Option<T> {"));
    }

    #[test]
    fn goto_multiple_definitions() {
        let val = serde_json::json!({
            "definitions": [
                { "file": "src/a.rs", "line": 10, "end_line": 15, "context": "fn foo()", "source": "project" },
                { "file": "src/b.rs", "line": 20, "end_line": 25, "context": "fn foo()", "source": "project" }
            ],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert!(result.starts_with("2 definitions"));
        assert!(result.contains("src/a.rs:10"));
        assert!(result.contains("src/b.rs:20"));
    }

    #[test]
    fn goto_empty_definitions() {
        let val = serde_json::json!({ "definitions": [] });
        assert_eq!(format_goto_definition(&val), "");
    }

    #[test]
    fn goto_no_context() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "src/lib.rs",
                "line": 1,
                "end_line": 1,
                "context": "",
                "source": "project"
            }],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert_eq!(result, "src/lib.rs:1");
    }

    #[test]
    fn goto_multiple_with_external() {
        let val = serde_json::json!({
            "definitions": [
                { "file": "src/a.rs", "line": 10, "end_line": 10, "context": "fn foo()", "source": "project" },
                { "file": "/ext/lib.rs", "line": 20, "end_line": 20, "context": "fn foo()", "source": "lib:serde" }
            ],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("2 definitions"));
        assert!(result.contains("src/a.rs:10"));
        assert!(result.contains("(lib:serde)"));
    }

    // --- format_hover tests ---

    #[test]
    fn hover_with_code_fence() {
        let val = serde_json::json!({
            "content": "```rust\npub struct OutputGuard {\n    mode: OutputMode,\n}\n```\n\nProgressive disclosure guard.",
            "location": "output.rs:35"
        });
        let result = format_hover(&val);
        assert!(result.starts_with("output.rs:35"));
        assert!(result.contains("  pub struct OutputGuard {"));
        assert!(result.contains("  Progressive disclosure guard."));
        assert!(!result.contains("```"));
    }

    #[test]
    fn hover_plain_text_no_fences() {
        let val = serde_json::json!({
            "content": "Some plain documentation.",
            "location": "lib.rs:10"
        });
        let result = format_hover(&val);
        assert_eq!(result, "lib.rs:10\n\n  Some plain documentation.");
    }

    #[test]
    fn hover_no_location() {
        let val = serde_json::json!({
            "content": "```rust\nfn main() {}\n```"
        });
        let result = format_hover(&val);
        assert!(!result.contains("```"));
        assert!(result.contains("  fn main() {}"));
    }

    #[test]
    fn hover_empty_content() {
        let val = serde_json::json!({});
        assert_eq!(format_hover(&val), "");
    }

    #[test]
    fn hover_multiline_doc() {
        let val = serde_json::json!({
            "content": "```rust\nfn add(a: i32, b: i32) -> i32\n```\n\nAdds two numbers.\n\nReturns the sum.",
            "location": "math.rs:5"
        });
        let result = format_hover(&val);
        assert!(result.contains("  fn add(a: i32, b: i32) -> i32"));
        assert!(result.contains("  Adds two numbers."));
        assert!(result.contains("  Returns the sum."));
        assert!(!result.contains("```"));
    }

    // --- format_find_symbol tests ---

    #[test]
    fn find_symbol_no_body() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "OutputGuard", "name_path": "OutputGuard",
                    "kind": "Struct", "file": "src/tools/output.rs",
                    "start_line": 35, "end_line": 50
                },
                {
                    "name": "cap_items", "name_path": "OutputGuard/cap_items",
                    "kind": "Function", "file": "src/tools/output.rs",
                    "start_line": 55, "end_line": 80
                }
            ],
            "total": 2
        });
        let result = format_find_symbol(&val);
        assert!(result.starts_with("2 matches\n"));
        assert!(result.contains("Struct"));
        assert!(result.contains("Function"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("OutputGuard/cap_items"));
        assert!(result.contains("src/tools/output.rs:35-50"));
        assert!(result.contains("src/tools/output.rs:55-80"));
    }

    #[test]
    fn find_symbol_with_body() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "cap_items", "name_path": "OutputGuard/cap_items",
                    "kind": "Function", "file": "src/tools/output.rs",
                    "start_line": 55, "end_line": 80,
                    "body": "pub fn cap_items(&self) -> Option<OverflowInfo> {\n    // impl\n}"
                }
            ],
            "total": 1
        });
        let result = format_find_symbol(&val);
        assert!(result.starts_with("1 match\n"));
        assert!(result.contains("Function"));
        assert!(result.contains("OutputGuard/cap_items"));
        assert!(result.contains("      pub fn cap_items(&self) -> Option<OverflowInfo> {"));
        assert!(result.contains("      // impl"));
        assert!(result.contains("      }"));
    }

    #[test]
    fn find_symbol_with_long_body_shows_hint_not_truncated_body() {
        // A body > 500 chars should not be inlined — it would get truncated by
        // COMPACT_SUMMARY_MAX_BYTES mid-function, misleading agents into thinking
        // the body is incomplete. Instead, show a navigation hint.
        let long_body = "fun convert() {\n".to_string() + &"    val x = 1\n".repeat(50) + "}";
        assert!(
            long_body.len() > 500,
            "test body should exceed INLINE_BODY_LIMIT"
        );
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "convert", "name_path": "Stage1ToStage2Converter/convert",
                    "kind": "Method", "file": "src/Converter.kt",
                    "start_line": 160, "end_line": 490,
                    "body": long_body
                }
            ],
            "total": 1
        });
        let result = format_find_symbol(&val);
        // Must mention the line count and the extraction path
        assert!(
            result.contains("52-line body"),
            "expected line count in hint, got: {result}"
        );
        assert!(
            result.contains("$.symbols[0].body"),
            "expected json_path hint, got: {result}"
        );
        // Must NOT inline the body content
        assert!(
            !result.contains("val x = 1"),
            "body content must not appear inline"
        );
    }

    #[test]
    fn find_symbol_with_overflow() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "foo", "name_path": "foo",
                    "kind": "Function", "file": "src/a.rs",
                    "start_line": 10, "end_line": 10
                }
            ],
            "total": 100,
            "overflow": {
                "shown": 20, "total": 100,
                "hint": "narrow with path=",
                "by_file": [["src/a.rs", 50], ["src/b.rs", 30]]
            }
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("20 matches (100 total)"));
        assert!(result.contains("20 of 100"));
        assert!(result.contains("narrow with path="));
    }

    #[test]
    fn find_symbol_empty() {
        let val = serde_json::json!({
            "symbols": [],
            "total": 0
        });
        assert_eq!(format_find_symbol(&val), "0 matches");
    }

    #[test]
    fn find_symbol_missing_symbols_key() {
        let val = serde_json::json!({});
        assert_eq!(format_find_symbol(&val), "");
    }

    #[test]
    fn find_symbol_alignment() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "Foo", "name_path": "Foo",
                    "kind": "Struct", "file": "src/a.rs",
                    "start_line": 1, "end_line": 5
                },
                {
                    "name": "bar_baz", "name_path": "bar_baz",
                    "kind": "Function", "file": "src/very/long/path.rs",
                    "start_line": 100, "end_line": 200
                }
            ],
            "total": 2
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("Struct  "));
        assert!(result.contains("Function"));
        assert!(result.contains("src/a.rs:1-5"));
        assert!(result.contains("src/very/long/path.rs:100-200"));
    }

    #[test]
    fn find_symbol_single_line_location() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "X", "name_path": "X",
                    "kind": "Constant", "file": "src/lib.rs",
                    "start_line": 42, "end_line": 42
                }
            ],
            "total": 1
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("src/lib.rs:42"));
        assert!(!result.contains("42-42"));
    }

    // --- format_list_symbols tests ---

    #[test]
    fn list_symbols_file_mode() {
        let val = serde_json::json!({
            "file": "src/tools/output.rs",
            "symbols": [
                {
                    "name": "OutputMode", "name_path": "OutputMode",
                    "kind": "Enum", "start_line": 10, "end_line": 15,
                    "children": [
                        { "name": "Exploring", "kind": "EnumMember", "start_line": 11, "end_line": 11 },
                        { "name": "Focused", "kind": "EnumMember", "start_line": 12, "end_line": 12 }
                    ]
                },
                {
                    "name": "OutputGuard", "name_path": "OutputGuard",
                    "kind": "Struct", "start_line": 35, "end_line": 50
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.starts_with("src/tools/output.rs — 2 symbols\n"));
        assert!(result.contains("Enum"));
        assert!(result.contains("OutputMode"));
        assert!(result.contains("L10-15"));
        assert!(result.contains("Exploring"));
        assert!(result.contains("L11"));
        assert!(result.contains("Focused"));
        assert!(result.contains("L12"));
        assert!(result.contains("Struct"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("L35-50"));
        assert!(!result.contains("EnumMember"));
    }

    #[test]
    fn list_symbols_directory_mode() {
        let val = serde_json::json!({
            "directory": "src/tools",
            "files": [
                {
                    "file": "src/tools/ast.rs",
                    "symbols": [
                        { "name": "ListFunctions", "name_path": "ListFunctions", "kind": "Struct", "start_line": 10, "end_line": 20 }
                    ]
                },
                {
                    "file": "src/tools/config.rs",
                    "symbols": [
                        { "name": "GetConfig", "name_path": "GetConfig", "kind": "Struct", "start_line": 5, "end_line": 15 },
                        { "name": "ActivateProject", "name_path": "ActivateProject", "kind": "Struct", "start_line": 20, "end_line": 30 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.starts_with("src/tools\n"));
        assert!(result.contains("src/tools/ast.rs — 1 symbol\n"));
        assert!(result.contains("src/tools/config.rs — 2 symbols\n"));
        assert!(result.contains("ListFunctions"));
        assert!(result.contains("GetConfig"));
        assert!(result.contains("ActivateProject"));
    }

    #[test]
    fn list_symbols_pattern_mode() {
        let val = serde_json::json!({
            "pattern": "src/**/*.rs",
            "files": [
                {
                    "file": "src/main.rs",
                    "symbols": [
                        { "name": "main", "name_path": "main", "kind": "Function", "start_line": 1, "end_line": 10 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.starts_with("src/**/*.rs\n"));
        assert!(result.contains("src/main.rs — 1 symbol\n"));
        assert!(result.contains("main"));
    }

    #[test]
    fn list_symbols_empty_file() {
        let val = serde_json::json!({
            "file": "src/empty.rs",
            "symbols": []
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("0 symbols"));
    }

    #[test]
    fn list_symbols_empty_directory() {
        let val = serde_json::json!({
            "directory": "src/empty",
            "files": []
        });
        let result = format_list_symbols(&val);
        assert_eq!(result, "src/empty — 0 symbols");
    }

    #[test]
    fn list_symbols_with_overflow() {
        let val = serde_json::json!({
            "directory": "src",
            "files": [
                {
                    "file": "src/a.rs",
                    "symbols": [
                        { "name": "Foo", "name_path": "Foo", "kind": "Struct", "start_line": 1, "end_line": 5 }
                    ]
                }
            ],
            "overflow": { "shown": 10, "total": 50, "hint": "Narrow with a more specific glob or file path" }
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("10 of 50"));
        assert!(result.contains("Narrow with a more specific glob"));
    }

    #[test]
    fn list_symbols_children_with_fields() {
        let val = serde_json::json!({
            "file": "src/model.rs",
            "symbols": [
                {
                    "name": "Config", "name_path": "Config",
                    "kind": "Struct", "start_line": 1, "end_line": 10,
                    "children": [
                        { "name": "port", "kind": "Field", "start_line": 2, "end_line": 2 },
                        { "name": "host", "kind": "Field", "start_line": 3, "end_line": 3 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(!result.contains("Field"));
        assert!(result.contains("port"));
        assert!(result.contains("host"));
        assert!(result.contains("L2"));
        assert!(result.contains("L3"));
    }

    #[test]
    fn list_symbols_children_with_methods() {
        let val = serde_json::json!({
            "file": "src/service.rs",
            "symbols": [
                {
                    "name": "Server", "name_path": "Server",
                    "kind": "Struct", "start_line": 1, "end_line": 50,
                    "children": [
                        { "name": "new", "kind": "Function", "start_line": 5, "end_line": 10 },
                        { "name": "run", "kind": "Function", "start_line": 12, "end_line": 40 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("Function  new"));
        assert!(result.contains("Function  run"));
    }

    #[test]
    fn list_symbols_missing_symbols_key() {
        let val = serde_json::json!({});
        assert_eq!(format_list_symbols(&val), "");
    }

    #[test]
    fn list_symbols_singular_symbol_word() {
        let val = serde_json::json!({
            "file": "src/single.rs",
            "symbols": [
                { "name": "main", "name_path": "main", "kind": "Function", "start_line": 1, "end_line": 5 }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("1 symbol\n"));
        assert!(!result.contains("1 symbols"));
    }

    // --- format_find_references tests ---

    #[test]
    fn find_references_basic() {
        let result = serde_json::json!({
            "references": [
                {"file": "src/foo.rs", "line": 10, "kind": "usage"},
                {"file": "src/bar.rs", "line": 20, "kind": "usage"},
                {"file": "src/foo.rs", "line": 30, "kind": "usage"}
            ],
            "total": 3
        });
        let text = format_find_references(&result);
        assert!(text.contains("3"), "should mention count");
        assert!(
            text.contains("refs") || text.contains("reference"),
            "should say refs or reference(s)"
        );
    }

    #[test]
    fn find_references_empty() {
        let result = serde_json::json!({ "references": [], "total": 0 });
        let text = format_find_references(&result);
        assert!(
            text.contains("No"),
            "should say 'No references found.', got: {}",
            text
        );
    }

    #[test]
    fn format_find_references_shows_locations() {
        let result = serde_json::json!({
            "total": 8,
            "references": [
                {"file": "src/tools/symbol.rs", "line": 142},
                {"file": "src/tools/symbol.rs", "line": 198},
                {"file": "src/server.rs", "line": 87},
                {"file": "src/agent.rs", "line": 210},
                {"file": "src/main.rs", "line": 45},
                {"file": "src/config.rs", "line": 12}
            ]
        });
        let out = format_find_references(&result);
        assert!(out.contains("8 refs"), "should show total");
        assert!(
            out.contains("src/tools/symbol.rs:142"),
            "should show locations"
        );
        assert!(out.contains("src/server.rs:87"), "should show locations");
        assert!(out.contains("more"), "should show trailer for hidden refs");
        assert!(!out.contains("src/config.rs"), "should cap at 5");
    }

    #[test]
    fn format_find_references_five_or_fewer_no_trailer() {
        let result = serde_json::json!({
            "total": 3,
            "references": [
                {"file": "src/a.rs", "line": 1},
                {"file": "src/b.rs", "line": 2},
                {"file": "src/c.rs", "line": 3}
            ]
        });
        let out = format_find_references(&result);
        assert!(out.contains("src/a.rs:1"));
        assert!(!out.contains("more"), "no trailer when all fit");
    }

    #[tokio::test]
    async fn find_symbol_falls_back_to_document_symbols_on_bad_workspace_range() {
        use crate::lsp::{mock::MockLspClient, mock::MockLspProvider, SymbolInfo, SymbolKind};

        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let file = src_dir.join("lib.rs");
        std::fs::write(
            &file,
            "fn helper(x: i32) -> i32 {\n    let y = x + 1;\n    y * 2\n}\n",
        )
        .unwrap();

        // workspace/symbol returns degenerate range (start == end)
        let degenerate = SymbolInfo {
            name: "helper".to_string(),
            name_path: "helper".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0,
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        // document_symbols returns correct range
        let correct = SymbolInfo {
            name: "helper".to_string(),
            name_path: "helper".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 3,
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        let mock = MockLspClient::new()
            .with_workspace_symbols(vec![degenerate])
            .with_symbols(&file, vec![correct]);
        let lsp = MockLspProvider::with_client(mock);

        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp,
            output_buffer: buf(),
            progress: None,
        };

        let result = FindSymbol
            .call(
                json!({
                    "pattern": "helper",
                    "include_body": true,
                }),
                &ctx,
            )
            .await;

        let val = result.expect("find_symbol should recover via document_symbols fallback");
        let symbols = val["symbols"].as_array().expect("symbols array");
        assert_eq!(symbols.len(), 1, "should find exactly one symbol");

        let body = symbols[0]["body"].as_str().expect("body should be present");
        assert!(
            body.contains("let y = x + 1"),
            "body should contain function contents; got: {body}"
        );
    }

    #[test]
    fn find_matching_symbol_finds_top_level() {
        use crate::lsp::SymbolKind;
        let symbols = vec![SymbolInfo {
            name: "foo".to_string(),
            name_path: "foo".to_string(),
            kind: SymbolKind::Function,
            file: PathBuf::from("lib.rs"),
            start_line: 10,
            end_line: 20,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        }];
        let result = find_matching_symbol(&symbols, "foo", 10);
        assert!(result.is_some());
        assert_eq!(result.unwrap().end_line, 20);
    }

    #[test]
    fn find_matching_symbol_finds_nested_child() {
        use crate::lsp::SymbolKind;
        let child = SymbolInfo {
            name: "bar".to_string(),
            name_path: "Foo/bar".to_string(),
            kind: SymbolKind::Function,
            file: PathBuf::from("lib.rs"),
            start_line: 15,
            end_line: 18,
            start_col: 4,
            children: vec![],
            range_start_line: None,
            detail: None,
        };
        let parent = SymbolInfo {
            name: "Foo".to_string(),
            name_path: "Foo".to_string(),
            kind: SymbolKind::Struct,
            file: PathBuf::from("lib.rs"),
            start_line: 10,
            end_line: 20,
            start_col: 0,
            children: vec![child],
            range_start_line: None,
            detail: None,
        };
        let result = find_matching_symbol(&[parent], "bar", 15);
        assert!(result.is_some());
        assert_eq!(result.unwrap().end_line, 18);
    }

    #[test]
    fn find_matching_symbol_returns_none_on_name_mismatch() {
        use crate::lsp::SymbolKind;
        let symbols = vec![SymbolInfo {
            name: "foo".to_string(),
            name_path: "foo".to_string(),
            kind: SymbolKind::Function,
            file: PathBuf::from("lib.rs"),
            start_line: 10,
            end_line: 20,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        }];
        let result = find_matching_symbol(&symbols, "bar", 10);
        assert!(result.is_none());
    }

    #[test]
    fn find_matching_symbol_returns_none_when_line_too_far() {
        use crate::lsp::SymbolKind;
        let symbols = vec![SymbolInfo {
            name: "foo".to_string(),
            name_path: "foo".to_string(),
            kind: SymbolKind::Function,
            file: PathBuf::from("lib.rs"),
            start_line: 10,
            end_line: 20,
            start_col: 0,
            children: vec![],
            range_start_line: None,
            detail: None,
        }];
        // lsp_start=13 → abs_diff(10, 13) = 3 > 1 → no match
        let result = find_matching_symbol(&symbols, "foo", 13);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_symbol_propagates_error_when_fallback_also_fails() {
        use crate::lsp::{mock::MockLspClient, mock::MockLspProvider, SymbolInfo, SymbolKind};

        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let file = src_dir.join("lib.rs");
        std::fs::write(
            &file,
            "fn helper(x: i32) -> i32 {\n    let y = x + 1;\n    y * 2\n}\n",
        )
        .unwrap();

        // workspace/symbol returns degenerate range
        let degenerate = SymbolInfo {
            name: "helper".to_string(),
            name_path: "helper".to_string(),
            kind: SymbolKind::Function,
            file: file.clone(),
            start_line: 0,
            end_line: 0,
            start_col: 3,
            children: vec![],
            range_start_line: None,
            detail: None,
        };

        // document_symbols returns NOTHING — fallback will fail
        let mock = MockLspClient::new().with_workspace_symbols(vec![degenerate]);
        // Note: NOT calling .with_symbols() — document_symbols will return empty vec
        let lsp = MockLspProvider::with_client(mock);

        // Use the same ToolContext setup pattern as the other test
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp,
            output_buffer: buf(),
            progress: None,
        };

        let result = FindSymbol
            .call(
                json!({
                    "pattern": "helper",
                    "include_body": true,
                }),
                &ctx,
            )
            .await;

        // Should fail with the original RecoverableError
        let err = result.expect_err("should propagate error when fallback fails");
        let msg = err.to_string();
        assert!(
            msg.contains("suspicious range"),
            "error should mention suspicious range; got: {msg}"
        );
    }

    // ── resolve_library_roots ────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_library_roots_empty_when_no_libraries() {
        let dir = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let roots = resolve_library_roots(&crate::library::scope::Scope::Libraries, &agent).await;
        assert!(roots.is_empty());
    }

    #[tokio::test]
    async fn resolve_library_roots_returns_registered_libraries() {
        let dir = tempdir().unwrap();
        let lib_dir = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "mylib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }
        let roots = resolve_library_roots(&crate::library::scope::Scope::Libraries, &agent).await;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].0, "mylib");
        assert_eq!(roots[0].1, lib_dir.path().to_path_buf());
    }

    #[tokio::test]
    async fn resolve_library_roots_filters_by_name() {
        let dir = tempdir().unwrap();
        let lib1 = tempdir().unwrap();
        let lib2 = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "alpha".to_string(),
                lib1.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
            project.library_registry.register(
                "beta".to_string(),
                lib2.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }
        let roots = resolve_library_roots(
            &crate::library::scope::Scope::Library("alpha".to_string()),
            &agent,
        )
        .await;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].0, "alpha");
    }

    #[tokio::test]
    async fn resolve_library_roots_project_scope_returns_empty() {
        let dir = tempdir().unwrap();
        let lib_dir = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "mylib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }
        let roots = resolve_library_roots(&crate::library::scope::Scope::Project, &agent).await;
        assert!(roots.is_empty());
    }

    #[tokio::test]
    async fn resolve_library_roots_all_scope_returns_all() {
        let dir = tempdir().unwrap();
        let lib1 = tempdir().unwrap();
        let lib2 = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "alpha".to_string(),
                lib1.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
            project.library_registry.register(
                "beta".to_string(),
                lib2.path().to_path_buf(),
                "python".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }
        let roots = resolve_library_roots(&crate::library::scope::Scope::All, &agent).await;
        assert_eq!(roots.len(), 2);
    }

    // ── format_library_path ──────────────────────────────────────────────────

    #[test]
    fn format_library_path_strips_root() {
        let lib_root = PathBuf::from("/home/user/.cargo/registry/src/serde-1.0");
        let file = PathBuf::from("/home/user/.cargo/registry/src/serde-1.0/src/lib.rs");
        let result = format_library_path("serde", &lib_root, &file);
        assert_eq!(result, "lib:serde/src/lib.rs");
    }

    #[test]
    fn format_library_path_fallback_for_outside_root() {
        let lib_root = PathBuf::from("/home/user/.cargo/registry/src/serde-1.0");
        let file = PathBuf::from("/somewhere/else/lib.rs");
        let result = format_library_path("serde", &lib_root, &file);
        assert_eq!(result, "/somewhere/else/lib.rs");
    }

    // ── classify_reference_path ──────────────────────────────────────────────

    #[test]
    fn classify_reference_path_project() {
        let root = PathBuf::from("/project");
        let libs = vec![("mylib".to_string(), PathBuf::from("/libs/mylib"))];
        let path = PathBuf::from("/project/src/main.rs");
        let (classification, display) = classify_reference_path(&path, &root, &libs);
        assert_eq!(classification, "project");
        assert_eq!(display, "src/main.rs");
    }

    #[test]
    fn classify_reference_path_library() {
        let root = PathBuf::from("/project");
        let libs = vec![("mylib".to_string(), PathBuf::from("/libs/mylib"))];
        let path = PathBuf::from("/libs/mylib/src/lib.rs");
        let (classification, display) = classify_reference_path(&path, &root, &libs);
        assert_eq!(classification, "lib:mylib");
        assert_eq!(display, "lib:mylib/src/lib.rs");
    }

    #[test]
    fn classify_reference_path_external() {
        let root = PathBuf::from("/project");
        let libs = vec![("mylib".to_string(), PathBuf::from("/libs/mylib"))];
        let path = PathBuf::from("/somewhere/else.rs");
        let (classification, display) = classify_reference_path(&path, &root, &libs);
        assert_eq!(classification, "external");
        assert_eq!(display, "/somewhere/else.rs");
    }

    fn test_ctx_with_agent(agent: Agent) -> ToolContext {
        ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: buf(),
            progress: None,
        }
    }

    #[tokio::test]
    async fn list_symbols_scope_libraries_includes_library_files() {
        let project_dir = tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join(".codescout")).unwrap();
        let lib_dir = tempdir().unwrap();
        let lib_src = lib_dir.path().join("src");
        std::fs::create_dir_all(&lib_src).unwrap();
        std::fs::write(lib_src.join("lib.rs"), "pub fn hello() {}\n").unwrap();

        let agent = Agent::new(Some(project_dir.path().to_path_buf()))
            .await
            .unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "testlib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }

        let ctx = test_ctx_with_agent(agent);
        let tool = ListSymbols;
        let result = tool
            .call(json!({"scope": "libraries"}), &ctx)
            .await
            .unwrap();

        let files = result["files"].as_array().unwrap();
        assert!(!files.is_empty(), "should find library files");
        let first_file = files[0]["file"].as_str().unwrap();
        assert!(
            first_file.starts_with("lib:testlib/"),
            "library file should have lib: prefix, got: {}",
            first_file
        );
    }

    #[tokio::test]
    async fn list_symbols_scope_project_excludes_libraries() {
        let project_dir = tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join(".codescout")).unwrap();
        let lib_dir = tempdir().unwrap();
        std::fs::create_dir_all(lib_dir.path().join("src")).unwrap();
        std::fs::write(lib_dir.path().join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
        std::fs::write(project_dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let agent = Agent::new(Some(project_dir.path().to_path_buf()))
            .await
            .unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "testlib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }

        let ctx = test_ctx_with_agent(agent);
        let tool = ListSymbols;
        let result = tool.call(json!({"scope": "project"}), &ctx).await.unwrap();

        let empty = vec![];
        let files = result["files"].as_array().unwrap_or(&empty);
        for f in files {
            let path = f["file"].as_str().unwrap();
            assert!(
                !path.starts_with("lib:"),
                "project scope should not include library files: {}",
                path
            );
        }
    }

    #[tokio::test]
    async fn find_symbol_scope_libraries_searches_library_dirs() {
        let project_dir = tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join(".codescout")).unwrap();
        let lib_dir = tempdir().unwrap();
        std::fs::create_dir_all(lib_dir.path().join("src")).unwrap();
        std::fs::write(
            lib_dir.path().join("src/lib.rs"),
            "pub fn library_unique_symbol_xyz() {}\n",
        )
        .unwrap();

        let agent = Agent::new(Some(project_dir.path().to_path_buf()))
            .await
            .unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "testlib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }

        let ctx = test_ctx_with_agent(agent);
        let tool = FindSymbol;
        let result = tool
            .call(
                json!({
                    "pattern": "library_unique_symbol_xyz",
                    "scope": "libraries"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        assert!(!symbols.is_empty(), "should find symbol in library");
        let file = symbols[0]["file"].as_str().unwrap();
        assert!(
            file.starts_with("lib:testlib/"),
            "file path should have lib: prefix: {}",
            file
        );
    }

    #[tokio::test]
    async fn find_symbol_scope_all_searches_both() {
        let project_dir = tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join(".codescout")).unwrap();
        let lib_dir = tempdir().unwrap();
        std::fs::write(project_dir.path().join("main.rs"), "fn project_func() {}\n").unwrap();
        std::fs::create_dir_all(lib_dir.path().join("src")).unwrap();
        std::fs::write(lib_dir.path().join("src/lib.rs"), "pub fn lib_func() {}\n").unwrap();

        let agent = Agent::new(Some(project_dir.path().to_path_buf()))
            .await
            .unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "testlib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }

        let ctx = test_ctx_with_agent(agent);
        let tool = FindSymbol;
        let result = tool
            .call(
                json!({
                    "pattern": "func",
                    "scope": "all"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        let files: Vec<&str> = symbols.iter().filter_map(|s| s["file"].as_str()).collect();
        assert!(
            files.iter().any(|f| f.starts_with("lib:testlib/")),
            "should include library symbol"
        );
        assert!(
            files.iter().any(|f| !f.starts_with("lib:")),
            "should include project symbol"
        );
    }

    #[tokio::test]
    async fn find_symbol_scope_project_default_excludes_libraries() {
        let project_dir = tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join(".codescout")).unwrap();
        let lib_dir = tempdir().unwrap();
        std::fs::write(project_dir.path().join("main.rs"), "fn my_func() {}\n").unwrap();
        std::fs::create_dir_all(lib_dir.path().join("src")).unwrap();
        std::fs::write(lib_dir.path().join("src/lib.rs"), "pub fn my_func() {}\n").unwrap();

        let agent = Agent::new(Some(project_dir.path().to_path_buf()))
            .await
            .unwrap();
        {
            let mut inner = agent.inner.write().await;
            let project = inner.active_project_mut().unwrap();
            project.library_registry.register(
                "testlib".to_string(),
                lib_dir.path().to_path_buf(),
                "rust".to_string(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }

        let ctx = test_ctx_with_agent(agent);
        let tool = FindSymbol;
        let result = tool
            .call(
                json!({
                    "pattern": "my_func",
                    "scope": "project"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let symbols = result["symbols"].as_array().unwrap();
        for s in symbols {
            let file = s["file"].as_str().unwrap();
            assert!(
                !file.starts_with("lib:"),
                "project scope should not include library: {}",
                file
            );
        }
    }
}
