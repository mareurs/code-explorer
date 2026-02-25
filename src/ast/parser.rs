//! Tree-sitter based symbol and docstring extractor.
//!
//! Provides offline symbol extraction from source code without requiring a
//! running language server. Supports Rust, Python, TypeScript/TSX, Go, Java, Kotlin.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

use crate::lsp::symbols::{SymbolInfo, SymbolKind};

/// Information about a docstring or comment associated with a symbol.
#[derive(Debug, Clone)]
pub struct DocstringInfo {
    /// The symbol this docstring is associated with, if any.
    pub symbol_name: Option<String>,
    /// The docstring/comment content (leading markers stripped).
    pub content: String,
    /// 0-indexed start line.
    pub start_line: u32,
    /// 0-indexed end line.
    pub end_line: u32,
}

// ---------------------------------------------------------------------------
// Language resolution
// ---------------------------------------------------------------------------

/// Get the tree-sitter Language for a language name.
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

// ---------------------------------------------------------------------------
// Symbol extraction
// ---------------------------------------------------------------------------

/// Extract symbols from source text using tree-sitter for the given language.
pub fn extract_symbols_from_source(
    source: &str,
    language: Option<&'static str>,
    path: &Path,
) -> Result<Vec<SymbolInfo>> {
    let lang = language.ok_or_else(|| anyhow!("Unknown language for {:?}", path))?;
    let ts_lang =
        get_ts_language(lang).ok_or_else(|| anyhow!("No tree-sitter grammar for '{}'", lang))?;

    let mut parser = Parser::new();
    parser.set_language(&ts_lang)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter parse failed for {:?}", path))?;

    let root = tree.root_node();
    let file = path.to_path_buf();

    match lang {
        "rust" => Ok(extract_rust_symbols(root, source, &file, "")),
        "python" => Ok(extract_python_symbols(root, source, &file, "")),
        "go" => Ok(extract_go_symbols(root, source, &file, "")),
        "typescript" | "javascript" | "tsx" | "jsx" => {
            Ok(extract_ts_symbols(root, source, &file, ""))
        }
        "java" => Ok(extract_java_symbols(root, source, &file, "")),
        "kotlin" => Ok(extract_kotlin_symbols(root, source, &file, "")),
        _ => Ok(vec![]),
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction
// ---------------------------------------------------------------------------

/// Extract docstrings and comments from source text using tree-sitter.
pub fn extract_docstrings_from_source(
    source: &str,
    language: Option<&'static str>,
    path: &Path,
) -> Result<Vec<DocstringInfo>> {
    let lang = language.ok_or_else(|| anyhow!("Unknown language for {:?}", path))?;
    let ts_lang =
        get_ts_language(lang).ok_or_else(|| anyhow!("No tree-sitter grammar for '{}'", lang))?;

    let mut parser = Parser::new();
    parser.set_language(&ts_lang)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter parse failed for {:?}", path))?;

    let root = tree.root_node();

    match lang {
        "rust" => Ok(extract_rust_docstrings(root, source)),
        "python" => Ok(extract_python_docstrings(root, source)),
        "go" => Ok(extract_go_docstrings(root, source)),
        "typescript" | "javascript" | "tsx" | "jsx" => Ok(extract_ts_docstrings(root, source)),
        "java" => Ok(extract_java_docstrings(root, source)),
        "kotlin" => Ok(extract_kotlin_docstrings(root, source)),
        _ => Ok(vec![]),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the text of a named child node.
fn child_name(node: Node, source: &str, field: &str) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string())
}

/// Build a name_path by joining prefix and name.
fn make_name_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", prefix, name)
    }
}

/// Get the first named child with a specific kind.
fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor).find(|c| c.kind() == kind);
    result
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

