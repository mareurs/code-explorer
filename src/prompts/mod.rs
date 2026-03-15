//! Prompt templates for LLM guidance.
//!
//! Templates are stored as markdown files and compiled into the binary
//! via `include_str!`. Dynamic sections are appended at runtime based
//! on project state.

/// Static server instructions — tool reference, workflow patterns, steering rules.
pub const SERVER_INSTRUCTIONS: &str = include_str!("server_instructions.md");
pub const GITHUB_INSTRUCTIONS: &str = include_str!("github_instructions.md");

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
                "- **Available shared memories:** {} — use `memory(action=\"read\", topic=...)` to read relevant ones as needed for your current task\n",
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

        // Workspace topology — inject project table when there are sibling projects.
        if let Some(projects) = &status.workspace {
            if !projects.is_empty() {
                instructions.push_str("\n## Workspace Projects\n\n");
                instructions.push_str("| Project | Root | Languages | Depends On |\n");
                instructions.push_str("|---------|------|-----------|------------|\n");
                for p in projects {
                    let langs = if p.languages.is_empty() {
                        "—".to_string()
                    } else {
                        p.languages.join(", ")
                    };
                    let deps = if p.depends_on.is_empty() {
                        "—".to_string()
                    } else {
                        p.depends_on.join(", ")
                    };
                    instructions.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        p.id, p.root, langs, deps
                    ));
                }
                instructions.push_str(
                    "\nUse `project: \"<id>\"` in `find_symbol` / `semantic_search` / `memory` to scope to a specific project.\n",
                );
            }
        }

        if status.github_enabled {
            instructions.push_str("\n\n");
            instructions.push_str(GITHUB_INSTRUCTIONS);
        }

        if let Some(prompt) = &status.system_prompt {
            instructions.push_str("\n\n## Custom Instructions\n\n");
            instructions.push_str(prompt);
            instructions.push('\n');
        }
    }

    instructions
}

/// One row in the workspace project table injected into server instructions.
#[derive(Debug)]
pub struct WorkspaceProjectSummary {
    pub id: String,
    pub root: String,
    pub languages: Vec<String>,
    pub depends_on: Vec<String>,
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
    pub github_enabled: bool,
    /// Other projects in the workspace, if this is a multi-project repo.
    /// None for single-project activations; Some([]) is never emitted.
    pub workspace: Option<Vec<WorkspaceProjectSummary>>,
}

/// Onboarding prompt template — instructs Claude what to explore and what memories to create.
pub const ONBOARDING_PROMPT: &str = include_str!("onboarding_prompt.md");

/// Workspace-specific onboarding prompt — appended when multiple projects are discovered.
pub const WORKSPACE_ONBOARDING_PROMPT: &str = include_str!("workspace_onboarding_prompt.md");

/// Context for building the onboarding prompt.
pub struct OnboardingContext<'a> {
    pub languages: &'a [String],
    pub top_level: &'a [String],
    pub key_files: &'a [String],
    pub ci_files: &'a [String],
    pub entry_points: &'a [String],
    pub test_dirs: &'a [String],
    pub index_ready: bool,
    pub index_files: usize,
    pub index_chunks: usize,
    pub projects: &'a [crate::workspace::DiscoveredProject],
    pub is_workspace: bool,
}

