//! Workflow and onboarding tools.

use super::{Tool, ToolContext};
use anyhow::anyhow;
use serde_json::{json, Value};

pub struct Onboarding;
pub struct RunCommand;

/// Context gathered from well-known project files during onboarding.
#[derive(Debug, Default)]
struct GatheredContext {
    readme: Option<String>,
    build_file_name: Option<String>,
    build_file_content: Option<String>,
    claude_md: Option<String>,
    ci_files: Vec<String>,
    entry_points: Vec<String>,
    test_dirs: Vec<String>,
}

const MAX_GATHERED_FILE_BYTES: u64 = 32_000;

/// Read a file if it exists and is within the size cap.
fn read_capped(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_GATHERED_FILE_BYTES {
        let content = std::fs::read_to_string(path).ok()?;
        let truncated: String = content
            .chars()
            .take(MAX_GATHERED_FILE_BYTES as usize)
            .collect();
        Some(format!(
            "{}\n\n[... truncated at {} bytes ...]",
            truncated, MAX_GATHERED_FILE_BYTES
        ))
    } else {
        std::fs::read_to_string(path).ok()
    }
}

/// Read key project files up-front so the onboarding prompt can include them.
fn gather_project_context(root: &std::path::Path) -> GatheredContext {
    let mut ctx = GatheredContext::default();

    // README (try common names)
    for name in &["README.md", "README.rst", "README.txt", "README"] {
        if let Some(content) = read_capped(&root.join(name)) {
            ctx.readme = Some(content);
            break;
        }
    }

    // CLAUDE.md
    ctx.claude_md = read_capped(&root.join("CLAUDE.md"));

    // Build file (first match wins, ordered by popularity)
    let build_files = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "build.gradle.kts",
        "build.gradle",
        "go.mod",
        "pom.xml",
        "Makefile",
        "CMakeLists.txt",
        "setup.py",
        "mix.exs",
        "Gemfile",
    ];
    for name in &build_files {
        if let Some(content) = read_capped(&root.join(name)) {
            ctx.build_file_name = Some(name.to_string());
            ctx.build_file_content = Some(content);
            break;
        }
    }

    // CI config files (just names, not contents)
    for dir in &[".github/workflows", ".gitlab", ".circleci"] {
        let ci_path = root.join(dir);
        if ci_path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&ci_path) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".yml") || name.ends_with(".yaml") {
                        ctx.ci_files.push(format!("{}/{}", dir, name));
                    }
                }
            }
        }
    }
    ctx.ci_files.sort();

    // Entry points (check common locations)
    let entry_candidates = [
        "src/main.rs",
        "src/lib.rs",
        "src/main.py",
        "src/index.ts",
        "src/index.js",
        "src/app.ts",
        "src/app.py",
        "main.go",
        "cmd/main.go",
        "lib/main.dart",
        "index.js",
        "index.ts",
        "app.py",
        "manage.py",
    ];
    for candidate in &entry_candidates {
        if root.join(candidate).exists() {
            ctx.entry_points.push(candidate.to_string());
        }
    }

    // Test directories
    for candidate in &[
        "tests",
        "test",
        "spec",
        "src/test",
        "src/tests",
        "__tests__",
    ] {
        if root.join(candidate).is_dir() {
            ctx.test_dirs.push(candidate.to_string());
        }
    }

    ctx
}

