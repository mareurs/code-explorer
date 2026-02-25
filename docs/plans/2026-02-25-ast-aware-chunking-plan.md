# AST-Aware Semantic Chunking Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace line-based chunking with AST-aware splitting so each embedded chunk is a complete semantic unit (function + doc comment).

**Architecture:** New `src/embed/ast_chunker.rs` module with a language registry dispatching to tree-sitter extraction, generic heuristic, or line-based fallback. Existing `chunker.rs` untouched — reused for sub-splitting oversized nodes.

**Tech Stack:** tree-sitter (already in Cargo.toml for 7 languages), existing `chunker::split()` / `split_markdown()` for sub-splitting and fallback.

**Design doc:** `docs/plans/2026-02-25-ast-aware-chunking-design.md`

---

### Task 1: Language Registry & Scaffolding

**Files:**
- Create: `src/embed/ast_chunker.rs`
- Modify: `src/embed/mod.rs:13` (add `pub mod ast_chunker;`)

**Step 1: Write the failing test**

In `src/embed/ast_chunker.rs`, write the module with registry structs and a test that verifies lookup:

```rust
//! AST-aware semantic chunker.
//!
//! Splits source files into chunks aligned to code structure (functions,
//! classes, structs) using tree-sitter, with doc comments attached.
//! Falls back to line-based splitting for unsupported languages.

use std::path::Path;

use super::chunker::RawChunk;

/// Specification for how to split a language's AST into chunks.
struct LanguageSpec {
    /// Tree-sitter node types representing top-level code units.
    node_types: &'static [&'static str],
    /// Line prefixes indicating doc comments (scanned backward from node).
    doc_prefixes: &'static [&'static str],
}

/// Get the language spec for a registered language.
fn get_language_spec(lang: &str) -> Option<&'static LanguageSpec> {
    LANGUAGE_REGISTRY
        .iter()
        .find(|(name, _)| *name == lang)
        .map(|(_, spec)| spec)
}

static LANGUAGE_REGISTRY: &[(&str, LanguageSpec)] = &[
    ("rust", LanguageSpec {
        node_types: &[
            "function_item", "struct_item", "enum_item", "trait_item",
            "impl_item", "mod_item", "type_item", "const_item",
            "static_item", "macro_definition",
        ],
        doc_prefixes: &["///", "//!"],
    }),
    ("python", LanguageSpec {
        node_types: &[
            "function_definition", "class_definition",
            "decorated_definition", "async_function_definition",
        ],
        doc_prefixes: &["#"],
    }),
    ("go", LanguageSpec {
        node_types: &[
            "function_declaration", "method_declaration",
            "type_declaration", "var_declaration", "const_declaration",
        ],
        doc_prefixes: &["//"],
    }),
    ("typescript", LanguageSpec {
        node_types: &[
            "function_declaration", "class_declaration", "method_definition",
            "export_statement", "interface_declaration", "type_alias_declaration",
        ],
        doc_prefixes: &["/**", " *", "//"],
    }),
    ("javascript", LanguageSpec {
        node_types: &[
            "function_declaration", "class_declaration", "method_definition",
            "export_statement",
        ],
        doc_prefixes: &["/**", " *", "//"],
    }),
    ("tsx", LanguageSpec {
        node_types: &[
            "function_declaration", "class_declaration", "method_definition",
            "export_statement", "interface_declaration", "type_alias_declaration",
        ],
        doc_prefixes: &["/**", " *", "//"],
    }),
    ("jsx", LanguageSpec {
        node_types: &[
            "function_declaration", "class_declaration", "method_definition",
            "export_statement",
        ],
        doc_prefixes: &["/**", " *", "//"],
    }),
    ("java", LanguageSpec {
        node_types: &[
            "method_declaration", "class_declaration", "interface_declaration",
            "constructor_declaration", "enum_declaration",
        ],
        doc_prefixes: &["/**", " *"],
    }),
    ("kotlin", LanguageSpec {
        node_types: &[
            "function_declaration", "class_declaration", "object_declaration",
            "property_declaration",
        ],
        doc_prefixes: &["/**", " *"],
    }),
];

/// Split a source file into semantic chunks using AST when available.
///
/// Dispatch chain:
/// 1. Markdown → heading-aware splitting
/// 2. Language with registry entry → AST with registered node types
/// 3. Language with tree-sitter grammar → AST with generic heuristic
/// 4. Fallback → line-based splitting with overlap
pub fn split_file(
    source: &str,
    lang: &str,
    _path: &Path,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Vec<RawChunk> {
    if source.is_empty() {
        return vec![];
    }

    // 1. Markdown
    if lang == "markdown" {
        return super::chunker::split_markdown(source, chunk_size, chunk_overlap);
    }

    // 2-3. AST-based (stub — falls through to line-based for now)
    // TODO: implement in Task 2

    // 4. Line-based fallback
    super::chunker::split(source, chunk_size, chunk_overlap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_finds_rust() {
        let spec = get_language_spec("rust").expect("rust should be registered");
        assert!(spec.node_types.contains(&"function_item"));
        assert!(spec.doc_prefixes.contains(&"///"));
    }

    #[test]
    fn registry_finds_all_languages() {
        for lang in &["rust", "python", "go", "typescript", "javascript", "tsx", "jsx", "java", "kotlin"] {
            assert!(get_language_spec(lang).is_some(), "{} should be registered", lang);
        }
    }

    #[test]
    fn registry_returns_none_for_unknown() {
        assert!(get_language_spec("ruby").is_none());
        assert!(get_language_spec("haskell").is_none());
    }

    #[test]
    fn split_file_empty_returns_empty() {
        let chunks = split_file("", "rust", Path::new("test.rs"), 1200, 200);
        assert!(chunks.is_empty());
    }

    #[test]
    fn split_file_markdown_delegates_to_markdown_splitter() {
        let source = "# Title\n\nIntro.\n\n## Section\n\nContent.\n";
        let chunks = split_file(source, "markdown", Path::new("test.md"), 500, 50);
        assert!(chunks.len() >= 2, "markdown should split on headings");
    }

    #[test]
    fn split_file_unknown_lang_falls_back_to_line_splitter() {
        let source = (0..50).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let chunks = split_file(&source, "ruby", Path::new("test.rb"), 200, 20);
        assert!(!chunks.is_empty(), "fallback should produce chunks");
    }
}
```

