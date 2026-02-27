//! Prompt templates for LLM guidance.
//!
//! Templates are stored as markdown files and compiled into the binary
//! via `include_str!`. Dynamic sections are appended at runtime based
//! on project state.

/// Static server instructions — tool reference, workflow patterns, steering rules.
pub const SERVER_INSTRUCTIONS: &str = include_str!("server_instructions.md");

/// Build the full server instructions string, optionally appending
/// dynamic project status.
pub fn build_server_instructions(project_status: Option<&ProjectStatus>) -> String {
    let mut instructions = SERVER_INSTRUCTIONS.to_string();

    if let Some(status) = project_status {
        instructions.push_str("\n\n## Project Status\n\n");
        instructions.push_str(&format!(
            "- **Project:** {} at `{}`\n",
            status.name, status.path
        ));
        if !status.languages.is_empty() {
            instructions.push_str(&format!(
                "- **Languages:** {}\n",
                status.languages.join(", ")
            ));
        }
        if !status.memories.is_empty() {
            instructions.push_str(&format!(
                "- **Available memories:** {} — use `read_memory(topic)` to read relevant ones as needed for your current task\n",
                status.memories.join(", ")
            ));
        } else {
            instructions.push_str(
                "- **Memories:** None yet — run `onboarding` to create project memories\n",
            );
        }
        if status.has_index {
            instructions
                .push_str("- **Semantic index:** Built — `semantic_search` is ready to use\n");
        } else {
            instructions.push_str(
                "- **Semantic index:** Not built — run `index_project` to enable `semantic_search`\n",
            );
        }

        if let Some(prompt) = &status.system_prompt {
            instructions.push_str("\n\n## Custom Instructions\n\n");
            instructions.push_str(prompt);
            instructions.push('\n');
        }
    }

    instructions
}

/// Dynamic project status used to build server instructions.
#[derive(Debug)]
pub struct ProjectStatus {
    pub name: String,
    pub path: String,
    pub languages: Vec<String>,
    pub memories: Vec<String>,
    pub has_index: bool,
    pub system_prompt: Option<String>,
}

/// Onboarding prompt template — instructs Claude what to explore and what memories to create.
pub const ONBOARDING_PROMPT: &str = include_str!("onboarding_prompt.md");