/// Build the onboarding prompt, substituting detected project information.
pub fn build_onboarding_prompt(ctx: &OnboardingContext) -> String {
    let mut prompt = ONBOARDING_PROMPT.to_string();

    prompt.push_str("\n\n---\n\n");

    if !ctx.languages.is_empty() {
        prompt.push_str(&format!(
            "**Detected languages:** {}\n\n",
            ctx.languages.join(", ")
        ));
    }

    if !ctx.top_level.is_empty() {
        prompt.push_str(&format!(
            "**Top-level structure:**\n```\n{}\n```\n\n",
            ctx.top_level.join("\n")
        ));
    }

    if !ctx.entry_points.is_empty() {
        prompt.push_str(&format!(
            "**Entry points found:** {}\n\n",
            ctx.entry_points.join(", ")
        ));
    }

    if !ctx.test_dirs.is_empty() {
        prompt.push_str(&format!(
            "**Test directories:** {}\n\n",
            ctx.test_dirs.join(", ")
        ));
    }

    if !ctx.ci_files.is_empty() {
        prompt.push_str(&format!(
            "**CI config files:** {}\n\n",
            ctx.ci_files.join(", ")
        ));
    }

    if !ctx.key_files.is_empty() {
        prompt.push_str(&format!(
            "**Key files to read during Phase 1:**\n{}\n\n",
            ctx.key_files
                .iter()
                .map(|f| format!("- `{f}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if ctx.index_ready {
        prompt.push_str(&format!(
            "**Semantic index:** ready ({} files, {} chunks)\n\n",
            ctx.index_files, ctx.index_chunks
        ));
    } else {
        prompt.push_str("**Semantic index:** not built\n\n");
    }

    if ctx.is_workspace && ctx.projects.len() > 1 {
        prompt.push_str(&format!(
            "**Workspace mode:** {} projects detected\n\n",
            ctx.projects.len()
        ));
        prompt.push_str(WORKSPACE_ONBOARDING_PROMPT);
        prompt.push_str("\n\n");
        prompt.push_str("**Discovered projects:**\n\n");
        prompt.push_str("| Project | Root | Languages | Build |\n");
        prompt.push_str("|---------|------|-----------|-------|\n");
        for p in ctx.projects {
            prompt.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                p.id,
                p.relative_root.display(),
                p.languages.join(", "),
                p.manifest.as_deref().unwrap_or("-"),
            ));
        }
        prompt.push('\n');
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_instructions_contain_key_sections() {
        assert!(SERVER_INSTRUCTIONS.contains("## How to Choose the Right Tool"));
        assert!(SERVER_INSTRUCTIONS.contains("## Output System"));
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
            github_enabled: false,
            workspace: None,
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
            github_enabled: false,
            workspace: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("run `onboarding`"));
        assert!(result.contains("run `index_project`"));
    }

    #[test]
    fn onboarding_prompt_contains_key_sections() {
        assert!(ONBOARDING_PROMPT.contains("### Rules"));
        assert!(ONBOARDING_PROMPT.contains("### Memories to Create"));
        assert!(ONBOARDING_PROMPT.contains("project-overview"));
        assert!(ONBOARDING_PROMPT.contains("architecture"));
        assert!(ONBOARDING_PROMPT.contains("conventions"));
        assert!(ONBOARDING_PROMPT.contains("development-commands"));
        assert!(ONBOARDING_PROMPT.contains("domain-glossary"));
        assert!(ONBOARDING_PROMPT.contains("gotchas"));
        assert!(ONBOARDING_PROMPT.contains("## Gathered Project Data"));
        // Verify enforcement sections exist
        assert!(ONBOARDING_PROMPT.contains("## Phase 0: Semantic Index Check"));
        assert!(ONBOARDING_PROMPT.contains("## THE IRON LAW"));
        assert!(ONBOARDING_PROMPT.contains("<HARD-GATE>"));
        assert!(ONBOARDING_PROMPT.contains("## Red Flags"));
        assert!(ONBOARDING_PROMPT.contains("## Common Rationalizations"));
    }

    #[test]
    fn workspace_onboarding_prompt_contains_key_sections() {
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("Phase 1A"));
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("Phase 1B"));
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("Phase 2"));
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("Subagent"));
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("HARD-GATE"));
        assert!(WORKSPACE_ONBOARDING_PROMPT.contains("Re-Onboarding"));
    }

    #[test]
    fn build_onboarding_includes_languages() {
        let result = build_onboarding_prompt(&OnboardingContext {
            languages: &["rust".into(), "python".into()],
            top_level: &["src/".into(), "tests/".into()],
            key_files: &[],
            ci_files: &[],
            entry_points: &[],
            test_dirs: &[],
            index_ready: false,
            index_files: 0,
            index_chunks: 0,
            projects: &[],
            is_workspace: false,
        });
        assert!(result.contains("rust, python"));
        assert!(result.contains("src/"));
    }

    #[test]
    fn build_onboarding_handles_empty() {
        let result = build_onboarding_prompt(&OnboardingContext {
            languages: &[],
            top_level: &[],
            key_files: &[],
            ci_files: &[],
            entry_points: &[],
            test_dirs: &[],
            index_ready: false,
            index_files: 0,
            index_chunks: 0,
            projects: &[],
            is_workspace: false,
        });
        assert!(result.contains("## Rules"));
        assert!(!result.contains("Detected languages"));
    }

    #[test]
    fn build_onboarding_includes_gathered_context() {
        let result = build_onboarding_prompt(&OnboardingContext {
            languages: &["rust".into(), "python".into()],
            top_level: &["src/".into(), "tests/".into()],
            key_files: &["README.md".into(), "Cargo.toml".into(), "CLAUDE.md".into()],
            ci_files: &[".github/workflows/ci.yml".into()],
            entry_points: &["src/main.rs".into()],
            test_dirs: &["tests".into()],
            index_ready: false,
            index_files: 0,
            index_chunks: 0,
            projects: &[],
            is_workspace: false,
        });
        assert!(result.contains("Cargo.toml"));
        assert!(result.contains("ci.yml"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("Detected languages"));
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
            github_enabled: false,
            workspace: None,
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
            github_enabled: false,
            workspace: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(!result.contains("## Custom Instructions"));
    }

    #[test]
    fn build_with_github_enabled_appends_github_instructions() {
        let status = ProjectStatus {
            name: "test".into(),
            path: "/tmp/test".into(),
            languages: vec![],
            memories: vec![],
            has_index: false,
            system_prompt: None,
            github_enabled: true,
            workspace: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(
            result.contains("github_identity"),
            "should include GitHub tool docs when enabled"
        );
        assert!(
            result.contains("github_pr"),
            "should include GitHub PR docs when enabled"
        );
    }

    #[test]
    fn build_without_github_excludes_github_instructions() {
        let status = ProjectStatus {
            name: "test".into(),
            path: "/tmp/test".into(),
            languages: vec![],
            memories: vec![],
            has_index: false,
            system_prompt: None,
            github_enabled: false,
            workspace: None,
        };
        let result = build_server_instructions(Some(&status));
        // Check for content unique to github_instructions.md (not the hint in server_instructions.md)
        assert!(
            !result.contains("github_identity(method)"),
            "should NOT include optional GitHub tool reference docs when disabled"
        );
    }

    #[test]
    fn build_with_workspace_appends_project_table() {
        let status = ProjectStatus {
            name: "backend-kotlin".into(),
            path: "/workspace/backend-kotlin".into(),
            languages: vec!["kotlin".into()],
            memories: vec![],
            has_index: false,
            system_prompt: None,
            github_enabled: false,
            workspace: Some(vec![
                WorkspaceProjectSummary {
                    id: "backend-kotlin".into(),
                    root: ".".into(),
                    languages: vec!["kotlin".into()],
                    depends_on: vec![],
                },
                WorkspaceProjectSummary {
                    id: "mcp-server".into(),
                    root: "mcp-server/".into(),
                    languages: vec!["typescript".into()],
                    depends_on: vec![],
                },
                WorkspaceProjectSummary {
                    id: "python-services".into(),
                    root: "python-services/".into(),
                    languages: vec!["python".into()],
                    depends_on: vec!["mcp-server".into()],
                },
            ]),
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("## Workspace Projects"));
        assert!(result.contains("mcp-server"));
        assert!(result.contains("python-services"));
        assert!(result.contains("python-services/"));
        // depends_on rendered for python-services
        assert!(result.contains("mcp-server"));
        // scoping hint present
        assert!(result.contains("project: \"<id>\""));
    }

    #[test]
    fn build_with_single_project_no_workspace_table() {
        // workspace: None → no table emitted even if the field is absent
        let status = ProjectStatus {
            name: "solo".into(),
            path: "/solo".into(),
            languages: vec!["rust".into()],
            memories: vec![],
            has_index: false,
            system_prompt: None,
            github_enabled: false,
            workspace: None,
        };
        let result = build_server_instructions(Some(&status));
        assert!(!result.contains("## Workspace Projects"));
    }

    #[test]
    fn build_onboarding_shows_index_ready() {
        let result = build_onboarding_prompt(&OnboardingContext {
            languages: &["rust".into()],
            top_level: &[],
            key_files: &[],
            ci_files: &[],
            entry_points: &[],
            test_dirs: &[],
            index_ready: true,
            index_files: 42,
            index_chunks: 350,
            projects: &[],
            is_workspace: false,
        });
        assert!(result.contains("Semantic index:** ready (42 files, 350 chunks)"));
    }

    #[test]
    fn build_onboarding_shows_index_not_built() {
        let result = build_onboarding_prompt(&OnboardingContext {
            languages: &["rust".into()],
            top_level: &[],
            key_files: &[],
            ci_files: &[],
            entry_points: &[],
            test_dirs: &[],
            index_ready: false,
            index_files: 0,
            index_chunks: 0,
            projects: &[],
            is_workspace: false,
        });
        assert!(result.contains("Semantic index:** not built"));
    }

    #[test]
    fn onboarding_prompt_includes_workspace_projects() {
        use std::path::PathBuf;
        let projects = vec![
            crate::workspace::DiscoveredProject {
                id: "api".to_string(),
                relative_root: PathBuf::from("api"),
                languages: vec!["rust".to_string()],
                manifest: Some("Cargo.toml".to_string()),
            },
            crate::workspace::DiscoveredProject {
                id: "frontend".to_string(),
                relative_root: PathBuf::from("frontend"),
                languages: vec!["typescript".to_string()],
                manifest: Some("package.json".to_string()),
            },
        ];
        let ctx = OnboardingContext {
            languages: &["rust".to_string(), "typescript".to_string()],
            top_level: &["api/".to_string(), "frontend/".to_string()],
            key_files: &[],
            ci_files: &[],
            entry_points: &["api/src/main.rs".to_string()],
            test_dirs: &[],
            index_ready: false,
            index_files: 0,
            index_chunks: 0,
            projects: &projects,
            is_workspace: true,
        };
        let prompt = build_onboarding_prompt(&ctx);
        assert!(prompt.contains("Workspace"));
        assert!(prompt.contains("Phase 1A"));
        assert!(prompt.contains("api"));
        assert!(prompt.contains("frontend"));
    }
}