#[async_trait::async_trait]
impl Tool for Onboarding {
    fn name(&self) -> &str {
        "onboarding"
    }
    fn description(&self) -> &str {
        "Perform initial project discovery: detect languages, read key files \
         (README, build config, CLAUDE.md), and return instructions for creating \
         project memories. Requires an active project. Returns status if already \
         onboarded (use force=true to re-scan)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "force": {
                    "type": "boolean",
                    "description": "Force full re-scan even if already onboarded (default: false)"
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let root = ctx.agent.require_project_root().await?;
        let force = input["force"].as_bool().unwrap_or(false);

        // If already onboarded and not forced, return status instead of re-scanning
        if !force {
            let status = ctx
                .agent
                .with_project(|p| {
                    let has_config = p.root.join(".code-explorer").join("project.toml").exists();
                    let memories = p.memory.list()?;
                    let has_onboarding_memory = memories.iter().any(|m| m == "onboarding");
                    Ok((has_config, has_onboarding_memory, memories))
                })
                .await?;
            let (has_config, has_onboarding_memory, memories) = status;
            if has_config && has_onboarding_memory {
                let message = format!(
                    "Onboarding already performed. Available memories: {}. \
                     Use `read_memory(topic)` to read relevant ones as needed for your current task. \
                     Do not read all memories at once — only read those relevant to what you're working on.",
                    memories.join(", ")
                );
                return Ok(json!({
                    "onboarded": true,
                    "has_config": true,
                    "has_onboarding_memory": true,
                    "memories": memories,
                    "message": message,
                }));
            }
        }

        // Detect languages by walking files
        let mut languages = std::collections::BTreeSet::new();
        let walker = ignore::WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build();
        for entry in walker.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(lang) = crate::ast::detect_language(entry.path()) {
                    languages.insert(lang.to_string());
                }
            }
        }

        // List top-level entries
        let mut top_level = vec![];
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let suffix = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    "/"
                } else {
                    ""
                };
                top_level.push(format!("{}{}", name, suffix));
            }
        }
        top_level.sort();

        // Create .code-explorer/project.toml if it doesn't exist
        let config_dir = root.join(".code-explorer");
        let config_path = config_dir.join("project.toml");
        let created_config = if !config_path.exists() {
            std::fs::create_dir_all(&config_dir)?;
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string();
            let langs: Vec<String> = languages.iter().cloned().collect();
            let config = crate::config::project::ProjectConfig {
                project: crate::config::project::ProjectSection {
                    name,
                    languages: langs,
                    encoding: "utf-8".into(),
                    tool_timeout_secs: 60,
                },
                embeddings: Default::default(),
                ignored_paths: Default::default(),
                security: Default::default(),
            };
            let toml_str = toml::to_string_pretty(&config)?;
            std::fs::write(&config_path, &toml_str)?;
            true
        } else {
            false
        };

        // Gather rich context from well-known project files
        let gathered = gather_project_context(&root);

        // Store onboarding result in memory
        let lang_list: Vec<String> = languages.iter().cloned().collect();
        ctx.agent
            .with_project(|p| {
                let summary = format!(
                    "Languages: {}\nHas README: {}\nHas CLAUDE.md: {}\nBuild file: {}\nEntry points: {}\nTest dirs: {}",
                    lang_list.join(", "),
                    gathered.readme.is_some(),
                    gathered.claude_md.is_some(),
                    gathered.build_file_name.as_deref().unwrap_or("none"),
                    if gathered.entry_points.is_empty() { "none".to_string() } else { gathered.entry_points.join(", ") },
                    if gathered.test_dirs.is_empty() { "none".to_string() } else { gathered.test_dirs.join(", ") },
                );
                p.memory.write("onboarding", &summary)?;
                Ok(())
            })
            .await?;

        // Build the onboarding instruction prompt
        let prompt = crate::prompts::build_onboarding_prompt(
            &lang_list,
            &top_level,
            gathered.readme.as_deref(),
            gathered
                .build_file_name
                .as_deref()
                .zip(gathered.build_file_content.as_deref()),
            gathered.claude_md.as_deref(),
            &gathered.ci_files,
            &gathered.entry_points,
            &gathered.test_dirs,
        );

        Ok(json!({
            "languages": lang_list,
            "top_level": top_level,
            "config_created": created_config,
            "has_readme": gathered.readme.is_some(),
            "has_claude_md": gathered.claude_md.is_some(),
            "build_file": gathered.build_file_name,
            "entry_points": gathered.entry_points,
            "test_dirs": gathered.test_dirs,
            "ci_files": gathered.ci_files,
            "instructions": prompt,
        }))
    }
}
#[async_trait::async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a shell command in the active project root and return stdout/stderr."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string" },
                "timeout_secs": { "type": "integer", "default": 30 }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'command' parameter"))?;
        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30);
        let root = ctx.agent.require_project_root().await?;
        let security = ctx.agent.security_config().await;

        // Check shell command mode
        match security.shell_command_mode.as_str() {
            "disabled" => {
                anyhow::bail!(
                    "shell commands are disabled. Set security.shell_command_mode = \"warn\" or \"unrestricted\" in .code-explorer/project.toml"
                );
            }
            "unrestricted" | "warn" | "" => {} // allowed
            other => {
                anyhow::bail!("unknown shell_command_mode: '{}'. Use \"warn\", \"unrestricted\", or \"disabled\"", other);
            }
        }

        #[cfg(unix)]
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&root)
            .output();

        #[cfg(windows)]
        let child = tokio::process::Command::new("cmd")
            .arg("/C")
            .arg(command)
            .current_dir(&root)
            .output();

        let output_limit = if security.shell_output_limit_bytes > 0 {
            security.shell_output_limit_bytes
        } else {
            100 * 1024
        };

        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child).await {
            Ok(Ok(output)) => {
                let raw_stdout = String::from_utf8_lossy(&output.stdout);
                let raw_stderr = String::from_utf8_lossy(&output.stderr);

                let (stdout, stdout_truncated) = truncate_output(&raw_stdout, output_limit);
                let (stderr, stderr_truncated) = truncate_output(&raw_stderr, output_limit);

                let mut result = json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": output.status.code()
                });

                if stdout_truncated {
                    result["stdout_truncated"] = json!(true);
                    result["stdout_total_bytes"] = json!(raw_stdout.len());
                }
                if stderr_truncated {
                    result["stderr_truncated"] = json!(true);
                    result["stderr_total_bytes"] = json!(raw_stderr.len());
                }

                // Add warning in warn mode
                if security.shell_command_mode == "warn" || security.shell_command_mode.is_empty() {
                    result["warning"] = json!(
                        "Shell commands execute with full user permissions. Only use for build/test commands."
                    );
                }

                Ok(result)
            }
            Ok(Err(e)) => Err(anyhow!("command execution error: {}", e)),
            Err(_) => Ok(json!({
                "timed_out": true,
                "stdout": "",
                "stderr": format!("Command timed out after {} seconds", timeout_secs),
                "exit_code": null
            })),
        }
    }
}

