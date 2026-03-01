//! AST-aware code chunker with language registry.
//!
//! Provides language-specific knowledge (node types, doc comment prefixes) used
//! to split source files into semantically meaningful chunks. Uses tree-sitter
//! grammars for registered languages to extract top-level declarations, falling
//! back to the plain text chunker for unknown languages.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::chunker::RawChunk;

/// Language-specific metadata for AST-aware chunking.
/// Language-specific metadata for AST-aware chunking.
pub struct LanguageSpec {
    /// Tree-sitter node types that represent top-level declarations.
    pub node_types: &'static [&'static str],
    /// Prefixes that introduce documentation comments.
    pub doc_prefixes: &'static [&'static str],
    /// Node types to recurse into when a container node is too large.
    ///
    /// When a node whose type is in `node_types` exceeds `chunk_size`, the
    /// chunker looks for children matching these types and emits one chunk per
    /// child instead of falling back to plain-text line splitting. This produces
    /// clean method-level chunks for `impl` blocks, classes, etc.
    ///
    /// Empty slice means no recursion (e.g. Go functions, Rust free functions).
    pub inner_node_types: &'static [&'static str],
}

/// Registry entry mapping a language name to its spec.
struct RegistryEntry {
    name: &'static str,
    spec: LanguageSpec,
}

static LANGUAGE_REGISTRY: &[RegistryEntry] = &[
    RegistryEntry {
        name: "rust",
        spec: LanguageSpec {
            node_types: &[
                "function_item",
                "struct_item",
                "enum_item",
                "trait_item",
                "impl_item",
                "mod_item",
                "type_item",
                "const_item",
                "static_item",
                "macro_definition",
            ],
            doc_prefixes: &["///", "//!"],
            // Recurse into impl/mod blocks to extract individual items.
            inner_node_types: &["function_item", "const_item", "type_item", "impl_item"],
        },
    },
    RegistryEntry {
        name: "python",
        spec: LanguageSpec {
            node_types: &[
                "function_definition",
                "class_definition",
                "decorated_definition",
                "async_function_definition",
            ],
            doc_prefixes: &["#"],
            // Recurse into class bodies to extract methods.
            inner_node_types: &[
                "function_definition",
                "decorated_definition",
                "async_function_definition",
            ],
        },
    },
    RegistryEntry {
        name: "go",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
                "var_declaration",
                "const_declaration",
            ],
            doc_prefixes: &["//"],
            // Go has no class/impl containers — all declarations are top-level.
            inner_node_types: &[],
        },
    },
    RegistryEntry {
        name: "typescript",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "export_statement",
                "interface_declaration",
                "type_alias_declaration",
            ],
            doc_prefixes: &["/**", " *", "//"],
            inner_node_types: &["method_definition"],
        },
    },
    RegistryEntry {
        name: "javascript",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "export_statement",
            ],
            doc_prefixes: &["/**", " *", "//"],
            inner_node_types: &["method_definition"],
        },
    },
    RegistryEntry {
        name: "tsx",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "export_statement",
                "interface_declaration",
                "type_alias_declaration",
            ],
            doc_prefixes: &["/**", " *", "//"],
            inner_node_types: &["method_definition"],
        },
    },
    RegistryEntry {
        name: "jsx",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "export_statement",
            ],
            doc_prefixes: &["/**", " *", "//"],
            inner_node_types: &["method_definition"],
        },
    },
    RegistryEntry {
        name: "java",
        spec: LanguageSpec {
            node_types: &[
                "method_declaration",
                "class_declaration",
                "interface_declaration",
                "constructor_declaration",
                "enum_declaration",
            ],
            doc_prefixes: &["/**", " *"],
            // Recurse into class/interface bodies.
            inner_node_types: &[
                "method_declaration",
                "constructor_declaration",
                "field_declaration",
            ],
        },
    },
    RegistryEntry {
        name: "kotlin",
        spec: LanguageSpec {
            node_types: &[
                "function_declaration",
                "class_declaration",
                "object_declaration",
                "property_declaration",
            ],
            doc_prefixes: &["/**", " *"],
            inner_node_types: &["function_declaration", "property_declaration"],
        },
    },
];

/// A located AST node to be turned into a chunk.
pub(crate) struct AstNode {
    /// 0-indexed start line.
    pub(crate) start_line: usize,
    /// 0-indexed end line (inclusive).
    pub(crate) end_line: usize,
}

/// Look up the language spec for the given language name (case-insensitive).
pub fn get_language_spec(lang: &str) -> Option<&'static LanguageSpec> {
    let lower = lang.to_lowercase();
    LANGUAGE_REGISTRY
        .iter()
        .find(|entry| entry.name == lower)
        .map(|entry| &entry.spec)
}

/// Returns `true` if the file extension indicates a markdown file.
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_lowercase();
            lower == "md" || lower == "markdown"
        })
        .unwrap_or(false)
}

/// Maps language name to tree-sitter grammar (case-insensitive).
///
/// JavaScript and JSX reuse the TypeScript/TSX grammars respectively —
/// TypeScript is a superset of JavaScript so the parse trees are compatible.
fn get_ts_language(lang: &str) -> Option<tree_sitter::Language> {
    match lang.to_ascii_lowercase().as_str() {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "typescript" | "javascript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" | "jsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "kotlin" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        _ => None,
    }
}