fn extract_rust_symbols(node: Node, source: &str, file: &PathBuf, prefix: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
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
            "struct_item" => {
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
            "enum_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    // Extract enum variants as children
                    let children = extract_enum_variants(child, source, file, &np);
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Enum,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "trait_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "declaration_list");
                    let children = body
                        .map(|b| extract_rust_symbols(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Interface,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "impl_item" => {
                // impl Type { ... } or impl Trait for Type { ... }
                let type_name = child_name(child, source, "type").or_else(|| {
                    // Fallback: find type_identifier child
                    let mut c = child.walk();
                    let found = child
                        .children(&mut c)
                        .find(|n| n.kind() == "type_identifier");
                    found
                        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                        .map(|s| s.to_string())
                });
                if let Some(name) = type_name {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "declaration_list");
                    let methods = body
                        .map(|b| extract_rust_impl_methods(b, source, file, &np))
                        .unwrap_or_default();
                    // Don't create a symbol for impl blocks; merge methods at current level
                    // This matches how LSP reports symbols (methods under the type)
                    symbols.extend(methods);
                }
            }
            "mod_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "declaration_list");
                    let children = body
                        .map(|b| extract_rust_symbols(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Module,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "const_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Constant,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "static_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Variable,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "type_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::TypeParameter,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            // function_signature_item inside trait declarations
            "function_signature_item" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Method,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            _ => {}
        }
    }

    symbols
}

fn extract_rust_impl_methods(
    body: Node,
    source: &str,
    file: &Path,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut methods = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        if child.kind() == "function_item" {
            if let Some(name) = child_name(child, source, "name") {
                methods.push(SymbolInfo {
                    name_path: make_name_path(prefix, &name),
                    name,
                    kind: SymbolKind::Method,
                    file: file.to_path_buf(),
                    start_line: child.start_position().row as u32,
                    end_line: child.end_position().row as u32,
                    start_col: child.start_position().column as u32,
                    children: vec![],
                });
            }
        }
    }

    methods
}

fn extract_enum_variants(node: Node, source: &str, file: &Path, prefix: &str) -> Vec<SymbolInfo> {
    let mut variants = Vec::new();
    let body = match find_child_by_kind(node, "enum_variant_list") {
        Some(b) => b,
        None => return variants,
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "enum_variant" {
            if let Some(name) = child_name(child, source, "name") {
                variants.push(SymbolInfo {
                    name_path: make_name_path(prefix, &name),
                    name,
                    kind: SymbolKind::EnumMember,
                    file: file.to_path_buf(),
                    start_line: child.start_position().row as u32,
                    end_line: child.end_position().row as u32,
                    start_col: child.start_position().column as u32,
                    children: vec![],
                });
            }
        }
    }
    variants
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn extract_python_symbols(
    node: Node,
    source: &str,
    file: &PathBuf,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let kind = if prefix.is_empty() {
                        SymbolKind::Function
                    } else {
                        SymbolKind::Method
                    };
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "class_definition" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "block");
                    let children = body
                        .map(|b| extract_python_symbols(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Class,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "decorated_definition" => {
                // Unwrap to the actual function/class inside
                let inner = extract_python_symbols(child, source, file, prefix);
                symbols.extend(inner);
            }
            _ => {}
        }
    }

    symbols
}

/// Extract the receiver type name from a Go method declaration.
fn extract_go_receiver<'a>(node: Node<'a>, source: &'a str) -> &'a str {
    // Go grammar uses "receiver" field (a parameter_list) for the (p *Point) part
    let receiver = match node.child_by_field_name("receiver") {
        Some(r) => r,
        None => return "",
    };
    let mut c1 = receiver.walk();
    let param_decl = match receiver
        .children(&mut c1)
        .find(|n| n.kind() == "parameter_declaration")
    {
        Some(p) => p,
        None => return "",
    };
    // Get the last named child (the type)
    let mut c2 = param_decl.walk();
    let type_node = match param_decl.children(&mut c2).filter(|n| n.is_named()).last() {
        Some(t) => t,
        None => return "",
    };
    // Handle *Type (pointer_type) — get the inner type
    let final_node = if type_node.kind() == "pointer_type" {
        let mut c3 = type_node.walk();
        let found = type_node
            .children(&mut c3)
            .find(|n| n.kind() == "type_identifier");
        match found {
            Some(n) => n,
            None => return "",
        }
    } else {
        type_node
    };
    final_node.utf8_text(source.as_bytes()).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn extract_go_symbols(node: Node, source: &str, file: &Path, prefix: &str) -> Vec<SymbolInfo> {
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
                        file: file.to_path_buf(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "method_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    // Try to extract receiver type for name_path
                    let receiver = extract_go_receiver(child, source);
                    let np = if receiver.is_empty() {
                        make_name_path(prefix, &name)
                    } else {
                        let full_prefix = make_name_path(prefix, receiver);
                        make_name_path(&full_prefix, &name)
                    };
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Method,
                        file: file.to_path_buf(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "type_declaration" => {
                // type_declaration contains type_spec children
                let mut inner_cursor = child.walk();
                for spec in child.children(&mut inner_cursor) {
                    if spec.kind() == "type_spec" {
                        if let Some(name) = child_name(spec, source, "name") {
                            let np = make_name_path(prefix, &name);
                            // Determine kind from the type body
                            let kind = spec
                                .child_by_field_name("type")
                                .map(|t| match t.kind() {
                                    "struct_type" => SymbolKind::Struct,
                                    "interface_type" => SymbolKind::Interface,
                                    _ => SymbolKind::TypeParameter,
                                })
                                .unwrap_or(SymbolKind::TypeParameter);
                            // Extract struct fields or interface methods
                            let children = extract_go_type_children(spec, source, file, &np);
                            symbols.push(SymbolInfo {
                                name_path: np,
                                name,
                                kind,
                                file: file.to_path_buf(),
                                start_line: spec.start_position().row as u32,
                                end_line: spec.end_position().row as u32,
                                start_col: spec.start_position().column as u32,
                                children,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    symbols
}

fn extract_go_type_children(
    spec: Node,
    source: &str,
    file: &Path,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut children = Vec::new();
    let type_node = match spec.child_by_field_name("type") {
        Some(t) => t,
        None => return children,
    };

    match type_node.kind() {
        "struct_type" => {
            if let Some(field_list) = find_child_by_kind(type_node, "field_declaration_list") {
                let mut cursor = field_list.walk();
                for field in field_list.children(&mut cursor) {
                    if field.kind() == "field_declaration" {
                        if let Some(name) = child_name(field, source, "name") {
                            children.push(SymbolInfo {
                                name_path: make_name_path(prefix, &name),
                                name,
                                kind: SymbolKind::Field,
                                file: file.to_path_buf(),
                                start_line: field.start_position().row as u32,
                                end_line: field.end_position().row as u32,
                                start_col: field.start_position().column as u32,
                                children: vec![],
                            });
                        }
                    }
                }
            }
        }
        "interface_type" => {
            // Interface methods are method_spec nodes
            let mut cursor = type_node.walk();
            for child in type_node.children(&mut cursor) {
                if child.kind() == "method_spec" {
                    if let Some(name) = child_name(child, source, "name") {
                        children.push(SymbolInfo {
                            name_path: make_name_path(prefix, &name),
                            name,
                            kind: SymbolKind::Method,
                            file: file.to_path_buf(),
                            start_line: child.start_position().row as u32,
                            end_line: child.end_position().row as u32,
                            start_col: child.start_position().column as u32,
                            children: vec![],
                        });
                    }
                }
            }
        }
        _ => {}
    }

    children
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

fn extract_java_symbols(node: Node, source: &str, file: &PathBuf, prefix: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "class_body");
                    let children = body
                        .map(|b| extract_java_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Class,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "interface_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "interface_body");
                    let children = body
                        .map(|b| extract_java_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Interface,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let children = extract_java_enum_constants(child, source, file, &np);
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Enum,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "record_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "class_body");
                    let children = body
                        .map(|b| extract_java_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Struct,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            _ => {}
        }
    }

    symbols
}

fn extract_java_class_members(
    body: Node,
    source: &str,
    file: &PathBuf,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        match child.kind() {
            "method_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Method,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "constructor_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Constructor,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "field_declaration" => {
                // field_declaration → variable_declarator → name
                if let Some(decl) = child.child_by_field_name("declarator") {
                    if let Some(name) = child_name(decl, source, "name") {
                        members.push(SymbolInfo {
                            name_path: make_name_path(prefix, &name),
                            name,
                            kind: SymbolKind::Field,
                            file: file.clone(),
                            start_line: child.start_position().row as u32,
                            end_line: child.end_position().row as u32,
                            start_col: child.start_position().column as u32,
                            children: vec![],
                        });
                    }
                }
            }
            // Nested types
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                let inner = extract_java_symbols(body, source, file, prefix);
                members.extend(inner);
            }
            _ => {}
        }
    }

    members
}

fn extract_java_enum_constants(
    node: Node,
    source: &str,
    file: &Path,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut constants = Vec::new();
    let body = match find_child_by_kind(node, "enum_body") {
        Some(b) => b,
        None => return constants,
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "enum_constant" {
            if let Some(name) = child_name(child, source, "name") {
                constants.push(SymbolInfo {
                    name_path: make_name_path(prefix, &name),
                    name,
                    kind: SymbolKind::EnumMember,
                    file: file.to_path_buf(),
                    start_line: child.start_position().row as u32,
                    end_line: child.end_position().row as u32,
                    start_col: child.start_position().column as u32,
                    children: vec![],
                });
            }
        }
    }
    constants
}

// ---------------------------------------------------------------------------
// Kotlin
// ---------------------------------------------------------------------------

fn extract_kotlin_symbols(
    node: Node,
    source: &str,
    file: &PathBuf,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    // Determine kind: check modifiers for enum/interface/annotation
                    let kind = detect_kotlin_class_kind(child, source);
                    let body = find_child_by_kind(child, "class_body")
                        .or_else(|| find_child_by_kind(child, "enum_class_body"));
                    let children = body
                        .map(|b| extract_kotlin_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "object_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "class_body");
                    let children = body
                        .map(|b| extract_kotlin_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Class,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
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
            "property_declaration" => {
                // property_declaration → variable_declaration → identifier
                let name = extract_kotlin_property_name(child, source);
                if let Some(name) = name {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Property,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "type_alias" => {
                if let Some(name) = child_name(child, source, "type") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::TypeParameter,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            _ => {}
        }
    }

    symbols
}

/// Detect whether a Kotlin class_declaration is a class, enum, or interface.
fn detect_kotlin_class_kind(node: Node, source: &str) -> SymbolKind {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" || !child.is_named() {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            if text.contains("enum") {
                return SymbolKind::Enum;
            }
            if text.contains("interface") {
                return SymbolKind::Interface;
            }
            if text.contains("annotation") {
                return SymbolKind::Interface;
            }
        }
    }
    SymbolKind::Class
}

/// Extract the property name from a Kotlin property_declaration.
fn extract_kotlin_property_name(node: Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let var_decl = node
        .children(&mut cursor)
        .find(|c| c.kind() == "variable_declaration");
    let var_decl = var_decl?;
    // variable_declaration has an identifier child
    let mut cursor2 = var_decl.walk();
    let ident = var_decl
        .children(&mut cursor2)
        .find(|c| c.kind() == "identifier");
    let ident = ident?;
    ident
        .utf8_text(source.as_bytes())
        .ok()
        .map(|s| s.to_string())
}

fn extract_kotlin_class_members(
    body: Node,
    source: &str,
    file: &PathBuf,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Method,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "property_declaration" => {
                let name = extract_kotlin_property_name(child, source);
                if let Some(name) = name {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Property,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "secondary_constructor" => {
                members.push(SymbolInfo {
                    name_path: make_name_path(prefix, "constructor"),
                    name: "constructor".to_string(),
                    kind: SymbolKind::Constructor,
                    file: file.clone(),
                    start_line: child.start_position().row as u32,
                    end_line: child.end_position().row as u32,
                    start_col: child.start_position().column as u32,
                    children: vec![],
                });
            }
            "companion_object" => {
                let name =
                    child_name(child, source, "name").unwrap_or_else(|| "Companion".to_string());
                let np = make_name_path(prefix, &name);
                let inner_body = find_child_by_kind(child, "class_body");
                let children = inner_body
                    .map(|b| extract_kotlin_class_members(b, source, file, &np))
                    .unwrap_or_default();
                members.push(SymbolInfo {
                    name_path: np,
                    name,
                    kind: SymbolKind::Class,
                    file: file.clone(),
                    start_line: child.start_position().row as u32,
                    end_line: child.end_position().row as u32,
                    start_col: child.start_position().column as u32,
                    children,
                });
            }
            // Nested class/object declarations
            "class_declaration" | "object_declaration" => {
                let inner = extract_kotlin_symbols(child, source, file, prefix);
                members.extend(inner);
            }
            // Enum entries
            "enum_entry" => {
                // enum_entry has identifier child, not a "name" field
                let mut entry_cursor = child.walk();
                let ident = child
                    .children(&mut entry_cursor)
                    .find(|c| c.kind() == "identifier");
                let ident = match ident {
                    Some(i) => i,
                    None => continue,
                };
                if let Ok(name) = ident.utf8_text(source.as_bytes()) {
                    let name = name.to_string();
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::EnumMember,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            _ => {}
        }
    }

    members
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript
// ---------------------------------------------------------------------------

fn extract_ts_symbols(node: Node, source: &str, file: &PathBuf, prefix: &str) -> Vec<SymbolInfo> {
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
            "class_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    let body = find_child_by_kind(child, "class_body");
                    let children = body
                        .map(|b| extract_ts_class_members(b, source, file, &np))
                        .unwrap_or_default();
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Class,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children,
                    });
                }
            }
            "interface_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Interface,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    let np = make_name_path(prefix, &name);
                    symbols.push(SymbolInfo {
                        name_path: np,
                        name,
                        kind: SymbolKind::Enum,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = child_name(child, source, "name") {
                    symbols.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::TypeParameter,
                        file: file.clone(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "export_statement" => {
                // export function ..., export class ..., export default ...
                let inner = extract_ts_symbols(child, source, file, prefix);
                symbols.extend(inner);
            }
            _ => {}
        }
    }

    symbols
}

fn extract_ts_class_members(
    body: Node,
    source: &str,
    file: &Path,
    prefix: &str,
) -> Vec<SymbolInfo> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        match child.kind() {
            "method_definition" => {
                if let Some(name) = child_name(child, source, "name") {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Method,
                        file: file.to_path_buf(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            "public_field_definition" => {
                if let Some(name) = child_name(child, source, "name") {
                    members.push(SymbolInfo {
                        name_path: make_name_path(prefix, &name),
                        name,
                        kind: SymbolKind::Property,
                        file: file.to_path_buf(),
                        start_line: child.start_position().row as u32,
                        end_line: child.end_position().row as u32,
                        start_col: child.start_position().column as u32,
                        children: vec![],
                    });
                }
            }
            _ => {}
        }
    }

    members
}

// ---------------------------------------------------------------------------
// Docstring extraction — Rust
// ---------------------------------------------------------------------------

fn extract_rust_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    let mut skip_until = 0usize;

    for (i, child) in children.iter().enumerate() {
        if i < skip_until {
            continue;
        }
        // Collect consecutive doc comments (/// or //!)
        if child.kind() == "line_comment" {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            if text.starts_with("///") || text.starts_with("//!") {
                // Collect all consecutive doc comments
                let start_line = child.start_position().row as u32;
                let mut end_line = child.end_position().row as u32;
                let mut lines = vec![strip_rust_doc_comment(text)];

                let mut j = i + 1;
                while j < children.len() {
                    let next = &children[j];
                    if next.kind() == "line_comment" {
                        let next_text = next.utf8_text(source.as_bytes()).unwrap_or("");
                        if next_text.starts_with("///") || next_text.starts_with("//!") {
                            lines.push(strip_rust_doc_comment(next_text));
                            end_line = next.end_position().row as u32;
                            j += 1;
                            continue;
                        }
                    }
                    break;
                }
                skip_until = j;

                // Find the next sibling that's a declaration
                let symbol_name = children.get(j).and_then(|next| match next.kind() {
                    "function_item" | "struct_item" | "enum_item" | "trait_item" | "mod_item"
                    | "const_item" | "static_item" | "type_item" | "impl_item" => {
                        child_name(*next, source, "name")
                    }
                    _ => None,
                });

                docs.push(DocstringInfo {
                    symbol_name,
                    content: lines.join("\n"),
                    start_line,
                    end_line,
                });
            }
        }
    }

    docs
}

fn strip_rust_doc_comment(line: &str) -> String {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("///") {
        rest.strip_prefix(' ').unwrap_or(rest).to_string()
    } else if let Some(rest) = trimmed.strip_prefix("//!") {
        rest.strip_prefix(' ').unwrap_or(rest).to_string()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction — Python
// ---------------------------------------------------------------------------

/// Extract a docstring from the first statement in a Python body/module block.
fn extract_python_body_docstring(
    body: Node,
    source: &str,
    symbol_name: Option<String>,
    docs: &mut Vec<DocstringInfo>,
) {
    let mut body_cursor = body.walk();
    let first_stmt = body.children(&mut body_cursor).find(|c| c.is_named());
    let first_stmt = match first_stmt {
        Some(s) if s.kind() == "expression_statement" => s,
        _ => return,
    };
    let mut stmt_cursor = first_stmt.walk();
    let string_node = first_stmt
        .children(&mut stmt_cursor)
        .find(|c| c.kind() == "string");
    if let Some(string_node) = string_node {
        let content =
            strip_python_docstring(string_node.utf8_text(source.as_bytes()).unwrap_or(""));
        docs.push(DocstringInfo {
            symbol_name,
            content,
            start_line: string_node.start_position().row as u32,
            end_line: string_node.end_position().row as u32,
        });
    }
}

fn extract_python_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    collect_python_docstrings(node, source, &mut docs);
    docs
}

fn collect_python_docstrings(node: Node, source: &str, docs: &mut Vec<DocstringInfo>) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "class_definition" => {
                let name = child_name(child, source, "name");
                // Look for docstring: first statement in body is expression_statement > string
                if let Some(body) = find_child_by_kind(child, "block") {
                    extract_python_body_docstring(body, source, name.clone(), docs);
                    // Recurse into the body for nested definitions
                    collect_python_docstrings(body, source, docs);
                }
            }
            "decorated_definition" => {
                collect_python_docstrings(child, source, docs);
            }
            "module" => {
                extract_python_body_docstring(child, source, None, docs);
                collect_python_docstrings(child, source, docs);
            }
            _ => {}
        }
    }
}

fn strip_python_docstring(s: &str) -> String {
    let s = s.trim();
    // Strip triple quotes
    if let Some(inner) = s
        .strip_prefix("\"\"\"")
        .and_then(|s| s.strip_suffix("\"\"\""))
    {
        inner.trim().to_string()
    } else if let Some(inner) = s.strip_prefix("'''").and_then(|s| s.strip_suffix("'''")) {
        inner.trim().to_string()
    } else if let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        inner.to_string()
    } else if let Some(inner) = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        inner.to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction — Go
// ---------------------------------------------------------------------------

fn extract_go_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    for (i, child) in children.iter().enumerate() {
        if child.kind() == "comment" {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            let start_line = child.start_position().row as u32;
            let end_line = child.end_position().row as u32;

            // Find the next non-comment sibling
            let symbol_name = children.get(i + 1).and_then(|next| match next.kind() {
                "function_declaration" | "method_declaration" => child_name(*next, source, "name"),
                "type_declaration" => find_child_by_kind(*next, "type_spec")
                    .and_then(|spec| child_name(spec, source, "name")),
                _ => None,
            });

            let content = strip_go_comment(text);
            docs.push(DocstringInfo {
                symbol_name,
                content,
                start_line,
                end_line,
            });
        }
    }

    docs
}

fn strip_go_comment(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("//") {
        rest.strip_prefix(' ').unwrap_or(rest).to_string()
    } else if let Some(inner) = s.strip_prefix("/*").and_then(|s| s.strip_suffix("*/")) {
        inner.trim().to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction — TypeScript / JavaScript
// ---------------------------------------------------------------------------

fn extract_ts_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    for (i, child) in children.iter().enumerate() {
        if child.kind() == "comment" {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            // Only extract JSDoc-style comments (/** ... */)
            if !text.starts_with("/**") {
                continue;
            }
            let start_line = child.start_position().row as u32;
            let end_line = child.end_position().row as u32;

            let symbol_name = children.get(i + 1).and_then(|next| {
                match next.kind() {
                    "function_declaration"
                    | "class_declaration"
                    | "interface_declaration"
                    | "enum_declaration"
                    | "type_alias_declaration" => child_name(*next, source, "name"),
                    "export_statement" => {
                        // export function X ...
                        let mut c = next.walk();
                        let found = next.children(&mut c).find(|n| {
                            matches!(
                                n.kind(),
                                "function_declaration"
                                    | "class_declaration"
                                    | "interface_declaration"
                            )
                        });
                        found.and_then(|n| child_name(n, source, "name"))
                    }
                    _ => None,
                }
            });

            let content = strip_jsdoc(text);
            docs.push(DocstringInfo {
                symbol_name,
                content,
                start_line,
                end_line,
            });
        }
    }

    docs
}

fn strip_jsdoc(s: &str) -> String {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("/**").and_then(|s| s.strip_suffix("*/")) {
        // Clean each line: strip leading * and whitespace
        inner
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix("* ")
                    .or_else(|| trimmed.strip_prefix('*'))
                    .unwrap_or(trimmed)
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction — Java (Javadoc)
// ---------------------------------------------------------------------------

fn extract_java_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    collect_java_docstrings(node, source, &mut docs);
    docs
}

fn collect_java_docstrings(node: Node, source: &str, docs: &mut Vec<DocstringInfo>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    for (i, child) in children.iter().enumerate() {
        // Java uses block_comment for Javadoc (/** ... */)
        if child.kind() == "block_comment" {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            if !text.starts_with("/**") {
                continue;
            }
            let start_line = child.start_position().row as u32;
            let end_line = child.end_position().row as u32;

            let symbol_name = children.get(i + 1).and_then(|next| match next.kind() {
                "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration" => child_name(*next, source, "name"),
                "method_declaration" | "constructor_declaration" => {
                    child_name(*next, source, "name")
                }
                _ => None,
            });

            let content = strip_jsdoc(text);
            docs.push(DocstringInfo {
                symbol_name,
                content,
                start_line,
                end_line,
            });
        }

        // Recurse into class/interface/enum bodies for inner Javadoc
        match child.kind() {
            "class_declaration" => {
                if let Some(body) = find_child_by_kind(*child, "class_body") {
                    collect_java_docstrings(body, source, docs);
                }
            }
            "interface_declaration" => {
                if let Some(body) = find_child_by_kind(*child, "interface_body") {
                    collect_java_docstrings(body, source, docs);
                }
            }
            "enum_declaration" => {
                if let Some(body) = find_child_by_kind(*child, "enum_body") {
                    collect_java_docstrings(body, source, docs);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Docstring extraction — Kotlin (KDoc)
// ---------------------------------------------------------------------------

fn extract_kotlin_docstrings(node: Node, source: &str) -> Vec<DocstringInfo> {
    let mut docs = Vec::new();
    collect_kotlin_docstrings(node, source, &mut docs);
    docs
}

fn collect_kotlin_docstrings(node: Node, source: &str, docs: &mut Vec<DocstringInfo>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    for (i, child) in children.iter().enumerate() {
        // Kotlin uses block_comment for KDoc (/** ... */)
        if child.kind() == "block_comment" {
            let text = child.utf8_text(source.as_bytes()).unwrap_or("");
            if !text.starts_with("/**") {
                continue;
            }
            let start_line = child.start_position().row as u32;
            let end_line = child.end_position().row as u32;

            let symbol_name = children.get(i + 1).and_then(|next| match next.kind() {
                "class_declaration" | "object_declaration" => child_name(*next, source, "name"),
                "function_declaration" => child_name(*next, source, "name"),
                "property_declaration" => extract_kotlin_property_name(*next, source),
                _ => None,
            });

            let content = strip_jsdoc(text);
            docs.push(DocstringInfo {
                symbol_name,
                content,
                start_line,
                end_line,
            });
        }

        // Recurse into class/object bodies
        match child.kind() {
            "class_declaration" => {
                let body = find_child_by_kind(*child, "class_body")
                    .or_else(|| find_child_by_kind(*child, "enum_class_body"));
                if let Some(body) = body {
                    collect_kotlin_docstrings(body, source, docs);
                }
            }
            "object_declaration" | "companion_object" => {
                if let Some(body) = find_child_by_kind(*child, "class_body") {
                    collect_kotlin_docstrings(body, source, docs);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rust_symbols() {
        let source = r#"
fn hello() {}

struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn distance(&self) -> f64 {
        0.0
    }
    pub fn origin() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

enum Color {
    Red,
    Green,
    Blue,
}

trait Drawable {
    fn draw(&self);
}

const MAX: u32 = 100;

mod utils {
    pub fn helper() {}
}
"#;
        let syms = extract_symbols_from_source(source, Some("rust"), Path::new("test.rs")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"), "missing hello: {:?}", names);
        assert!(names.contains(&"Point"), "missing Point: {:?}", names);
        assert!(names.contains(&"Color"), "missing Color: {:?}", names);
        assert!(names.contains(&"Drawable"), "missing Drawable: {:?}", names);
        assert!(names.contains(&"MAX"), "missing MAX: {:?}", names);
        assert!(names.contains(&"utils"), "missing utils: {:?}", names);

        // impl methods should appear as top-level symbols with Point/ prefix
        assert!(
            names.contains(&"distance"),
            "missing distance method: {:?}",
            names
        );
        assert!(
            names.contains(&"origin"),
            "missing origin method: {:?}",
            names
        );

        // Check name_path for impl methods
        let distance = syms.iter().find(|s| s.name == "distance").unwrap();
        assert_eq!(distance.name_path, "Point/distance");
        assert_eq!(distance.kind, SymbolKind::Method);

        // Enum variants should be children
        let color = syms.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.children.len(), 3);
        assert_eq!(color.children[0].name, "Red");

        // Trait methods should be children
        let drawable = syms.iter().find(|s| s.name == "Drawable").unwrap();
        assert_eq!(drawable.children.len(), 1);
        assert_eq!(drawable.children[0].name, "draw");

        // Module children
        let utils = syms.iter().find(|s| s.name == "utils").unwrap();
        assert_eq!(utils.children.len(), 1);
        assert_eq!(utils.children[0].name_path, "utils/helper");
    }

    #[test]
    fn python_symbols() {
        let source = r#"
def greet(name):
    """Say hello."""
    print(f"Hello, {name}")

class Animal:
    def __init__(self, name):
        self.name = name

    def speak(self):
        pass

class Dog(Animal):
    def speak(self):
        return "Woof!"
"#;
        let syms =
            extract_symbols_from_source(source, Some("python"), Path::new("test.py")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "missing greet: {:?}", names);
        assert!(names.contains(&"Animal"), "missing Animal: {:?}", names);
        assert!(names.contains(&"Dog"), "missing Dog: {:?}", names);

        let animal = syms.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, SymbolKind::Class);
        let method_names: Vec<&str> = animal.children.iter().map(|s| s.name.as_str()).collect();
        assert!(
            method_names.contains(&"__init__"),
            "missing __init__: {:?}",
            method_names
        );
        assert!(
            method_names.contains(&"speak"),
            "missing speak: {:?}",
            method_names
        );
        assert_eq!(animal.children[0].kind, SymbolKind::Method);
    }

    #[test]
    fn go_symbols() {
        let source = r#"
package main

func main() {
    fmt.Println("Hello")
}

type Point struct {
    X float64
    Y float64
}

func (p *Point) Distance() float64 {
    return 0.0
}

type Reader interface {
    Read(p []byte) (n int, err error)
}
"#;
        let syms = extract_symbols_from_source(source, Some("go"), Path::new("test.go")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"main"), "missing main: {:?}", names);
        assert!(names.contains(&"Point"), "missing Point: {:?}", names);
        assert!(
            names.contains(&"Distance"),
            "missing Distance method: {:?}",
            names
        );
        assert!(names.contains(&"Reader"), "missing Reader: {:?}", names);

        let point = syms.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Struct);
        assert_eq!(point.children.len(), 2); // X, Y fields

        let distance = syms.iter().find(|s| s.name == "Distance").unwrap();
        assert_eq!(distance.kind, SymbolKind::Method);
        assert_eq!(distance.name_path, "Point/Distance");

        let reader = syms.iter().find(|s| s.name == "Reader").unwrap();
        assert_eq!(reader.kind, SymbolKind::Interface);
    }

    #[test]
    fn typescript_symbols() {
        let source = r#"
function greet(name: string): string {
    return `Hello, ${name}`;
}

class Animal {
    name: string;
    constructor(name: string) {
        this.name = name;
    }
    speak(): string {
        return "";
    }
}

interface Drawable {
    draw(): void;
}

enum Direction {
    Up,
    Down,
    Left,
    Right,
}

type ID = string | number;
"#;
        let syms =
            extract_symbols_from_source(source, Some("typescript"), Path::new("test.ts")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "missing greet: {:?}", names);
        assert!(names.contains(&"Animal"), "missing Animal: {:?}", names);
        assert!(names.contains(&"Drawable"), "missing Drawable: {:?}", names);
        assert!(
            names.contains(&"Direction"),
            "missing Direction: {:?}",
            names
        );
        assert!(names.contains(&"ID"), "missing ID type alias: {:?}", names);

        let animal = syms.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, SymbolKind::Class);
        let member_names: Vec<&str> = animal.children.iter().map(|s| s.name.as_str()).collect();
        assert!(
            member_names.contains(&"constructor"),
            "missing constructor: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"speak"),
            "missing speak: {:?}",
            member_names
        );
    }

    #[test]
    fn rust_docstrings() {
        let source = r#"
/// A greeting function.
/// Returns a friendly message.
fn hello() {}

/// A point in 2D space.
struct Point {
    x: f64,
}
"#;
        let docs =
            extract_docstrings_from_source(source, Some("rust"), Path::new("test.rs")).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].symbol_name.as_deref(), Some("hello"));
        assert!(docs[0].content.contains("greeting function"));
        assert!(docs[0].content.contains("friendly message"));
        assert_eq!(docs[1].symbol_name.as_deref(), Some("Point"));
    }

    #[test]
    fn python_docstrings() {
        let source = r#"
def greet(name):
    """Say hello to someone.

    Args:
        name: The person's name.
    """
    print(f"Hello, {name}")

class Animal:
    """An animal base class."""
    def speak(self):
        """Make a sound."""
        pass
"#;
        let docs =
            extract_docstrings_from_source(source, Some("python"), Path::new("test.py")).unwrap();
        assert!(
            docs.len() >= 2,
            "expected at least 2 docstrings, got {}",
            docs.len()
        );
        let greet_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("greet"))
            .unwrap();
        assert!(greet_doc.content.contains("Say hello"));
        let animal_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("Animal"))
            .unwrap();
        assert!(animal_doc.content.contains("animal base class"));
    }

    #[test]
    fn java_symbols() {
        let source = r#"
package com.example;

public class Calculator {
    private int value;

    public Calculator(int initial) {
        this.value = initial;
    }

    public int add(int x) {
        return value + x;
    }

    public static void main(String[] args) {
        System.out.println("Hello");
    }
}

interface Computable {
    int compute(int x);
}

enum Color {
    RED,
    GREEN,
    BLUE;
}

record Point(int x, int y) {}
"#;
        let syms = extract_symbols_from_source(source, Some("java"), Path::new("Calculator.java"))
            .unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Calculator"),
            "missing Calculator: {:?}",
            names
        );
        assert!(
            names.contains(&"Computable"),
            "missing Computable: {:?}",
            names
        );
        assert!(names.contains(&"Color"), "missing Color: {:?}", names);
        assert!(names.contains(&"Point"), "missing Point: {:?}", names);

        // Class members
        let calc = syms.iter().find(|s| s.name == "Calculator").unwrap();
        assert_eq!(calc.kind, SymbolKind::Class);
        let member_names: Vec<&str> = calc.children.iter().map(|s| s.name.as_str()).collect();
        assert!(
            member_names.contains(&"add"),
            "missing add method: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"main"),
            "missing main method: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"Calculator"),
            "missing constructor: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"value"),
            "missing field: {:?}",
            member_names
        );

        // Enum constants
        let color = syms.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
        assert_eq!(color.children.len(), 3);
        assert_eq!(color.children[0].name, "RED");

        // Interface
        let comp = syms.iter().find(|s| s.name == "Computable").unwrap();
        assert_eq!(comp.kind, SymbolKind::Interface);

        // Record
        let point = syms.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Struct);
    }

    #[test]
    fn java_docstrings() {
        let source = r#"
package com.example;

/**
 * A calculator class.
 * @author Test
 */
public class Calculator {
    /** Add two numbers. */
    public int add(int a, int b) {
        return a + b;
    }
}
"#;
        let docs =
            extract_docstrings_from_source(source, Some("java"), Path::new("Calculator.java"))
                .unwrap();
        assert!(
            docs.len() >= 2,
            "expected at least 2 Javadoc comments, got {:?}",
            docs
        );
        let class_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("Calculator"))
            .unwrap();
        assert!(class_doc.content.contains("calculator class"));
        let method_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("add"))
            .unwrap();
        assert!(method_doc.content.contains("Add two numbers"));
    }

    #[test]
    fn kotlin_symbols() {
        let source = r#"
package com.example

fun greet(name: String): String {
    return "Hello, $name"
}

class Animal(val name: String) {
    var sound: String = ""

    fun speak(): String {
        return sound
    }

    companion object {
        fun create(name: String): Animal = Animal(name)
    }
}

object Singleton {
    fun doSomething() {}
}

enum class Direction {
    NORTH,
    SOUTH,
    EAST,
    WEST
}

interface Drawable {
    fun draw()
}

val PI = 3.14159
"#;
        let syms =
            extract_symbols_from_source(source, Some("kotlin"), Path::new("main.kt")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "missing greet: {:?}", names);
        assert!(names.contains(&"Animal"), "missing Animal: {:?}", names);
        assert!(
            names.contains(&"Singleton"),
            "missing Singleton: {:?}",
            names
        );
        assert!(
            names.contains(&"Direction"),
            "missing Direction: {:?}",
            names
        );
        assert!(names.contains(&"Drawable"), "missing Drawable: {:?}", names);
        assert!(names.contains(&"PI"), "missing PI property: {:?}", names);

        // Class members
        let animal = syms.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, SymbolKind::Class);
        let member_names: Vec<&str> = animal.children.iter().map(|s| s.name.as_str()).collect();
        assert!(
            member_names.contains(&"speak"),
            "missing speak: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"sound"),
            "missing sound property: {:?}",
            member_names
        );
        assert!(
            member_names.contains(&"Companion"),
            "missing companion: {:?}",
            member_names
        );

        // Companion object members
        let companion = animal
            .children
            .iter()
            .find(|s| s.name == "Companion")
            .unwrap();
        assert_eq!(companion.children.len(), 1);
        assert_eq!(companion.children[0].name, "create");

        // Enum class members
        let direction = syms.iter().find(|s| s.name == "Direction").unwrap();
        assert_eq!(direction.kind, SymbolKind::Enum);
        assert_eq!(direction.children.len(), 4);
        assert_eq!(direction.children[0].name, "NORTH");

        // Interface
        let drawable = syms.iter().find(|s| s.name == "Drawable").unwrap();
        assert_eq!(drawable.kind, SymbolKind::Interface);
    }

    #[test]
    fn kotlin_docstrings() {
        let source = r#"
package com.example

/**
 * Greet someone by name.
 * @param name the person's name
 */
fun greet(name: String): String {
    return "Hello, $name"
}

/**
 * An animal class.
 */
class Animal(val name: String) {
    /** Make a sound. */
    fun speak() {}
}
"#;
        let docs =
            extract_docstrings_from_source(source, Some("kotlin"), Path::new("main.kt")).unwrap();
        assert!(
            docs.len() >= 2,
            "expected at least 2 KDoc comments, got {:?}",
            docs
        );
        let greet_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("greet"))
            .unwrap();
        assert!(greet_doc.content.contains("Greet someone"));
        let animal_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("Animal"))
            .unwrap();
        assert!(animal_doc.content.contains("animal class"));
    }

    #[test]
    fn tsx_symbols() {
        let source = r#"
import React from 'react';

interface Props {
    name: string;
    count: number;
}

function Greeting({ name }: Props): JSX.Element {
    return <div>Hello, {name}!</div>;
}

class Counter extends React.Component<Props> {
    render() {
        return <span>{this.props.count}</span>;
    }
}

export default Greeting;
"#;
        let syms = extract_symbols_from_source(source, Some("tsx"), Path::new("App.tsx")).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Props"),
            "missing Props interface: {:?}",
            names
        );
        assert!(
            names.contains(&"Greeting"),
            "missing Greeting function: {:?}",
            names
        );
        assert!(
            names.contains(&"Counter"),
            "missing Counter class: {:?}",
            names
        );

        let counter = syms.iter().find(|s| s.name == "Counter").unwrap();
        assert_eq!(counter.kind, SymbolKind::Class);
        let member_names: Vec<&str> = counter.children.iter().map(|s| s.name.as_str()).collect();
        assert!(
            member_names.contains(&"render"),
            "missing render method: {:?}",
            member_names
        );
    }

    #[test]
    fn unsupported_language() {
        let result = extract_symbols_from_source("code", None, Path::new("test.txt"));
        assert!(result.is_err());
        let result = extract_symbols_from_source("code", Some("haskell"), Path::new("test.hs"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_source() {
        let syms = extract_symbols_from_source("", Some("rust"), Path::new("empty.rs")).unwrap();
        assert!(syms.is_empty());
    }

    #[test]
    fn go_docstrings() {
        let source = r#"
package main

// Hello prints a greeting.
func Hello() {}

// Point represents a 2D point.
type Point struct {
    X float64
}
"#;
        let docs =
            extract_docstrings_from_source(source, Some("go"), Path::new("test.go")).unwrap();
        assert!(docs.len() >= 2, "expected at least 2 docs, got {:?}", docs);
        let hello_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("Hello"))
            .unwrap();
        assert!(hello_doc.content.contains("greeting"));
    }

    #[test]
    fn typescript_jsdoc() {
        let source = r#"
/** Greet someone by name. */
function greet(name: string): string {
    return `Hello, ${name}`;
}

/**
 * An animal class.
 * @param name - The animal's name
 */
class Animal {
    constructor(name: string) {}
}
"#;
        let docs = extract_docstrings_from_source(source, Some("typescript"), Path::new("test.ts"))
            .unwrap();
        assert!(
            docs.len() >= 2,
            "expected at least 2 JSDoc comments, got {:?}",
            docs
        );
        let greet_doc = docs
            .iter()
            .find(|d| d.symbol_name.as_deref() == Some("greet"))
            .unwrap();
        assert!(greet_doc.content.contains("Greet someone"));
    }
}