fn truncate_output(output: &str, limit: usize) -> (String, bool) {
    if output.len() > limit {
        let truncated = &output[..limit];
        // Find a safe UTF-8 boundary
        let safe_end = truncated
            .char_indices()
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        (
            format!(
                "{}\n... (truncated, showing first {} of {} bytes)",
                &output[..safe_end],
                safe_end,
                output.len()
            ),
            true,
        )
    } else {
        (output.to_string(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn lsp() -> Arc<LspManager> {
        Arc::new(LspManager::new())
    }

    async fn project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        // Create some source files for language detection
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("lib.py"), "def hello(): pass").unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (dir, ToolContext { agent, lsp: lsp() })
    }

    #[tokio::test]
    async fn onboarding_detects_languages() {
        let (_dir, ctx) = project_ctx().await;
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        let langs: Vec<&str> = result["languages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(langs.contains(&"rust"));
        assert!(langs.contains(&"python"));
    }

    #[tokio::test]
    async fn onboarding_creates_config() {
        let (dir, ctx) = project_ctx().await;
        // Remove the config if it exists
        let _ = std::fs::remove_file(dir.path().join(".code-explorer/project.toml"));

        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["config_created"], true);
        assert!(dir.path().join(".code-explorer/project.toml").exists());
    }

    #[tokio::test]
    async fn onboarding_returns_status_when_already_done() {
        let (dir, ctx) = project_ctx().await;
        let _ = std::fs::remove_file(dir.path().join(".code-explorer/project.toml"));

        // First call does full onboarding
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        assert!(result.get("languages").is_some()); // full onboarding result

        // Second call (no force) returns status instead
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["onboarded"], true);
        assert_eq!(result["has_config"], true);
        assert_eq!(result["has_onboarding_memory"], true);

        // Force re-scan
        let result = Onboarding
            .call(json!({ "force": true }), &ctx)
            .await
            .unwrap();
        assert!(result.get("languages").is_some()); // full onboarding again
    }
    #[tokio::test]
    async fn onboarding_returns_instruction_prompt() {
        let (_dir, ctx) = project_ctx().await;
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("## Rules"));
        assert!(instructions.contains("## Memories to Create"));
        assert!(instructions.contains("rust")); // detected language
    }

    #[tokio::test]
    async fn onboarding_errors_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
        };
        assert!(Onboarding.call(json!({}), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn onboarding_status_includes_memories_and_message() {
        let (_dir, ctx) = project_ctx().await;

        // Run onboarding first
        Onboarding.call(json!({}), &ctx).await.unwrap();

        // Status call returns guidance message and memories
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        let msg = result["message"].as_str().unwrap();
        assert!(msg.contains("already performed"));
        assert!(result["memories"].as_array().unwrap().len() > 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_shell_command_timeout_is_enforced() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "sleep 10", "timeout_secs": 1 }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["timed_out"], true, "command should have timed out");
        assert!(result["stderr"]
            .as_str()
            .unwrap()
            .contains("timed out after 1 seconds"));
    }

    #[tokio::test]
    async fn execute_shell_command_fast_command_succeeds() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo hello", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["timed_out"], serde_json::Value::Null);
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_shell_command_output_truncated() {
        let (_dir, ctx) = project_ctx().await;
        // Generate output larger than default limit
        let result = RunCommand
            .call(
                json!({ "command": "seq 1 100000", "timeout_secs": 10 }),
                &ctx,
            )
            .await
            .unwrap();
        // Output should be truncated (seq 1 100000 produces ~588KB)
        assert_eq!(
            result["stdout_truncated"], true,
            "large output should be truncated"
        );
        assert!(result["stdout_total_bytes"].as_u64().unwrap() > 100 * 1024);
        // The truncated output should still be valid
        assert!(result["stdout"].as_str().unwrap().contains("truncated"));
    }

    #[tokio::test]
    async fn execute_shell_command_small_output_not_truncated() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo hello", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["stdout_truncated"], serde_json::Value::Null);
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn execute_shell_command_warn_mode_includes_warning() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo test", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        assert!(
            result["warning"].as_str().is_some(),
            "warn mode should include warning"
        );
    }

    #[tokio::test]
    async fn execute_shell_command_exit_code_preserved() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "exit 42", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["exit_code"], 42);
    }

    #[tokio::test]
    async fn execute_shell_command_echo_cross_platform() {
        let (_dir, ctx) = project_ctx().await;
        // "echo hello" works on both sh and cmd.exe
        let result = RunCommand
            .call(json!({ "command": "echo hello", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let stdout = result["stdout"].as_str().unwrap();
        assert!(
            stdout.contains("hello"),
            "stdout should contain 'hello': {}",
            stdout
        );
    }

    #[test]
    fn gather_context_reads_readme_and_build_file() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("README.md"),
            "# My Project\nA test project.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        let ctx = gather_project_context(dir.path());
        assert_eq!(ctx.readme.as_deref(), Some("# My Project\nA test project."));
        assert_eq!(ctx.build_file_name.as_deref(), Some("Cargo.toml"));
        assert!(ctx.build_file_content.as_ref().unwrap().contains("test"));
        assert!(ctx.claude_md.is_none());
    }

    #[test]
    fn gather_context_finds_ci_files() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        std::fs::write(dir.path().join(".github/workflows/ci.yml"), "name: CI").unwrap();
        let ctx = gather_project_context(dir.path());
        assert_eq!(ctx.ci_files, vec![".github/workflows/ci.yml"]);
    }

    #[test]
    fn gather_context_finds_entry_points_and_test_dirs() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let ctx = gather_project_context(dir.path());
        assert!(ctx.entry_points.contains(&"src/main.rs".to_string()));
        assert!(ctx.test_dirs.contains(&"tests".to_string()));
    }

    #[test]
    fn gather_context_handles_empty_project() {
        let dir = tempdir().unwrap();
        let ctx = gather_project_context(dir.path());
        assert!(ctx.readme.is_none());
        assert!(ctx.build_file_name.is_none());
        assert!(ctx.claude_md.is_none());
        assert!(ctx.ci_files.is_empty());
        assert!(ctx.entry_points.is_empty());
        assert!(ctx.test_dirs.is_empty());
    }

    #[tokio::test]
    async fn onboarding_returns_gathered_context_fields() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "# Test Project").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext { agent, lsp: lsp() };
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();

        assert_eq!(result["has_readme"], true);
        assert_eq!(result["build_file"], "Cargo.toml");
        assert!(result["test_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "tests"));
        // Verify the instructions now contain the gathered README content
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("# Test Project"));
    }
}