/// Parses source with tree-sitter and extracts top-level AST nodes.
///
/// If `spec` is `Some`, matches against `spec.node_types`. Otherwise uses a
/// generic heuristic: named nodes spanning 3+ lines with at least one named child.
pub(crate) fn extract_ast_nodes(
    source: &str,
    ts_lang: &tree_sitter::Language,
    spec: Option<&LanguageSpec>,
) -> anyhow::Result<Vec<AstNode>> {
    let mut parser = Parser::new();
    parser.set_language(ts_lang)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter parse failed"))?;

    let root = tree.root_node();
    let mut nodes = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        let is_extractable = if let Some(spec) = spec {
            spec.node_types.contains(&child.kind())
        } else {
            child.is_named()
                && (child
                    .end_position()
                    .row
                    .saturating_sub(child.start_position().row))
                    >= 2
                && has_named_child(child)
        };
        if is_extractable {
            nodes.push(AstNode {
                start_line: child.start_position().row,
                end_line: child.end_position().row,
            });
        }
    }
    Ok(nodes)
}

fn has_named_child(node: Node) -> bool {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor).any(|c| c.is_named());
    result
}

/// Converts AST nodes to RawChunks, handling gaps and doc expansion.
/// Recursively collect nodes matching `inner_types` from a tree-sitter subtree.
///
/// Recurses into non-matching nodes (e.g. `declaration_list`, `block`) but stops
/// when it finds a match — this avoids capturing nested lambdas or inner functions
/// inside methods while still finding methods at any depth inside a body node.
fn collect_inner_nodes(
    node: tree_sitter::Node,
    inner_types: &[&str],
    source_lines: &[&str],
    doc_prefixes: &[&str],
    line_offset: usize,
    result: &mut Vec<AstNode>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if inner_types.contains(&child.kind()) {
            // Expand doc comment upward within the container's local line space.
            let local_start = child.start_position().row;
            let expanded_local = expand_doc_comment_start(source_lines, local_start, doc_prefixes);
            result.push(AstNode {
                start_line: line_offset + expanded_local,
                end_line: line_offset + child.end_position().row,
            });
            // Do NOT recurse into matched nodes — prevents picking up nested
            // lambdas or inner functions inside method bodies.
        } else {
            collect_inner_nodes(
                child,
                inner_types,
                source_lines,
                doc_prefixes,
                line_offset,
                result,
            );
        }
    }
}