**Step 2: Add module to `src/embed/mod.rs`**

Add `pub mod ast_chunker;` after line 13 (`pub mod chunker;`).

**Step 3: Run tests to verify they pass**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 6 tests PASS

**Step 4: Commit**

```bash
git add src/embed/ast_chunker.rs src/embed/mod.rs
git commit -m "feat(embed): add ast_chunker scaffold with language registry"
```

---

### Task 2: Doc Comment Expansion

**Files:**
- Modify: `src/embed/ast_chunker.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `ast_chunker.rs`:

```rust
#[test]
fn expand_doc_comments_rust() {
    let source = "use std::io;\n\n/// Adds two numbers.\n/// Returns the sum.\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let lines: Vec<&str> = source.lines().collect();
    // function_item starts at line index 4 (0-indexed), "fn add..."
    let expanded = expand_doc_comment_start(&lines, 4, &["///", "//!"]);
    assert_eq!(expanded, 2, "should expand to include both /// lines");
}

#[test]
fn expand_doc_comments_java() {
    let source = "import foo;\n\n/**\n * Does something.\n */\npublic void doIt() {\n}\n";
    let lines: Vec<&str> = source.lines().collect();
    // method starts at line index 5 (0-indexed), "public void doIt..."
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
```

**Step 2: Implement `expand_doc_comment_start`**

Add to `ast_chunker.rs` (above tests module):

```rust
/// Expand a node's start line upward to include preceding doc comments.
///
/// Scans backward from `node_start_line` (0-indexed) through `lines`.
/// Skips blank lines, then includes consecutive lines starting with any
/// of `doc_prefixes`. Returns the new (possibly earlier) start line index.
fn expand_doc_comment_start(
    lines: &[&str],
    node_start_line: usize,
    doc_prefixes: &[&str],
) -> usize {
    if node_start_line == 0 {
        return 0;
    }

    let mut i = node_start_line - 1;

    // Skip blank lines immediately above the node
    while i > 0 && lines[i].trim().is_empty() {
        i -= 1;
    }
    // Check if we landed on a blank line at index 0
    if lines[i].trim().is_empty() {
        return node_start_line;
    }

    // Check if this line is a doc comment
    if !is_doc_line(lines[i], doc_prefixes) {
        return node_start_line;
    }

    // Scan upward through consecutive doc comment lines
    let mut doc_start = i;
    while doc_start > 0 {
        let prev = lines[doc_start - 1].trim_start();
        if is_doc_line(lines[doc_start - 1], doc_prefixes) {
            doc_start -= 1;
        } else {
            break;
        }
    }

    doc_start
}

/// Check if a line is a doc comment line (matches any prefix after trim).
fn is_doc_line(line: &str, doc_prefixes: &[&str]) -> bool {
    let trimmed = line.trim_start();
    // Also match closing */ for block comments
    if trimmed == "*/" {
        return true;
    }
    doc_prefixes.iter().any(|prefix| trimmed.starts_with(prefix))
}
```

**Step 3: Run tests**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 10 tests PASS (6 old + 4 new)

**Step 4: Commit**

```bash
git add src/embed/ast_chunker.rs
git commit -m "feat(embed): add doc comment expansion for AST chunker"
```

---

### Task 3: Core AST Extraction — Registered Languages

**Files:**
- Modify: `src/embed/ast_chunker.rs`

**Step 1: Write the failing test**

Add to tests:

```rust
#[test]
fn ast_split_rust_two_functions() {
    let source = "\
use std::io;

/// Adds two numbers.
fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Subtracts b from a.
fn sub(a: i32, b: i32) -> i32 {
    a - b
}
";
    let chunks = split_file(source, "rust", Path::new("test.rs"), 4000, 200);
    // Should have 3 chunks: gap (use stmt), add fn, sub fn
    assert!(chunks.len() >= 2, "got {} chunks: {:?}", chunks.len(),
        chunks.iter().map(|c| c.content.lines().next().unwrap_or("")).collect::<Vec<_>>());
    // Each function chunk should contain its doc comment
    let add_chunk = chunks.iter().find(|c| c.content.contains("fn add")).expect("add chunk");
    assert!(add_chunk.content.contains("/// Adds two numbers"), "add chunk should include doc");
    let sub_chunk = chunks.iter().find(|c| c.content.contains("fn sub")).expect("sub chunk");
    assert!(sub_chunk.content.contains("/// Subtracts"), "sub chunk should include doc");
    // Functions should be in separate chunks
    assert!(!add_chunk.content.contains("fn sub"), "add chunk should not contain sub");
}

#[test]
fn ast_split_python_class_and_function() {
    let source = "\
import os

# Helper to greet.
def greet(name):
    return f'Hello {name}'

class Greeter:
    def __init__(self, name):
        self.name = name
";
    let chunks = split_file(source, "python", Path::new("test.py"), 4000, 200);
    assert!(chunks.len() >= 2, "should have at least 2 chunks (fn + class)");
    let greet_chunk = chunks.iter().find(|c| c.content.contains("def greet")).expect("greet chunk");
    assert!(greet_chunk.content.contains("# Helper"), "greet should include doc comment");
}

#[test]
fn ast_split_preserves_line_numbers() {
    let source = "\
/// First.
fn first() {}

/// Second.
fn second() {}
";
    let chunks = split_file(source, "rust", Path::new("test.rs"), 4000, 200);
    let first = chunks.iter().find(|c| c.content.contains("fn first")).unwrap();
    assert_eq!(first.start_line, 1, "first fn starts at line 1");
    let second = chunks.iter().find(|c| c.content.contains("fn second")).unwrap();
    assert_eq!(second.start_line, 4, "second fn starts at line 4");
}
```

**Step 2: Implement AST extraction**

Add `use crate::ast::parser;` at the top of `ast_chunker.rs`.

Replace the `// TODO: implement in Task 2` section in `split_file()` with:

```rust
    // Try tree-sitter parsing
    let ts_lang = get_ts_language(lang);
    let spec = get_language_spec(lang);

    if let Some(ts_lang) = ts_lang {
        if let Ok(nodes) = extract_ast_nodes(source, &ts_lang, spec) {
            if !nodes.is_empty() {
                return nodes_to_chunks(source, &nodes, chunk_size, chunk_overlap,
                    spec.map(|s| s.doc_prefixes).unwrap_or(&["//"]));
            }
        }
    }
```

Add the helper functions:

```rust
use tree_sitter::{Node, Parser};

/// A located AST node to be turned into a chunk.
struct AstNode {
    /// 0-indexed start line (possibly expanded to include doc comment).
    start_line: usize,
    /// 0-indexed end line (inclusive).
    end_line: usize,
}

/// Get the tree-sitter Language for a language name.
/// Re-uses the same grammar set as `crate::ast::parser`.
fn get_ts_language(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "javascript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "jsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "kotlin" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        _ => None,
    }
}

/// Extract AST nodes from source using tree-sitter.
///
/// If `spec` is provided, uses registered node types.
/// Otherwise, uses the generic heuristic.
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
            // Generic heuristic: named, 3+ lines, has named children
            child.is_named()
                && (child.end_position().row - child.start_position().row) >= 2
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
    node.children(&mut cursor).any(|c| c.is_named())
}

/// Convert AST nodes into RawChunks, handling gaps and doc expansion.
fn nodes_to_chunks(
    source: &str,
    nodes: &[AstNode],
    chunk_size: usize,
    chunk_overlap: usize,
    doc_prefixes: &[&str],
) -> Vec<RawChunk> {
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    let mut prev_end: usize = 0; // 0-indexed line after previous node

    for node in nodes {
        // Expand start to include doc comments
        let expanded_start = expand_doc_comment_start(&lines, node.start_line, doc_prefixes);

        // Gap chunk: text between previous node and this one's expanded start
        if expanded_start > prev_end {
            let gap_content = lines[prev_end..expanded_start].join("\n");
            let trimmed = gap_content.trim();
            if !trimmed.is_empty() {
                // Sub-split gap if oversized, with overlap
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

        // Node chunk (with doc comment)
        let node_end = (node.end_line + 1).min(lines.len()); // exclusive
        let content = lines[expanded_start..node_end].join("\n");

        if content.len() <= chunk_size {
            chunks.push(RawChunk {
                content,
                start_line: expanded_start + 1, // 1-indexed
                end_line: node_end,
            });
        } else {
            // Sub-split oversized node (Task 4)
            let sub = sub_split_node(&lines, expanded_start, node_end, chunk_size, chunk_overlap);
            chunks.extend(sub);
        }

        prev_end = node_end;
    }

    // Trailing gap after last node
    if prev_end < lines.len() {
        let gap_content = lines[prev_end..].join("\n");
        let trimmed = gap_content.trim();
        if !trimmed.is_empty() {
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

/// Sub-split an oversized node. Stub for Task 4.
fn sub_split_node(
    lines: &[&str],
    start: usize,
    end: usize,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Vec<RawChunk> {
    // Temporary: plain line-based split (replaced in Task 4)
    let content = lines[start..end].join("\n");
    let sub = super::chunker::split(&content, chunk_size, chunk_overlap);
    sub.into_iter()
        .map(|mut sc| {
            sc.start_line += start;
            sc.end_line += start;
            sc
        })
        .collect()
}
```

**Step 3: Run tests**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 13 tests PASS (10 old + 3 new)

**Step 4: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 5: Commit**

```bash
git add src/embed/ast_chunker.rs
git commit -m "feat(embed): AST extraction for registered languages with doc expansion"
```

---

### Task 4: Sub-splitting Oversized Nodes with Prefix

**Files:**
- Modify: `src/embed/ast_chunker.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn oversized_node_is_sub_split_with_prefix() {
    // Create a function that exceeds chunk_size
    let mut source = String::from("/// Important function.\nfn big() {\n");
    for i in 0..50 {
        source.push_str(&format!("    let x{} = {};\n", i, i));
    }
    source.push_str("}\n");

    let chunks = split_file(&source, "rust", Path::new("test.rs"), 300, 50);
    assert!(chunks.len() > 1, "oversized fn should be sub-split, got {}", chunks.len());

    // Every sub-chunk should contain the doc + signature prefix
    for (i, chunk) in chunks.iter().enumerate() {
        assert!(
            chunk.content.contains("/// Important function"),
            "sub-chunk {} missing doc prefix", i
        );
        assert!(
            chunk.content.contains("fn big()"),
            "sub-chunk {} missing signature prefix", i
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
    let body_lines: Vec<String> = (0..40).map(|i| format!("    let x{} = {};", i, i)).collect();
    source.push_str(&body_lines.join("\n"));
    source.push_str("\n}\n");

    let chunks = split_file(&source, "rust", Path::new("test.rs"), 200, 30);
    // Every body line should appear in at least one chunk
    for (i, body_line) in body_lines.iter().enumerate() {
        let covered = chunks.iter().any(|c| c.content.contains(body_line.as_str()));
        assert!(covered, "body line {} not covered: {}", i, body_line);
    }
}
```

**Step 2: Implement `sub_split_node` with prefix**

Replace the stub `sub_split_node` function:

```rust
/// Sub-split an oversized node, carrying doc+signature as prefix in each chunk.
fn sub_split_node(
    lines: &[&str],
    start: usize,
    end: usize,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Vec<RawChunk> {
    let node_lines = &lines[start..end];

    // Find the signature boundary: first line containing '{', ':', or '=>'
    // after any doc comment lines, limited to first 5 non-doc lines.
    let mut sig_end = 0;
    let mut non_doc_count = 0;
    for (i, line) in node_lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("///") || trimmed.starts_with("//!")
            || trimmed.starts_with("/**") || trimmed.starts_with(" *")
            || trimmed.starts_with("#") && !trimmed.starts_with("#[")
            || trimmed.starts_with("\"\"\"")
        {
            sig_end = i + 1;
            continue;
        }
        non_doc_count += 1;
        sig_end = i + 1;
        if trimmed.contains('{') || trimmed.ends_with(':') || trimmed.contains("=>") {
            break;
        }
        if non_doc_count >= 3 {
            break;
        }
    }

    let prefix_lines = &node_lines[..sig_end];
    let prefix = prefix_lines.join("\n");
    let continued_marker = "    // ... (continued)";

    let body = &node_lines[sig_end..];
    if body.is_empty() {
        // Entire node is just signature — emit as single chunk
        let content = node_lines.join("\n");
        return vec![RawChunk {
            content,
            start_line: start + 1,
            end_line: end,
        }];
    }

    let body_text = body.join("\n");
    let prefix_overhead = prefix.len() + continued_marker.len() + 2; // +2 for newlines
    let body_chunk_size = chunk_size.saturating_sub(prefix_overhead).max(100);

    let sub_chunks = super::chunker::split(&body_text, body_chunk_size, chunk_overlap);

    sub_chunks
        .into_iter()
        .enumerate()
        .map(|(i, sc)| {
            let content = if i == 0 {
                // First sub-chunk: prefix + body (no continued marker)
                format!("{}\n{}", prefix, sc.content)
            } else {
                // Subsequent sub-chunks: prefix + marker + body
                format!("{}\n{}\n{}", prefix, continued_marker, sc.content)
            };
            RawChunk {
                content,
                start_line: if i == 0 { start + 1 } else { start + sig_end + sc.start_line },
                end_line: start + sig_end + sc.end_line,
            }
        })
        .collect()
}
```

**Step 3: Run tests**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 15 tests PASS (13 old + 2 new)

**Step 4: Commit**

```bash
git add src/embed/ast_chunker.rs
git commit -m "feat(embed): sub-split oversized AST nodes with doc+sig prefix"
```

---

### Task 5: Generic Heuristic for Unregistered Languages

**Files:**
- Modify: `src/embed/ast_chunker.rs`

**Step 1: Write the failing test**

The generic heuristic is already wired in Task 3 via the `else` branch in
`extract_ast_nodes`. We need a test that exercises it with a language that has a
tree-sitter grammar but no registry entry. Go is registered, but we can test the
heuristic directly.

```rust
#[test]
fn generic_heuristic_extracts_multiline_named_nodes() {
    // Use Rust grammar but pretend no registry entry by calling extract_ast_nodes
    // with spec=None
    let source = "\
fn hello() {
    println!(\"hi\");
}

fn world() {
    println!(\"world\");
}
";
    let ts_lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let nodes = extract_ast_nodes(source, &ts_lang, None).unwrap();
    assert_eq!(nodes.len(), 2, "generic heuristic should find 2 functions");
}

#[test]
fn generic_heuristic_ignores_single_line_nodes() {
    let source = "\
use std::io;
use std::fmt;

fn multi_line() {
    let x = 1;
    let y = 2;
}
";
    let ts_lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let nodes = extract_ast_nodes(source, &ts_lang, None).unwrap();
    // use statements are single-line named nodes — should be ignored
    assert_eq!(nodes.len(), 1, "should only find multi_line fn");
    assert_eq!(nodes[0].start_line, 3);
}
```

**Step 2: Run tests (they should already pass from Task 3 implementation)**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 17 tests PASS

If they don't pass, adjust the generic heuristic threshold or `has_named_child`
check. The heuristic from Task 3 should already handle this.

**Step 3: Commit**

```bash
git add src/embed/ast_chunker.rs
git commit -m "test(embed): verify generic heuristic for unregistered languages"
```

---

### Task 6: Error Resilience & Fallback

**Files:**
- Modify: `src/embed/ast_chunker.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn broken_syntax_falls_back_to_line_splitting() {
    let source = "fn broken( { {{ missing close\n    let x = 1;\n    let y = 2;\n";
    // Should not panic — falls back to line-based
    let chunks = split_file(source, "rust", Path::new("test.rs"), 200, 20);
    assert!(!chunks.is_empty(), "broken syntax should still produce chunks via fallback");
}

#[test]
fn ast_with_no_extractable_nodes_falls_back() {
    // A file with only comments and blank lines — no AST nodes
    let source = "// just a comment\n// another comment\n\n";
    let chunks = split_file(source, "rust", Path::new("test.rs"), 200, 20);
    assert!(!chunks.is_empty(), "should fall back to line-based for comment-only files");
}
```

**Step 2: Verify the fallback path works**

The `split_file` function from Task 3 already falls through to line-based
splitting when `extract_ast_nodes` returns an error or empty list. The
`nodes.is_empty()` check handles the no-nodes case. Tree-sitter's
`parser.parse()` is tolerant of syntax errors (returns a partial tree), so the
error case is harder to trigger — but the `Ok(nodes)` being empty handles it.

Ensure the extract function catches parse failures:

In `split_file`, the existing code:
```rust
if let Ok(nodes) = extract_ast_nodes(source, &ts_lang, spec) {
    if !nodes.is_empty() {
        return nodes_to_chunks(...);
    }
}
```
...already falls through on `Err` or empty nodes.

**Step 3: Run tests**

Run: `cargo test --lib embed::ast_chunker -- -v`
Expected: 19 tests PASS

**Step 4: Commit**

```bash
git add src/embed/ast_chunker.rs
git commit -m "test(embed): verify error resilience and fallback behavior"
```

---

### Task 7: Integration — Wire into `build_index()`

**Files:**
- Modify: `src/embed/index.rs:280-292`

**Step 1: Write a test that verifies the integration point compiles**

No new test needed — the existing `build_index` tests cover the pipeline. The
change is a single call-site swap.

**Step 2: Change `build_index()` call site**

In `src/embed/index.rs`, replace lines 280-292:

```rust
// BEFORE:
let chunks = if lang == "markdown" {
    chunker::split_markdown(
        &source,
        config.embeddings.chunk_size,
        config.embeddings.chunk_overlap,
    )
} else {
    chunker::split(
        &source,
        config.embeddings.chunk_size,
        config.embeddings.chunk_overlap,
    )
};

// AFTER:
let chunks = super::ast_chunker::split_file(
    &source,
    lang,
    path,
    config.embeddings.chunk_size,
    config.embeddings.chunk_overlap,
);
```

Also update the import at the top of `build_index()` — remove `chunker` from
the use statement since it's no longer called directly:

```rust
// BEFORE:
use crate::embed::{chunker, create_embedder, Embedding};
// AFTER:
use crate::embed::{create_embedder, Embedding};
```

**Step 3: Run full test suite**

Run: `cargo test --lib`
Expected: All 183+ tests PASS (181 original + new ast_chunker tests)

**Step 4: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- -D warnings`
Expected: Clean

**Step 5: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): wire ast_chunker into build_index pipeline"
```

---

### Task 8: Final Verification & Cleanup

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 3: Run fmt**

Run: `cargo fmt -- --check`
Expected: No formatting issues

**Step 4: Verify unused code**

Check that `chunker::split` and `chunker::split_markdown` are still used (via
`ast_chunker` sub-splitting). No dead code warnings should appear.

**Step 5: Commit any cleanup**

```bash
git add -A
git commit -m "chore: final cleanup for AST-aware chunking"
```