/// Build the onboarding prompt, substituting detected project information.
#[allow(clippy::too_many_arguments)]
pub fn build_onboarding_prompt(
    languages: &[String],
    top_level: &[String],
    readme: Option<&str>,
    build_file: Option<(&str, &str)>, // (name, content)
    claude_md: Option<&str>,
    ci_files: &[String],
    entry_points: &[String],
    test_dirs: &[String],
) -> String {
    let mut prompt = ONBOARDING_PROMPT.to_string();

    // Append gathered data after the "Gathered Project Data" section header
    prompt.push_str("\n\n---\n\n");

    if !languages.is_empty() {
        prompt.push_str(&format!(
            "**Detected languages:** {}\n\n",
            languages.join(", ")
        ));
    }

    if !top_level.is_empty() {
        prompt.push_str(&format!(
            "**Top-level structure:**\n```\n{}\n```\n\n",
            top_level.join("\n")
        ));
    }

    if !entry_points.is_empty() {
        prompt.push_str(&format!(
            "**Entry points found:** {}\n\n",
            entry_points.join(", ")
        ));
    }

    if !test_dirs.is_empty() {
        prompt.push_str(&format!(
            "**Test directories:** {}\n\n",
            test_dirs.join(", ")
        ));
    }

    if !ci_files.is_empty() {
        prompt.push_str(&format!("**CI config files:** {}\n\n", ci_files.join(", ")));
    }

    if let Some(content) = readme {
        prompt.push_str(&format!("**README.md:**\n```\n{}\n```\n\n", content));
    }

    if let Some((name, content)) = build_file {
        prompt.push_str(&format!(
            "**Build file (`{}`):**\n```\n{}\n```\n\n",
            name, content
        ));
    }

    if let Some(content) = claude_md {
        prompt.push_str(&format!(
            "**CLAUDE.md (loaded every session — do NOT duplicate this in memories):**\n```\n{}\n```\n\n",
            content
        ));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_instructions_contain_key_sections() {
        assert!(SERVER_INSTRUCTIONS.contains("## How to Choose the Right Tool"));
        assert!(SERVER_INSTRUCTIONS.contains("## Output Modes"));
        assert!(SERVER_INSTRUCTIONS.contains("## Rules"));
    }

    #[test]
    fn build_without_project_returns_static() {
        let result = build_server_instructions(None);
        assert_eq!(result, SERVER_INSTRUCTIONS);
        assert!(!result.contains("## Project Status"));
    }

    #[test]
    fn build_with_project_appends_status() {
        let status = ProjectStatus {
            name: "my-project".into(),
            path: "/home/user/my-project".into(),
            languages: vec!["rust".into(), "python".into()],
            memories: vec!["architecture".into(), "conventions".into()],
            has_index: true,
            system_prompt: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("## Project Status"));
        assert!(result.contains("my-project"));
        assert!(result.contains("rust, python"));
        assert!(result.contains("architecture, conventions"));
        assert!(result.contains("Semantic index:** Built"));
    }

    #[test]
    fn build_with_no_memories_suggests_onboarding() {
        let status = ProjectStatus {
            name: "new-project".into(),
            path: "/tmp/new".into(),
            languages: vec![],
            memories: vec![],
            has_index: false,
            system_prompt: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("run `onboarding`"));
        assert!(result.contains("run `index_project`"));
    }

    #[test]
    fn onboarding_prompt_contains_key_sections() {
        assert!(ONBOARDING_PROMPT.contains("## Rules"));
        assert!(ONBOARDING_PROMPT.contains("## Memories to Create"));
        assert!(ONBOARDING_PROMPT.contains("project-overview"));
        assert!(ONBOARDING_PROMPT.contains("architecture"));
        assert!(ONBOARDING_PROMPT.contains("conventions"));
        assert!(ONBOARDING_PROMPT.contains("development-commands"));
        assert!(ONBOARDING_PROMPT.contains("domain-glossary"));
        assert!(ONBOARDING_PROMPT.contains("gotchas"));
        assert!(ONBOARDING_PROMPT.contains("## Gathered Project Data"));
    }

    #[test]
    fn build_onboarding_includes_languages() {
        let result = build_onboarding_prompt(
            &["rust".into(), "python".into()],
            &["src/".into(), "tests/".into()],
            None,
            None,
            None,
            &[],
            &[],
            &[],
        );
        assert!(result.contains("rust, python"));
        assert!(result.contains("src/"));
    }

    #[test]
    fn build_onboarding_handles_empty() {
        let result = build_onboarding_prompt(&[], &[], None, None, None, &[], &[], &[]);
        assert!(result.contains("## Rules"));
        assert!(!result.contains("Detected languages"));
    }

    #[test]
    fn build_onboarding_includes_gathered_context() {
        let result = build_onboarding_prompt(
            &["rust".into(), "python".into()],
            &["src/".into(), "tests/".into()],
            Some("# My Project\nA cool thing."),
            Some(("Cargo.toml", "[package]\nname = \"cool\"")),
            Some("# CLAUDE.md\nDev commands here."),
            &[".github/workflows/ci.yml".into()],
            &["src/main.rs".into()],
            &["tests".into()],
        );
        assert!(result.contains("# My Project"));
        assert!(result.contains("Cargo.toml"));
        assert!(result.contains("ci.yml"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("Detected languages"));
        assert!(result.contains("CLAUDE.md (loaded every session"));
    }

    #[test]
    fn build_with_system_prompt_appends_custom_section() {
        let status = ProjectStatus {
            name: "my-project".into(),
            path: "/tmp/my-project".into(),
            languages: vec![],
            memories: vec![],
            has_index: false,
            system_prompt: Some("Always use pytest.".into()),
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("## Custom Instructions"));
        assert!(result.contains("Always use pytest."));
        // Custom instructions should come after project status
        let status_pos = result.find("## Project Status").unwrap();
        let custom_pos = result.find("## Custom Instructions").unwrap();
        assert!(custom_pos > status_pos);
    }

    #[test]
    fn build_without_system_prompt_has_no_custom_section() {
        let status = ProjectStatus {
            name: "my-project".into(),
            path: "/tmp/my-project".into(),
            languages: vec![],
            memories: vec![],
            has_index: false,
            system_prompt: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(!result.contains("## Custom Instructions"));
    }
}