/// Attempt to extract inner declarations from a container node that is too large
/// to embed as a single chunk.
///
/// Re-parses the container's source text with tree-sitter and walks the resulting
/// tree to collect child nodes matching `inner_types`. Returns `None` if no inner
/// nodes are found (e.g. the container is a huge single function with no structure).
fn try_extract_inner_nodes(
    source_lines: &[&str],
    node: &AstNode,
    ts_lang: &tree_sitter::Language,
    inner_types: &[&str],
    doc_prefixes: &[&str],
) -> Option<Vec<AstNode>> {
    let node_source = source_lines[node.start_line..=node.end_line].join("\n");
    let node_lines: Vec<&str> = node_source.lines().collect();

    let mut parser = Parser::new();
    parser.set_language(ts_lang).ok()?;
    let tree = parser.parse(&node_source, None)?;

    let mut result = Vec::new();
    // `root_node()` is always a `source_file` wrapper — skip it and start at
    // the actual container node (impl_item, class_definition, etc.) so that
    // `collect_inner_nodes` iterates the container's children (methods, fields)
    // rather than collecting the container itself.
    let root = tree.root_node();
    let container = root.named_child(0).unwrap_or(root);
    collect_inner_nodes(
        container,
        inner_types,
        &node_lines,
        doc_prefixes,
        node.start_line,
        &mut result,
    );

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Extract the container header (doc comment + signature line up to `{` or `:`)
/// as a `RawChunk`. Used to give the embedding model context about what `impl` or
/// class the following method chunks belong to.
fn extract_container_header(
    lines: &[&str],
    start: usize,
    end: usize,
    doc_prefixes: &[&str],
) -> RawChunk {
    let node_lines = &lines[start..end];

    // Consume any doc comment lines at the start.
    let mut sig_end = 0;
    while sig_end < node_lines.len() && is_doc_line(node_lines[sig_end], doc_prefixes) {
        sig_end += 1;
    }

    // Consume non-doc lines until the signature terminator (`{`, `:`, or `=>`).
    let sig_search_start = sig_end;
    while sig_end < node_lines.len() && (sig_end - sig_search_start) < 3 {
        sig_end += 1;
        let trimmed = node_lines[sig_end - 1].trim();
        if trimmed.contains('{') || trimmed.ends_with(':') || trimmed.contains("=>") {
            break;
        }
    }

    // Guard against an empty window (e.g. a single-line node like `impl Foo {}`).
    let sig_end = sig_end.max(1).min(node_lines.len());

    RawChunk {
        content: node_lines[..sig_end].join("\n"),
        start_line: start + 1,
        end_line: start + sig_end,
    }
}

/// Converts AST nodes to RawChunks, handling gaps and doc expansion.
///
/// When a node exceeds `chunk_size` and the language spec defines
/// `inner_node_types`, the node is recursed into to produce one chunk per inner
/// declaration (method, constructor, etc.) plus a header chunk for the container
/// signature. Oversized nodes with no inner structure (e.g. a huge `main()`) fall
/// back to `sub_split_node` for plain-text line splitting.
///
/// Pass `ts_lang: None` to disable the recursive inner-node path (used for the
/// single-level recursion limit — inner calls never recurse further).
fn nodes_to_chunks(
    source: &str,
    nodes: &[AstNode],
    chunk_size: usize,
    doc_prefixes: &[&str],
    ts_lang: Option<&tree_sitter::Language>,
    spec: Option<&LanguageSpec>,
) -> Vec<RawChunk> {
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    let mut prev_end: usize = 0;

    for node in nodes {
        let expanded_start = expand_doc_comment_start(&lines, node.start_line, doc_prefixes);

        // Gap chunk: text between previous node and this one's (expanded) start.
        if expanded_start > prev_end {
            let gap_content = lines[prev_end..expanded_start].join("\n");
            if !gap_content.trim().is_empty() {
                if gap_content.len() > chunk_size {
                    let sub = super::chunker::split(&gap_content, chunk_size, 0);
                    for mut sc in sub {
                        // chunker::split returns 1-indexed lines relative to gap_content.
                        // prev_end is 0-indexed, so adding gives correct 1-indexed file lines.
                        sc.start_line += prev_end;
                        sc.end_line += prev_end;
                        chunks.push(sc);
                    }
                } else {
                    chunks.push(RawChunk {
                        content: gap_content,
                        start_line: prev_end + 1,
                        end_line: expanded_start,
                    });
                }
            }
        }

        // Node chunk
        let node_end = (node.end_line + 1).min(lines.len());
        let content = lines[expanded_start..node_end].join("\n");

        if content.len() <= chunk_size {
            chunks.push(RawChunk {
                content,
                start_line: expanded_start + 1,
                end_line: node_end,
            });
        } else {
            // Oversized node: try recursive inner-node extraction first.
            let inner_nodes = spec
                .filter(|s| !s.inner_node_types.is_empty())
                .zip(ts_lang)
                .and_then(|(s, ts)| {
                    try_extract_inner_nodes(&lines, node, ts, s.inner_node_types, doc_prefixes)
                });

            if let Some(inner) = inner_nodes {
                // Emit a header chunk (impl/class signature) for embedding context.
                let header =
                    extract_container_header(&lines, expanded_start, node_end, doc_prefixes);
                if !header.content.trim().is_empty() {
                    chunks.push(header);
                }
                // Recursively chunk inner nodes. Pass None/None to prevent further
                // recursion — oversized inner methods fall through to sub_split_node.
                let inner_chunks =
                    nodes_to_chunks(source, &inner, chunk_size, doc_prefixes, None, None);
                chunks.extend(inner_chunks);
            } else {
                // No inner structure — fall back to prefix + plain-text sub-splitting.
                let sub =
                    sub_split_node(&lines, expanded_start, node_end, chunk_size, doc_prefixes);
                chunks.extend(sub);
            }
        }

        prev_end = node_end;
    }

    // Trailing gap
    if prev_end < lines.len() {
        let gap_content = lines[prev_end..].join("\n");
        if !gap_content.trim().is_empty() {
            if gap_content.len() > chunk_size {
                let sub = super::chunker::split(&gap_content, chunk_size, 0);
                for mut sc in sub {
                    sc.start_line += prev_end;
                    sc.end_line += prev_end;
                    chunks.push(sc);
                }
            } else {
                chunks.push(RawChunk {
                    content: gap_content,
                    start_line: prev_end + 1,
                    end_line: lines.len(),
                });
            }
        }
    }

    chunks
}

/// Sub-split an oversized AST node, prepending doc comment + signature prefix
/// to every sub-chunk so each chunk retains the context of what it belongs to.
/// Sub-split an oversized AST node, prepending doc comment + signature prefix
/// to every sub-chunk so each chunk retains the context of what it belongs to.
///
/// Used as a last-resort fallback for nodes with no inner structure (e.g. a
/// 400-line `main()` function). Overlap is 0 — each sub-chunk is distinct.
fn sub_split_node(
    lines: &[&str],
    start: usize,
    end: usize,
    chunk_size: usize,
    doc_prefixes: &[&str],
) -> Vec<RawChunk> {
    let node_lines = &lines[start..end];

    // --- Step 1: Extract the prefix (doc comment + signature) ---
    let mut sig_end = 0; // exclusive index into node_lines where prefix ends

    // Consume doc comment lines from the start.
    while sig_end < node_lines.len() && is_doc_line(node_lines[sig_end], doc_prefixes) {
        sig_end += 1;
    }

    // After doc lines, consume non-doc lines until we find a signature terminator
    // (line containing `{`, ending with `:`, or containing `=>`). Cap at 3 non-doc lines.
    let sig_search_start = sig_end;
    let max_sig_lines = 3;
    while sig_end < node_lines.len() && (sig_end - sig_search_start) < max_sig_lines {
        sig_end += 1;
        let line = node_lines[sig_end - 1];
        let trimmed = line.trim();
        if trimmed.contains('{') || trimmed.ends_with(':') || trimmed.contains("=>") {
            break;
        }
    }

    let prefix = node_lines[..sig_end].join("\n");

    // --- Step 2: Sub-split the body ---
    let body_lines = &node_lines[sig_end..];
    if body_lines.is_empty() {
        // No body beyond the prefix — emit as single chunk
        return vec![RawChunk {
            content: node_lines.join("\n"),
            start_line: start + 1,
            end_line: end,
        }];
    }

    let continued_marker = "    // ... (continued)";
    let overhead = prefix.len() + 1 /* newline */ + continued_marker.len() + 1 /* newline */;
    let body_chunk_size = if chunk_size > overhead {
        chunk_size - overhead
    } else {
        // Pathological: chunk_size is tiny, just use a minimal body budget
        chunk_size / 2
    };

    let body_text = body_lines.join("\n");
    // Overlap is 0 — AST chunks have clean boundaries; no fragments should repeat.
    let sub_chunks = super::chunker::split(&body_text, body_chunk_size, 0);

    // --- Step 3: Prepend prefix to each sub-chunk ---
    sub_chunks
        .into_iter()
        .enumerate()
        .map(|(i, sc)| {
            let content = if i == 0 {
                format!("{}\n{}", prefix, sc.content)
            } else {
                format!("{}\n{}\n{}", prefix, continued_marker, sc.content)
            };

            // sc.start_line / sc.end_line are 1-indexed relative to body_text.
            // Convert to file-level 1-indexed line numbers.
            let body_offset = start + sig_end; // 0-indexed file line where body starts
            let start_line = if i == 0 {
                start + 1 // include prefix lines
            } else {
                body_offset + sc.start_line // sc.start_line is 1-indexed
            };
            let end_line = body_offset + sc.end_line; // sc.end_line is 1-indexed inclusive

            RawChunk {
                content,
                start_line,
                end_line,
            }
        })
        .collect()
}

/// Split a source file into chunks, using language-aware strategies where possible.
///
/// - Returns empty for empty source.
/// - Delegates to `split_markdown` for markdown files.
/// - Uses AST-based splitting for registered languages, with recursive inner-node
///   extraction for oversized container nodes (impl blocks, classes, etc.).
/// - Falls through to the plain text `split` for unrecognised languages.
///
/// `chunk_overlap` has been removed: AST chunks have clean semantic boundaries,
/// so overlap is meaningless. The plain-text fallback paths also use 0 overlap.
pub fn split_file(source: &str, lang: &str, path: &Path, chunk_size: usize) -> Vec<RawChunk> {
    if source.is_empty() {
        return vec![];
    }

    if is_markdown(path) {
        return super::chunker::split_markdown(source, chunk_size, 0);
    }

    // Try AST-based splitting
    let spec = get_language_spec(lang);
    if let Some(ts_lang) = get_ts_language(lang) {
        if let Ok(nodes) = extract_ast_nodes(source, &ts_lang, spec) {
            if !nodes.is_empty() {
                let doc_prefixes = spec.map(|s| s.doc_prefixes).unwrap_or(&["//"] as &[&str]);
                return nodes_to_chunks(
                    source,
                    &nodes,
                    chunk_size,
                    doc_prefixes,
                    Some(&ts_lang),
                    spec,
                );
            }
        }
    }

    // Fallback to line-based splitting
    super::chunker::split(source, chunk_size, 0)
}

/// Returns `true` if the given line is a doc comment line.
///
/// A line is considered a doc comment if:
/// - Its trimmed form starts with any of the given `doc_prefixes`, or
/// - Its trimmed form is `*/` (closing a block doc comment).
pub fn is_doc_line(line: &str, doc_prefixes: &[&str]) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    // Closing `*/` always matches as part of a block doc comment.
    if trimmed.starts_with("*/") {
        return true;
    }
    doc_prefixes.iter().any(|prefix| {
        // Check the trimmed line for prefixes without leading whitespace,
        // and the original line for prefixes that include leading whitespace
        // (e.g. " *" in Javadoc blocks).
        trimmed.starts_with(prefix) || line.starts_with(prefix)
    })
}

