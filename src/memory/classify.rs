/// Classify memory content into a semantic bucket based on keyword heuristics.
///
/// Returns one of: `"code"`, `"system"`, `"preferences"`, or `"unstructured"`.
pub fn classify_bucket(content: &str) -> &'static str {
    let lower = content.to_lowercase();

    let code_keywords = [
        "function",
        "method",
        "struct",
        "class",
        "trait",
        "impl",
        "pattern",
        "api",
        "endpoint",
        "convention",
        "naming",
        "import",
        "module",
        "crate",
        "package",
        "type",
        "interface",
        "refactor",
        "abstraction",
        "generic",
        "lifetime",
        "async",
        "iterator",
        "closure",
        "macro",
        "enum",
        "variant",
    ];
    let system_keywords = [
        "build",
        "deploy",
        "ci",
        "config",
        "environment",
        "docker",
        "infra",
        "database",
        "migration",
        "permission",
        "secret",
        "credential",
        "server",
        "port",
        "host",
        "pipeline",
        "cargo test",
        "npm",
        "pip",
        "github actions",
        "dockerfile",
        "kubernetes",
        "nginx",
        "ssl",
        "certificate",
    ];
    let preferences_keywords = [
        "prefer",
        "always",
        "never",
        "style",
        "habit",
        "default to",
        "next time",
        "remember to",
        "i like",
        "i want",
        "don't use",
        "snake_case",
        "camelcase",
        "tabs",
        "spaces",
        "indentation",
    ];

    // File path heuristic: language-specific extensions
    let has_code_path = lower.contains(".rs")
        || lower.contains(".ts")
        || lower.contains(".py")
        || lower.contains(".go")
        || lower.contains(".java")
        || lower.contains(".js")
        || lower.contains(".kt");

    let code_score: usize = code_keywords.iter().filter(|k| lower.contains(*k)).count()
        + if has_code_path { 2 } else { 0 };
    let system_score: usize = system_keywords
        .iter()
        .filter(|k| lower.contains(*k))
        .count();
    let preferences_score: usize = preferences_keywords
        .iter()
        .filter(|k| lower.contains(*k))
        .count();

    if code_score == 0 && system_score == 0 && preferences_score == 0 {
        return "unstructured";
    }

    let max = code_score.max(system_score).max(preferences_score);
    if max == preferences_score && preferences_score > 0 {
        "preferences"
    } else if code_score >= system_score {
        "code"
    } else {
        "system"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_code_content() {
        assert_eq!(
            classify_bucket("The auth module uses the builder pattern for request construction"),
            "code"
        );
    }

    #[test]
    fn classifies_system_content() {
        assert_eq!(
            classify_bucket("CI pipeline requires docker and uses GitHub Actions for deployment"),
            "system"
        );
    }

    #[test]
    fn classifies_unstructured_by_default() {
        assert_eq!(classify_bucket("The weather is nice today"), "unstructured");
    }

    #[test]
    fn classifies_preferences_content() {
        assert_eq!(
            classify_bucket("I prefer snake_case for all variable names, always use it"),
            "preferences"
        );
    }

    #[test]
    fn code_keywords_beat_system_when_mixed() {
        assert_eq!(
            classify_bucket("The function uses a config struct pattern with builder methods"),
            "code"
        );
    }

    #[test]
    fn file_paths_trigger_code() {
        assert_eq!(
            classify_bucket("Check src/tools/memory.rs for the implementation"),
            "code"
        );
    }

    #[test]
    fn empty_content_returns_unstructured() {
        assert_eq!(classify_bucket(""), "unstructured");
    }

    #[test]
    fn preferences_detected_from_remember_to() {
        assert_eq!(
            classify_bucket("Remember to always run clippy before committing"),
            "preferences"
        );
    }
}
