use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// A single test expectation for one language.
#[derive(Debug, Clone, Deserialize)]
pub struct LangExpectation {
    /// File or directory path relative to the fixture root.
    #[serde(alias = "file")]
    pub path: Option<String>,
    /// For find_symbol: the symbol name to search for.
    pub symbol: Option<String>,
    /// Expected symbol names in the output.
    pub contains_symbols: Option<Vec<String>>,
    /// Expected substrings in the symbol body (requires include_body=true).
    pub body_contains: Option<Vec<String>>,
    /// Expected function names (for list_functions).
    pub contains_functions: Option<Vec<String>>,
    /// Regex pattern (for search_for_pattern).
    pub pattern: Option<String>,
    /// Expected file names in search results.
    pub expected_files: Option<Vec<String>>,
    /// Expected references (for find_referencing_symbols).
    pub expected_refs_contain: Option<Vec<String>>,
}

/// Load expectations from a TOML file, filtered for a specific language.
/// For core-expectations.toml, extracts the language-specific sub-tables.
/// For <lang>-extensions.toml, each section IS the expectation (no language sub-tables).
pub fn load_expectations(
    toml_path: &Path,
    language: &str,
) -> Vec<(String, LangExpectation, String)> {
    let content = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", toml_path.display()));
    let raw: HashMap<String, toml::Value> = toml::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", toml_path.display()));

    let mut expectations = Vec::new();

    for (test_name, value) in &raw {
        let table = value.as_table().expect("Each section should be a table");
        let description = table
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let _ = description; // used for logging if needed
        let tool = table
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("get_symbols_overview")
            .to_string();

        // Check if this section has a language-specific sub-table
        if let Some(lang_value) = table.get(language) {
            // Core expectations: language sub-table
            if let Ok(lang_exp) = lang_value.clone().try_into::<LangExpectation>() {
                expectations.push((test_name.clone(), lang_exp, tool.clone()));
            }
        } else if table.get("path").is_some() || table.get("file").is_some() {
            // Extension expectations: top-level path/symbol (single-language)
            if let Ok(lang_exp) = value.clone().try_into::<LangExpectation>() {
                expectations.push((test_name.clone(), lang_exp, tool.clone()));
            }
        }
    }

    expectations
}