/// Expand a node's start line upward to include preceding doc comments.
///
/// Scans backward from `node_start_line`, skipping blank lines, to find
/// contiguous doc comment lines. Returns the earliest line that is part of
/// the doc comment block, or `node_start_line` if none is found.
pub fn expand_doc_comment_start(
    lines: &[&str],
    node_start_line: usize,
    doc_prefixes: &[&str],
) -> usize {
    if node_start_line == 0 {
        return 0;
    }

    // Phase 1: skip blank lines immediately above the node (max 2).
    let mut cursor = node_start_line;
    let mut blank_count = 0;
    while cursor > 0 && lines[cursor - 1].trim().is_empty() && blank_count < 2 {
        cursor -= 1;
        blank_count += 1;
    }

    // If we only found blank lines all the way to the top, no doc comment.
    if cursor == 0 && lines[0].trim().is_empty() {
        return node_start_line;
    }

    // Check if the line at cursor-1 is a doc line.
    if cursor == 0 || !is_doc_line(lines[cursor - 1], doc_prefixes) {
        return node_start_line;
    }

    // Phase 2: consume contiguous doc comment lines upward.
    let mut doc_start = cursor - 1;
    while doc_start > 0 && is_doc_line(lines[doc_start - 1], doc_prefixes) {
        doc_start -= 1;
    }

    doc_start
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- Registry lookup ----------

    #[test]
    fn registry_lookup_all_languages() {
        let languages = [
            "rust",
            "python",
            "go",
            "typescript",
            "javascript",
            "tsx",
            "jsx",
            "java",
            "kotlin",
        ];
        for lang in &languages {
            let spec = get_language_spec(lang);
            assert!(
                spec.is_some(),
                "expected LanguageSpec for '{}', got None",
                lang
            );
            let spec = spec.unwrap();
            assert!(
                !spec.node_types.is_empty(),
                "'{}' should have node_types",
                lang
            );
            assert!(
                !spec.doc_prefixes.is_empty(),
                "'{}' should have doc_prefixes",
                lang
            );
        }
    }

    #[test]
    fn registry_lookup_case_insensitive() {
        assert!(get_language_spec("Rust").is_some());
        assert!(get_language_spec("PYTHON").is_some());
        assert!(get_language_spec("TypeScript").is_some());
    }

    #[test]
    fn registry_returns_none_for_unknown() {
        assert!(get_language_spec("haskell").is_none());
        assert!(get_language_spec("brainfuck").is_none());
        assert!(get_language_spec("").is_none());
    }

    // ---------- split_file ----------

    #[test]
    fn split_file_empty_source() {
        let chunks = split_file("", "rust", Path::new("main.rs"), 4000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn split_file_markdown_delegates_to_markdown_splitter() {
        let source = "# Heading\n\nIntro.\n\n## Section\n\nBody text.\n";
        let chunks = split_file(source, "markdown", Path::new("README.md"), 4000);
        assert!(!chunks.is_empty());
        // Markdown splitter splits on headings, so we should get at least 2 chunks
        assert!(
            chunks.len() >= 2,
            "expected markdown heading split, got {} chunks",
            chunks.len()
        );
        assert!(chunks[0].content.contains("Heading"));
        assert!(chunks.iter().any(|c| c.content.contains("Section")));
    }

    #[test]
    fn split_file_markdown_uppercase_extension() {
        let source = "# Title\n\nText.\n\n## Part Two\n\nMore text.\n";
        let chunks = split_file(source, "markdown", Path::new("NOTES.MD"), 4000);
        assert!(chunks.len() >= 2, "should recognise .MD as markdown");
    }

    #[test]
    fn split_file_unknown_lang_falls_through_to_plain_split() {
        let source = "line 1\nline 2\nline 3\n";
        let chunks = split_file(source, "unknown_lang", Path::new("file.xyz"), 4000);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 1);
    }

    #[test]
    fn split_file_known_lang_uses_ast_split() {
        // Known languages now use AST-based splitting; a small function is still 1 chunk
        let source = "fn main() {\n    println!(\"hello\");\n}\n";
        let chunks = split_file(source, "rust", Path::new("main.rs"), 4000);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("fn main"));
    }

    // ---------- Doc comment expansion ----------

    #[test]
    fn expand_doc_comments_rust() {
        let source =
            "use std::io;\n\n/// Adds two numbers.\n/// Returns the sum.\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        let lines: Vec<&str> = source.lines().collect();
        // fn add is at line index 4 (0-indexed)
        let expanded = expand_doc_comment_start(&lines, 4, &["///", "//!"]);
        assert_eq!(expanded, 2, "should expand to include both /// lines");
    }

    #[test]
    fn expand_doc_comments_java_block() {
        let source = "import foo;\n\n/**\n * Does something.\n */\npublic void doIt() {\n}\n";
        let lines: Vec<&str> = source.lines().collect();
        // method starts at line index 5 (0-indexed)
        let expanded = expand_doc_comment_start(&lines, 5, &["/**", " *", " */"]);
        assert_eq!(expanded, 2, "should expand to include /** block");
    }

    #[test]
    fn expand_doc_comments_none() {
        let source = "use std::io;\n\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        let lines: Vec<&str> = source.lines().collect();
        let expanded = expand_doc_comment_start(&lines, 2, &["///", "//!"]);
        assert_eq!(expanded, 2, "no doc comment — should not expand");
    }

    #[test]
    fn expand_skips_blank_lines_between_doc_and_node() {
        let source = "/// Documented.\n\nfn foo() {}\n";
        let lines: Vec<&str> = source.lines().collect();
        let expanded = expand_doc_comment_start(&lines, 2, &["///"]);
        assert_eq!(expanded, 0, "should cross blank line to find doc comment");
    }

    #[test]
    fn expand_at_line_zero() {
        let source = "fn foo() {}\n";
        let lines: Vec<&str> = source.lines().collect();
        let expanded = expand_doc_comment_start(&lines, 0, &["///"]);
        assert_eq!(expanded, 0, "already at start — no expansion possible");
    }

    #[test]
    fn is_doc_line_matches_prefix() {
        assert!(is_doc_line("/// hello", &["///"]));
        assert!(is_doc_line("  /// indented", &["///"]));
        assert!(is_doc_line(" * middle of block", &[" *"]));
        assert!(is_doc_line(" */", &[" *"]));
        assert!(is_doc_line("*/", &[]), "closing */ always matches");
    }

    #[test]
    fn is_doc_line_rejects_non_doc() {
        assert!(!is_doc_line("fn foo() {}", &["///"]));
        assert!(!is_doc_line("// regular comment", &["///"]));
        assert!(!is_doc_line("", &["///"]));
    }

    // ---------- AST-based splitting ----------

    #[test]
    fn ast_split_rust_two_functions() {
        let source = "use std::io;\n\n/// Adds two numbers.\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n/// Subtracts b from a.\nfn sub(a: i32, b: i32) -> i32 {\n    a - b\n}\n";
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000);
        assert!(chunks.len() >= 2, "got {} chunks", chunks.len());
        let add_chunk = chunks
            .iter()
            .find(|c| c.content.contains("fn add"))
            .expect("add chunk");
        assert!(
            add_chunk.content.contains("/// Adds two numbers"),
            "add chunk should include doc"
        );
        let sub_chunk = chunks
            .iter()
            .find(|c| c.content.contains("fn sub"))
            .expect("sub chunk");
        assert!(
            sub_chunk.content.contains("/// Subtracts"),
            "sub chunk should include doc"
        );
        assert!(
            !add_chunk.content.contains("fn sub"),
            "add chunk should not contain sub"
        );
    }

    #[test]
    fn ast_split_python_function_with_comment() {
        let source = "import os\n\n# Helper to greet.\ndef greet(name):\n    return f'Hello {name}'\n\nclass Greeter:\n    def __init__(self, name):\n        self.name = name\n";
        let chunks = split_file(source, "python", Path::new("test.py"), 4000);
        assert!(
            chunks.len() >= 2,
            "should split into function + class, got {}",
            chunks.len()
        );
        let greet_chunk = chunks
            .iter()
            .find(|c| c.content.contains("def greet"))
            .expect("greet chunk");
        assert!(
            greet_chunk.content.contains("# Helper"),
            "greet should include doc comment"
        );
    }

    #[test]
    fn ast_split_preserves_line_numbers() {
        let source = "/// First.\nfn first() {}\n\n/// Second.\nfn second() {}\n";
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000);
        let first = chunks
            .iter()
            .find(|c| c.content.contains("fn first"))
            .unwrap();
        assert_eq!(
            first.start_line, 1,
            "first fn starts at line 1 (includes doc)"
        );
        let second = chunks
            .iter()
            .find(|c| c.content.contains("fn second"))
            .unwrap();
        assert_eq!(
            second.start_line, 4,
            "second fn starts at line 4 (includes doc)"
        );
    }

    #[test]
    fn ast_split_captures_gap_text() {
        let source = "use std::io;\nuse std::fmt;\n\nfn foo() {}\n";
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000);
        // Should have a gap chunk for the use statements and a chunk for foo
        let has_use = chunks.iter().any(|c| c.content.contains("use std::io"));
        let has_fn = chunks.iter().any(|c| c.content.contains("fn foo"));
        assert!(has_use, "should capture use statements as gap chunk");
        assert!(has_fn, "should capture function");
    }

    // ---------- Sub-split with prefix ----------

    #[test]
    fn oversized_node_is_sub_split_with_prefix() {
        let mut source = String::from("/// Important function.\nfn big() {\n");
        for i in 0..50 {
            source.push_str(&format!("    let x{} = {};\n", i, i));
        }
        source.push_str("}\n");

        let chunks = split_file(&source, "rust", Path::new("test.rs"), 300);
        assert!(
            chunks.len() > 1,
            "oversized fn should be sub-split, got {}",
            chunks.len()
        );

        // Every sub-chunk should contain the doc + signature prefix
        for (i, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.content.contains("/// Important function"),
                "sub-chunk {} missing doc prefix",
                i
            );
            assert!(
                chunk.content.contains("fn big()"),
                "sub-chunk {} missing signature prefix",
                i
            );
        }

        // First chunk should NOT have the "(continued)" marker
        assert!(!chunks[0].content.contains("(continued)"));
        // Second+ chunks should have it
        if chunks.len() > 1 {
            assert!(chunks[1].content.contains("(continued)"));
        }
    }

    #[test]
    fn sub_split_covers_all_body_lines() {
        let mut source = String::from("fn big() {\n");
        let body_lines: Vec<String> = (0..40)
            .map(|i| format!("    let x{} = {};", i, i))
            .collect();
        source.push_str(&body_lines.join("\n"));
        source.push_str("\n}\n");

        let chunks = split_file(&source, "rust", Path::new("test.rs"), 200);
        // Every body line should appear in at least one chunk
        for (i, body_line) in body_lines.iter().enumerate() {
            let covered = chunks
                .iter()
                .any(|c| c.content.contains(body_line.as_str()));
            assert!(covered, "body line {} not covered: {}", i, body_line);
        }
    }

    // ---------- Generic heuristic (no registry entry) ----------

    #[test]
    fn generic_heuristic_extracts_multiline_named_nodes() {
        // Use Rust grammar but call extract_ast_nodes with spec=None
        let source =
            "fn hello() {\n    println!(\"hi\");\n}\n\nfn world() {\n    println!(\"world\");\n}\n";
        let ts_lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let nodes = extract_ast_nodes(source, &ts_lang, None).unwrap();
        assert_eq!(nodes.len(), 2, "generic heuristic should find 2 functions");
    }

    #[test]
    fn generic_heuristic_ignores_single_line_nodes() {
        let source =
            "use std::io;\nuse std::fmt;\n\nfn multi_line() {\n    let x = 1;\n    let y = 2;\n}\n";
        let ts_lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let nodes = extract_ast_nodes(source, &ts_lang, None).unwrap();
        // use statements are single-line — should be ignored by heuristic
        assert_eq!(nodes.len(), 1, "should only find multi_line fn");
    }

    // ---------- Error resilience & fallback ----------

    #[test]
    fn broken_syntax_falls_back_to_line_splitting() {
        let source = "fn broken( { {{ missing close\n    let x = 1;\n    let y = 2;\n";
        // Should not panic — falls back to line-based
        let chunks = split_file(source, "rust", Path::new("test.rs"), 200);
        assert!(
            !chunks.is_empty(),
            "broken syntax should still produce chunks via fallback"
        );
    }

    #[test]
    fn ast_with_no_extractable_nodes_falls_back() {
        // A file with only comments and blank lines
        let source = "// just a comment\n// another comment\n\n";
        let chunks = split_file(source, "rust", Path::new("test.rs"), 200);
        assert!(
            !chunks.is_empty(),
            "should fall back to line-based for comment-only files"
        );
    }

    // ---------- Additional language coverage ----------

    #[test]
    fn ast_split_go_function_with_doc() {
        let source = "package main\n\nimport \"fmt\"\n\n// Greet prints a greeting.\nfunc Greet(name string) {\n\tfmt.Println(\"Hello\", name)\n}\n\n// Add returns the sum.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n";
        let chunks = split_file(source, "go", Path::new("main.go"), 4000);
        assert!(
            chunks.len() >= 2,
            "Go should split into 2+ chunks, got {}",
            chunks.len()
        );
        let greet = chunks
            .iter()
            .find(|c| c.content.contains("func Greet"))
            .expect("Greet chunk");
        assert!(
            greet.content.contains("// Greet prints"),
            "Go func should include doc comment"
        );
        let add = chunks
            .iter()
            .find(|c| c.content.contains("func Add"))
            .expect("Add chunk");
        assert!(
            !greet.content.contains("func Add"),
            "Greet chunk should not contain Add"
        );
        assert!(
            add.content.contains("// Add returns"),
            "Add should include doc comment"
        );
    }

    #[test]
    fn ast_split_typescript_with_jsdoc() {
        let source = "import { foo } from 'bar';\n\n/**\n * Adds two numbers.\n * @param a first\n * @param b second\n */\nfunction add(a: number, b: number): number {\n    return a + b;\n}\n\nfunction sub(a: number, b: number): number {\n    return a - b;\n}\n";
        let chunks = split_file(source, "typescript", Path::new("math.ts"), 4000);
        assert!(chunks.len() >= 2, "TS should split, got {}", chunks.len());
        let add_chunk = chunks
            .iter()
            .find(|c| c.content.contains("function add"))
            .expect("add chunk");
        assert!(
            add_chunk.content.contains("Adds two numbers"),
            "TS func should include JSDoc"
        );
    }

    #[test]
    fn ast_split_trailing_gap_captured() {
        let source = "fn foo() {\n    1\n}\n\n// trailing comment\nconst X: i32 = 42;\n";
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000);
        let has_trailing = chunks
            .iter()
            .any(|c| c.content.contains("trailing comment"));
        assert!(has_trailing, "trailing gap text should be captured");
    }

    #[test]
    fn expand_doc_does_not_bridge_many_blank_lines() {
        // 4 blank lines between doc comment and node — should NOT expand
        let source = "/// Orphaned doc.\n\n\n\n\nfn foo() {}\n";
        let lines: Vec<&str> = source.lines().collect();
        let expanded = expand_doc_comment_start(&lines, 5, &["///"]);
        assert_eq!(expanded, 5, "should not bridge 4 blank lines");
    }

    // ---------- Recursive inner-node extraction ----------

    #[test]
    fn recursive_impl_block_extracts_methods() {
        // Build an impl block with 3 methods, each large enough that the whole
        // impl exceeds chunk_size but individual methods fit within it.
        let mut source = String::from("struct Foo;\n\nimpl Foo {\n");
        // method1 — 8 lines
        source.push_str("    /// First method.\n    fn method1(&self) -> i32 {\n");
        for i in 0..6 {
            source.push_str(&format!("        let _x{} = {};\n", i, i));
        }
        source.push_str("    }\n");
        // method2 — 8 lines
        source.push_str("    /// Second method.\n    fn method2(&self) -> i32 {\n");
        for i in 0..6 {
            source.push_str(&format!("        let _y{} = {};\n", i, i));
        }
        source.push_str("    }\n");
        // method3 — 8 lines
        source.push_str("    /// Third method.\n    fn method3(&self) -> i32 {\n");
        for i in 0..6 {
            source.push_str(&format!("        let _z{} = {};\n", i, i));
        }
        source.push_str("    }\n}\n");

        // chunk_size = 200 forces recursion into the impl (total ~700+ chars)
        // but each individual method (~120 chars) fits within it.
        let chunks = split_file(&source, "rust", Path::new("test.rs"), 200);

        // Expect: 1 gap chunk (struct Foo), 1 header chunk (impl Foo {), 3 method chunks.
        let header = chunks
            .iter()
            .find(|c| c.content.contains("impl Foo"))
            .expect("should have an impl Foo header chunk");
        assert!(
            !header.content.contains("fn method1"),
            "header chunk should not include method bodies"
        );

        let m1 = chunks
            .iter()
            .find(|c| c.content.contains("fn method1"))
            .expect("method1 chunk");
        let m2 = chunks
            .iter()
            .find(|c| c.content.contains("fn method2"))
            .expect("method2 chunk");
        let m3 = chunks
            .iter()
            .find(|c| c.content.contains("fn method3"))
            .expect("method3 chunk");

        // Each method chunk should be distinct (no cross-contamination).
        assert!(
            !m1.content.contains("fn method2"),
            "m1 should not contain m2"
        );
        assert!(
            !m2.content.contains("fn method3"),
            "m2 should not contain m3"
        );
        assert!(
            !m3.content.contains("fn method1"),
            "m3 should not contain m1"
        );

        // Doc comments should be captured per method.
        assert!(
            m1.content.contains("/// First method"),
            "m1 should include its doc"
        );
        assert!(
            m2.content.contains("/// Second method"),
            "m2 should include its doc"
        );
        assert!(
            m3.content.contains("/// Third method"),
            "m3 should include its doc"
        );

        // Line numbers must be file-level (not relative to the impl block).
        // struct Foo + blank + impl Foo { = 3 lines before the first method.
        // method1 doc starts at line 4 (1-indexed) after struct+blank+impl.
        assert!(
            m1.start_line > 3,
            "method1 start_line {} should be > 3 (file-level, not impl-relative)",
            m1.start_line
        );
    }

    #[test]
    fn recursive_class_extracts_methods_python() {
        let mut source = String::from("import os\n\nclass MyService:\n");
        source.push_str("    # Process items.\n    def process(self, items):\n");
        for i in 0..8 {
            source.push_str(&format!("        item_{} = items[{}]\n", i, i));
        }
        source.push_str("        return items\n");
        source.push_str("    # Validate input.\n    def validate(self, data):\n");
        for i in 0..8 {
            source.push_str(&format!("        val_{} = data.get('{}')\n", i, i));
        }
        source.push_str("        return True\n");

        // chunk_size = 200 forces recursion into the class.
        let chunks = split_file(&source, "python", Path::new("service.py"), 200);

        let process = chunks.iter().find(|c| c.content.contains("def process"));
        let validate = chunks.iter().find(|c| c.content.contains("def validate"));

        assert!(process.is_some(), "should have a process chunk");
        assert!(validate.is_some(), "should have a validate chunk");

        let process = process.unwrap();
        let validate = validate.unwrap();
        assert!(
            !process.content.contains("def validate"),
            "process chunk should not contain validate"
        );
        assert!(
            process.content.contains("# Process items"),
            "process chunk should include its comment"
        );
        assert!(
            validate.content.contains("# Validate input"),
            "validate chunk should include its comment"
        );
    }

    #[test]
    fn recursive_falls_back_when_no_inner_types() {
        // Go has inner_node_types = [] so a large function must fall back to sub_split_node.
        let mut source = String::from("package main\n\n// BigFunc does a lot.\nfunc BigFunc() {\n");
        for i in 0..60 {
            source.push_str(&format!("\tx{} := {}\n", i, i));
        }
        source.push_str("}\n");

        let chunks = split_file(&source, "go", Path::new("big.go"), 300);

        // Should be sub-split (multiple chunks), each with the signature prefix.
        assert!(
            chunks.len() > 1,
            "large Go func should be sub-split, got {} chunks",
            chunks.len()
        );
        // The leading "package main" gap chunk doesn't carry the function signature —
        // only the BigFunc sub-split chunks do. Filter to those and verify.
        let func_chunks: Vec<_> = chunks
            .iter()
            .filter(|c| c.content.contains("func BigFunc"))
            .collect();
        assert!(
            func_chunks.len() > 1,
            "large Go func should produce multiple sub-chunks with signature prefix, got {}",
            func_chunks.len()
        );
    }

    #[test]
    fn recursive_impl_inner_doc_comments_included() {
        // Doc comments on inner methods must be captured in the method chunks,
        // not left behind in the header or lost entirely.
        let mut source = String::from("impl Calculator {\n");
        source.push_str("    /// Adds a and b.\n    /// Returns the sum.\n");
        source.push_str("    fn add(&self, a: i32, b: i32) -> i32 {\n");
        for i in 0..12 {
            source.push_str(&format!("        let _step{} = {};\n", i, i));
        }
        source.push_str("        a + b\n    }\n");
        source.push_str("    /// Subtracts b from a.\n");
        source.push_str("    fn sub(&self, a: i32, b: i32) -> i32 {\n");
        for i in 0..12 {
            source.push_str(&format!("        let _step{} = {};\n", i, i));
        }
        source.push_str("        a - b\n    }\n}\n");

        let chunks = split_file(&source, "rust", Path::new("calc.rs"), 250);

        let add_chunk = chunks
            .iter()
            .find(|c| c.content.contains("fn add"))
            .expect("add chunk");
        assert!(
            add_chunk.content.contains("/// Adds a and b"),
            "add chunk should include first doc line"
        );
        assert!(
            add_chunk.content.contains("/// Returns the sum"),
            "add chunk should include second doc line"
        );

        let sub_chunk = chunks
            .iter()
            .find(|c| c.content.contains("fn sub"))
            .expect("sub chunk");
        assert!(
            sub_chunk.content.contains("/// Subtracts b from a"),
            "sub chunk should include its doc"
        );
    }

    #[test]
    fn chunk_overlap_removed_from_ast_paths() {
        // Verify that gap chunks between functions contain no duplicate content
        // (overlap = 0, so no lines from one chunk should appear in the next).
        let source = concat!(
            "use std::io;\n",
            "use std::fmt;\n",
            "\n",
            "fn foo() {\n    let x = 1;\n}\n",
            "\n",
            "fn bar() {\n    let y = 2;\n}\n",
        );

        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000);

        // With zero overlap and small source, each declaration is its own chunk.
        // The gap (use statements) should appear exactly once.
        let gap_chunks: Vec<_> = chunks
            .iter()
            .filter(|c| c.content.contains("use std::io"))
            .collect();
        assert_eq!(
            gap_chunks.len(),
            1,
            "use statements should appear in exactly one chunk, not duplicated by overlap"
        );
    }
}
