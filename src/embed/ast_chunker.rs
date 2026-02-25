//! AST-aware code chunker with language registry.
//!
//! Provides language-specific knowledge (node types, doc comment prefixes) used
//! to split source files into semantically meaningful chunks. Currently delegates
//! to the plain text chunker; AST extraction will be added in a later task.

use std::path::Path;

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

/// Split a source file into chunks, using language-aware strategies where possible.
///
/// - Returns empty for empty source.
/// - Delegates to `split_markdown` for markdown files.
/// - Falls through to the plain text `split` for everything else (AST extraction
///   will be added in a later task).
pub fn split_file(
    source: &str,
    _lang: &str,
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

    // TODO: AST-aware splitting will be added in Task 3.
    // For now, fall through to the plain text chunker.
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
    fn split_file_known_lang_falls_through_to_plain_split() {
        // Until AST extraction is implemented, known languages also use the plain splitter
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
}
