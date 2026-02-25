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
pub struct LanguageSpec {
    /// Tree-sitter node types that represent top-level declarations.
    pub node_types: &'static [&'static str],
    /// Prefixes that introduce documentation comments.
    pub doc_prefixes: &'static [&'static str],
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
        },
    },
];

/// A located AST node to be turned into a chunk.
struct AstNode {
    /// 0-indexed start line.
    start_line: usize,
    /// 0-indexed end line (inclusive).
    end_line: usize,
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

/// Maps language name to tree-sitter grammar.
fn get_ts_language(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
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
fn extract_ast_nodes(
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
        let dominated = if let Some(spec) = spec {
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
        if dominated {
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
fn nodes_to_chunks(
    source: &str,
    nodes: &[AstNode],
    chunk_size: usize,
    chunk_overlap: usize,
    doc_prefixes: &[&str],
) -> Vec<RawChunk> {
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    let mut prev_end: usize = 0;

    for node in nodes {
        let expanded_start = expand_doc_comment_start(&lines, node.start_line, doc_prefixes);

        // Gap chunk
        if expanded_start > prev_end {
            let gap_content = lines[prev_end..expanded_start].join("\n");
            if !gap_content.trim().is_empty() {
                if gap_content.len() > chunk_size {
                    let sub = super::chunker::split(&gap_content, chunk_size, chunk_overlap);
                    for mut sc in sub {
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
            // Sub-split oversized node with doc+signature prefix on each sub-chunk
            let sub = sub_split_node(
                &lines,
                expanded_start,
                node_end,
                chunk_size,
                chunk_overlap,
                doc_prefixes,
            );
            chunks.extend(sub);
        }

        prev_end = node_end;
    }

    // Trailing gap
    if prev_end < lines.len() {
        let gap_content = lines[prev_end..].join("\n");
        if !gap_content.trim().is_empty() {
            if gap_content.len() > chunk_size {
                let sub = super::chunker::split(&gap_content, chunk_size, chunk_overlap);
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
fn sub_split_node(
    lines: &[&str],
    start: usize,
    end: usize,
    chunk_size: usize,
    chunk_overlap: usize,
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
    let sub_chunks = super::chunker::split(&body_text, body_chunk_size, chunk_overlap);

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
/// - Uses AST-based splitting for registered languages.
/// - Falls through to the plain text `split` for unrecognised languages.
pub fn split_file(
    source: &str,
    lang: &str,
    path: &Path,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Vec<RawChunk> {
    if source.is_empty() {
        return vec![];
    }

    if is_markdown(path) {
        return super::chunker::split_markdown(source, chunk_size, chunk_overlap);
    }

    // Try AST-based splitting
    let spec = get_language_spec(lang);
    if let Some(ts_lang) = get_ts_language(lang) {
        if let Ok(nodes) = extract_ast_nodes(source, &ts_lang, spec) {
            if !nodes.is_empty() {
                let doc_prefixes = spec.map(|s| s.doc_prefixes).unwrap_or(&["//"] as &[&str]);
                return nodes_to_chunks(source, &nodes, chunk_size, chunk_overlap, doc_prefixes);
            }
        }
    }

    // Fallback to line-based splitting
    super::chunker::split(source, chunk_size, chunk_overlap)
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

    // Phase 1: skip blank lines immediately above the node.
    let mut cursor = node_start_line;
    while cursor > 0 && lines[cursor - 1].trim().is_empty() {
        cursor -= 1;
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
        let chunks = split_file("", "rust", Path::new("main.rs"), 4000, 400);
        assert!(chunks.is_empty());
    }

    #[test]
    fn split_file_markdown_delegates_to_markdown_splitter() {
        let source = "# Heading\n\nIntro.\n\n## Section\n\nBody text.\n";
        let chunks = split_file(source, "markdown", Path::new("README.md"), 4000, 400);
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
        let chunks = split_file(source, "markdown", Path::new("NOTES.MD"), 4000, 400);
        assert!(chunks.len() >= 2, "should recognise .MD as markdown");
    }

    #[test]
    fn split_file_unknown_lang_falls_through_to_plain_split() {
        let source = "line 1\nline 2\nline 3\n";
        let chunks = split_file(source, "unknown_lang", Path::new("file.xyz"), 4000, 400);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 1);
    }

    #[test]
    fn split_file_known_lang_uses_ast_split() {
        // Known languages now use AST-based splitting; a small function is still 1 chunk
        let source = "fn main() {\n    println!(\"hello\");\n}\n";
        let chunks = split_file(source, "rust", Path::new("main.rs"), 4000, 400);
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
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000, 200);
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
        let chunks = split_file(source, "python", Path::new("test.py"), 4000, 200);
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
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000, 200);
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
        let chunks = split_file(source, "rust", Path::new("test.rs"), 4000, 200);
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

        let chunks = split_file(&source, "rust", Path::new("test.rs"), 300, 50);
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

        let chunks = split_file(&source, "rust", Path::new("test.rs"), 200, 30);
        // Every body line should appear in at least one chunk
        for (i, body_line) in body_lines.iter().enumerate() {
            let covered = chunks
                .iter()
                .any(|c| c.content.contains(body_line.as_str()));
            assert!(covered, "body line {} not covered: {}", i, body_line);
        }
    }
}
