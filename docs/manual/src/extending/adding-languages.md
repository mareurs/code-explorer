# Adding Languages

codescout supports languages at three levels, each building on the
previous. You can ship a partial implementation and add deeper support later.

| Level | What it enables | Effort |
|-------|-----------------|--------|
| Detection only | File detection, semantic search chunking, basic file ops | 1 line |
| LSP support | All symbol tools (`find_symbol`, `list_symbols`, references, rename) | ~10 lines |
| Tree-sitter grammar | Richer offline AST extraction, improved symbol fallback | ~50–200 lines |

---

## Level 1: Detection Only (easiest)

Add an extension mapping in `src/ast/mod.rs` in the `detect_language()` function:

```rust
pub fn detect_language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" => Some("javascript"),
        "jsx" => Some("jsx"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "c" => Some("c"),
        "cpp" | "cc" | "cxx" => Some("cpp"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "ex" | "exs" => Some("elixir"),
        "hs" => Some("haskell"),
        "lua" => Some("lua"),
        "sh" | "bash" => Some("bash"),
        // Add your language here:
        "zig" => Some("zig"),
        _ => None,
    }
}
```

The string you return (e.g. `"zig"`) becomes the canonical language identifier
used throughout the codebase. Keep it lowercase, no spaces.

**What this enables:**

- `detect_language()` calls throughout the codebase recognize your file type
- The semantic search chunker can split files of this type into chunks
- `list_dir` reports the language for each file
- `search_pattern` and `find_file` include these files in results

This is enough to ship. Many languages in the current codebase (e.g. `php`,
`swift`, `scala`, `elixir`, `haskell`, `lua`, `bash`) have detection only.

---

## Level 2: LSP Support (medium)

LSP support enables all seven symbol tools. You need two changes.

### Add a server config

In `src/lsp/servers/mod.rs`, add a match arm to `default_config()`:

```rust
pub fn default_config(language: &str, workspace_root: &Path) -> Option<LspServerConfig> {
    let root = workspace_root.to_path_buf();
    match language {
        "rust" => Some(LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: root,
        }),
        // ... existing languages ...
        "ruby" => Some(LspServerConfig {
            command: "solargraph".into(),
            args: vec!["stdio".into()],
            workspace_root: root,
        }),
        // Add your language:
        "zig" => Some(LspServerConfig {
            command: "zls".into(),
            args: vec![],
            workspace_root: root,
        }),
        _ => None,
    }
}
```

The `LspServerConfig` struct has three fields:

```rust
pub struct LspServerConfig {
    /// Executable to launch (e.g. "rust-analyzer", "pyright-langserver")
    pub command: String,
    /// Arguments passed to the executable
    pub args: Vec<String>,
    /// Working directory (usually the project root)
    pub workspace_root: PathBuf,
}
```

The server must speak LSP over stdio. Most language servers do this by default
or with a `--stdio` flag.

### Add the language ID mapping (if needed)

The LSP spec sometimes uses a different language identifier than our canonical
name. For example, TSX files use `"typescriptreact"` in the LSP protocol. If
your language's LSP ID differs from the canonical name, add a mapping in
`lsp_language_id()`:

```rust
pub fn lsp_language_id(lang: &str) -> &str {
    match lang {
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        // ... existing mappings ...
        // Only add here if the LSP ID differs from your canonical name:
        // "zig" => "zig",  // Not needed — same as canonical name
        other => other,  // Falls through if names match
    }
}
```

Most languages use the same identifier for both, so you likely do not need to
touch this function.

**What this enables:**

- `list_symbols` — symbol tree for files and directories
- `find_symbol` — search symbols by name across the project
- `find_references` — find all callers/references
- `replace_symbol` — edit a symbol's source code
- `insert_code` — insert code before or after symbols
- `rename_symbol` — project-wide rename
- `rename_symbol` — project-wide rename

The `LspManager` starts the server lazily on first use and keeps it alive for
subsequent requests.

---

## Level 3: Tree-sitter Grammar (full support)

Tree-sitter gives you offline symbol extraction without a running language
server. This improves the fallback path when LSP is unavailable and enables
richer AST extraction used internally by symbol tools.

### Step 1: Add the tree-sitter crate

Add the grammar crate to `Cargo.toml`:

```toml
[dependencies]
tree-sitter-zig = "0.1"   # Use the latest version
```

