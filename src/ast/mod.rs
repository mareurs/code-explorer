//! AST engine: offline, in-process code parsing via tree-sitter.
//!
//! Provides symbol extraction and structural queries without requiring a
//! running language server. Used as the primary fallback when no LSP is
//! configured, and as a complement to LSP for fast structural analysis.

pub mod parser;

use anyhow::Result;
use std::path::Path;

use crate::lsp::symbols::SymbolInfo;
pub use parser::DocstringInfo;

/// Extract top-level symbols from a file using tree-sitter.
///
/// Faster than an LSP round-trip and works offline — ideal for initial
/// indexing or when an LSP is unavailable.
pub fn extract_symbols(path: &Path) -> Result<Vec<SymbolInfo>> {
    let source = std::fs::read_to_string(path)?;
    let language = detect_language(path);
    parser::extract_symbols_from_source(&source, language, path)
}

/// Extract docstrings/comments from a file using tree-sitter.
pub fn extract_docstrings(path: &Path) -> Result<Vec<DocstringInfo>> {
    let source = std::fs::read_to_string(path)?;
    let language = detect_language(path);
    parser::extract_docstrings_from_source(&source, language, path)
}

/// Detect the programming language from a file extension.
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
        "md" => Some("markdown"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "json" => Some("json"),
        _ => None,
    }
}