Tree-sitter grammars are compiled statically into the binary. There are no
runtime grammar files to distribute.

### Step 2: Add the language mapping

In `src/ast/parser.rs`, add a match arm to `get_ts_language()`:

```rust
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
        // Add your language:
        "zig" => Some(tree_sitter_zig::LANGUAGE.into()),
        _ => None,
    }
}
```

Note: the exact API varies by crate. Some expose `LANGUAGE`, others
`language()`. Check the crate's docs.

### Step 3: Write the symbol extractor

Create an `extract_zig_symbols()` function following the pattern of existing
extractors. Here is a simplified skeleton based on the Rust extractor:

```rust
fn extract_zig_symbols(
    node: Node,
    source: &str,
    file: &PathBuf,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Function,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "struct_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Struct,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            // Add more node kinds as needed...
            _ => {}
        }
    }

    symbols
}
```

**Key helpers** already available in `src/ast/parser.rs`:

- `child_name(node, source, field)` — extracts a named field from a tree-sitter node
- `make_name_path(prefix, name)` — builds `"Parent/Child"` name paths
- `find_child_by_kind(node, kind)` — finds a child node by its tree-sitter kind

To discover the correct node kinds for your language, use `tree-sitter
parse <file>` on a sample source file, or inspect the grammar's
`node-types.json`.

### Step 4: Add the dispatch case

In `extract_symbols_from_source()`, add your language to the match:

```rust
pub fn extract_symbols_from_source(
    source: &str,
    language: Option<&'static str>,
    path: &Path,
) -> Result<Vec<SymbolInfo>> {
    // ... parser setup ...

    match lang {
        "rust" => Ok(extract_rust_symbols(root, source, &file, "")),
        "python" => Ok(extract_python_symbols(root, source, &file, "")),
        "go" => Ok(extract_go_symbols(root, source, &file, "")),
        "typescript" | "javascript" | "tsx" | "jsx" => {
            Ok(extract_ts_symbols(root, source, &file, ""))
        }
        "java" => Ok(extract_java_symbols(root, source, &file, "")),
        "kotlin" => Ok(extract_kotlin_symbols(root, source, &file, "")),
        // Add your language:
        "zig" => Ok(extract_zig_symbols(root, source, &file, "")),
        _ => Ok(vec![]),
    }
}
```

### Step 5: Add docstring extraction (optional)

If the language has a documentation comment convention, add a corresponding
`extract_zig_docstrings()` function and wire it into
`extract_docstrings_from_source()`. This follows the same pattern as
`extract_symbols_from_source()`.

**What this enables:**

- Richer offline symbol extraction used internally by `list_symbols` and semantic chunking
- Better fallback when the LSP server is unavailable or slow to start

---

## Testing

### Detection and AST

Run the full test suite:

```bash
cargo test
```

The AST tests in `src/ast/parser.rs` exercise each extractor with sample
source code. Add a test for your language following the existing pattern — parse
a small snippet, assert on the extracted symbols.

### LSP

LSP support requires the actual language server binary to be installed on the
system. This makes it impractical to test in CI, so manual testing is the norm:

1. Install the language server (e.g. `zls` for Zig)
2. Create or find a test project in that language
3. Run the MCP server against it:
   ```bash
   cargo run -- start --project /path/to/test-project
   ```
4. Use an MCP client (or `curl` against the SSE endpoint) to invoke symbol
   tools and verify results

---

## Checklist

When adding a new language, use this as a quick reference:

- [ ] `src/ast/mod.rs` — extension mapping in `detect_language()`
- [ ] `src/lsp/servers/mod.rs` — server config in `default_config()` (if LSP available)
- [ ] `src/lsp/servers/mod.rs` — ID mapping in `lsp_language_id()` (only if LSP ID differs)
- [ ] `Cargo.toml` — tree-sitter crate dependency (if adding grammar)
- [ ] `src/ast/parser.rs` — grammar in `get_ts_language()` (if adding grammar)
- [ ] `src/ast/parser.rs` — `extract_<lang>_symbols()` function (if adding grammar)
- [ ] `src/ast/parser.rs` — dispatch in `extract_symbols_from_source()` (if adding grammar)
- [ ] `cargo test` — all tests pass
- [ ] `cargo clippy -- -D warnings` — no warnings
- [ ] Update `docs/manual/src/language-support.md` with the new language
