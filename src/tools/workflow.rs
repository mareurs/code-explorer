//! Workflow and onboarding tools.

use std::path::Path;

use super::{Tool, ToolContext};
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

fn build_system_prompt_draft(languages: &[String], entry_points: &[String]) -> String {
    let mut draft = String::new();
    draft.push_str("# Project — Code Explorer Guidance\n\n");

    // Entry points section
    draft.push_str("## Entry Points\n");
    if entry_points.is_empty() {
        draft.push_str("- Explore with `list_dir(\".\")` then `list_symbols` on key files\n");
    } else {
        for ep in entry_points {
            draft.push_str(&format!("- `{}` — start here\n", ep));
        }
    }
    draft.push('\n');

    // Key abstractions — placeholder for the LLM to fill
    draft.push_str("## Key Abstractions\n");
    draft.push_str("- [Discover with `list_symbols` on main source directories]\n\n");

    // Search tips
    draft.push_str("## Search Tips\n");
    if !languages.is_empty() {
        draft.push_str(&format!("- This is a {} project\n", languages.join("/")));
    }
    draft.push_str("- Use specific terms over generic ones (e.g., avoid 'data', 'utils')\n\n");

    // Navigation strategy
    draft.push_str("## Navigation Strategy\n");
    draft.push_str("1. `read_memory(\"architecture\")` — orient yourself\n");
    if !entry_points.is_empty() {
        draft.push_str(&format!(
            "2. `list_symbols(\"{}\")` — see main structure\n",
            entry_points[0]
        ));
    } else {
        draft.push_str("2. `list_symbols(\"src/\")` — see main structure\n");
    }
    draft.push_str("3. `semantic_search(\"your concept\")` — find relevant code\n");
    draft.push_str("4. `find_symbol(\"Name\", include_body=true)` — read implementation\n\n");

    // Project rules — placeholder
    draft.push_str("## Project Rules\n");
    draft.push_str("- [Add project-specific conventions here]\n");

    draft
}

#[async_trait::async_trait]
impl Tool for Onboarding {
    fn name(&self) -> &str {
        "onboarding"
    }
    fn description(&self) -> &str {
        "Perform initial project discovery: detect languages, read key files \
         (README, build config, CLAUDE.md), and return instructions for creating \
         project memories and a system prompt draft. Requires an active project. \
         Returns status if already onboarded (use force=true to re-scan)."
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
                    system_prompt: None,
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

        // Build the system prompt draft scaffold
        let system_prompt_draft = build_system_prompt_draft(&lang_list, &gathered.entry_points);

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
            "system_prompt_draft": system_prompt_draft,
        }))
    }
}
#[async_trait::async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a shell command in the project root. Large output is buffered with a smart summary \
         — query it with Unix tools via @output_id refs (e.g. grep pattern @cmd_abc)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute. May reference stored output buffers with @output_id syntax (e.g. grep FAILED @cmd_a1b2c3)."
                },
                "timeout_secs": {
                    "type": "integer",
                    "default": 30,
                    "description": "Max execution time in seconds (default: 30)."
                },
                "cwd": {
                    "type": "string",
                    "description": "Subdirectory relative to project root. Validated to stay within project."
                },
                "acknowledge_risk": {
                    "type": "boolean",
                    "description": "Bypass speed bump for dangerous commands. Required after a destructive command is detected."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use super::output_buffer::OutputBuffer;

        let command = super::require_str_param(&input, "command")?;
        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30);
        let acknowledge_risk = input["acknowledge_risk"].as_bool().unwrap_or(false);
        let cwd_param = input["cwd"].as_str();
        let root = ctx.agent.require_project_root().await?;
        let security = ctx.agent.security_config().await;

        // --- Step 1: Resolve @cmd_ buffer references ---
        let (resolved_command, temp_files, buffer_only) =
            ctx.output_buffer.resolve_refs(command)?;

        // Helper: run inner logic then always clean up temp files.
        let result = run_command_inner(
            command,
            &resolved_command,
            timeout_secs,
            acknowledge_risk,
            cwd_param,
            buffer_only,
            &root,
            &security,
            ctx,
        )
        .await;

        OutputBuffer::cleanup_temp_files(&temp_files);
        result
    }
}

/// Inner logic for `RunCommand::call`, extracted so temp-file cleanup
/// always happens in the caller regardless of early returns.
#[allow(clippy::too_many_arguments)]
async fn run_command_inner(
    original_command: &str,
    resolved_command: &str,
    timeout_secs: u64,
    acknowledge_risk: bool,
    cwd_param: Option<&str>,
    buffer_only: bool,
    root: &Path,
    security: &crate::util::path_security::PathSecurityConfig,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    use super::command_summary::{
        count_lines, detect_command_type, needs_summary, summarize_build_output, summarize_generic,
        summarize_test_output, CommandType,
    };
    use crate::util::path_security::is_dangerous_command;

    // --- Step 2: Dangerous command speed bump ---
    if !buffer_only && !acknowledge_risk {
        // Use resolved_command (with @refs substituted) so buffer-only grep/awk
        // commands don't get flagged for patterns in the buffer content.
        if let Some(reason) = is_dangerous_command(resolved_command, security) {
            return Err(super::RecoverableError::with_hint(
                format!("dangerous command blocked: {}", reason),
                "Re-run with acknowledge_risk: true if you are certain this is safe.",
            )
            .into());
        }
    }

    // --- Step 3: Shell command mode check (skip for buffer-only queries) ---
    if !buffer_only {
        match security.shell_command_mode.as_str() {
            "disabled" => {
                return Err(super::RecoverableError::with_hint(
                    "shell commands are disabled",
                    "Set security.shell_command_mode = \"warn\" or \"unrestricted\" in .code-explorer/project.toml",
                ).into());
            }
            "unrestricted" | "warn" | "" => {} // allowed
            other => {
                return Err(super::RecoverableError::with_hint(
                    format!("unknown shell_command_mode: '{}'", other),
                    "Use \"warn\", \"unrestricted\", or \"disabled\".",
                )
                .into());
            }
        }
    }

    // --- Step 4: Resolve working directory ---
    let work_dir = if let Some(rel) = cwd_param {
        let candidate = root.join(rel);
        let canonical = candidate.canonicalize().map_err(|e| {
            super::RecoverableError::with_hint(
                format!("cwd '{}' is not a valid directory: {}", rel, e),
                "Provide a relative path to an existing subdirectory of the project.",
            )
        })?;
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !canonical.starts_with(canonical_root.as_path()) {
            return Err(super::RecoverableError::with_hint(
                format!("cwd '{}' escapes project root", rel),
                "The cwd must be a subdirectory within the project.",
            )
            .into());
        }
        canonical
    } else {
        root.to_path_buf()
    };

    // --- Step 5: Execute command ---
    #[cfg(unix)]
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(resolved_command)
        .current_dir(&work_dir)
        .output();

    #[cfg(windows)]
    let child = tokio::process::Command::new("cmd")
        .arg("/C")
        .arg(resolved_command)
        .current_dir(&work_dir)
        .output();

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child).await {
        Ok(Ok(output)) => {
            let raw_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let raw_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let exit_code = output.status.code().unwrap_or(-1);

            // --- Step 6: Decide whether to buffer + summarize ---
            if needs_summary(&raw_stdout, &raw_stderr) {
                let output_id = ctx.output_buffer.store(
                    original_command.to_string(),
                    raw_stdout.clone(),
                    raw_stderr.clone(),
                    exit_code,
                );

                let cmd_type = detect_command_type(original_command);
                let mut summary = match cmd_type {
                    CommandType::Test => summarize_test_output(&raw_stdout, &raw_stderr, exit_code),
                    CommandType::Build => {
                        summarize_build_output(&raw_stdout, &raw_stderr, exit_code)
                    }
                    CommandType::Generic => summarize_generic(&raw_stdout, &raw_stderr, exit_code),
                };

                // Add buffer metadata
                summary["output_id"] = json!(output_id);
                summary["total_stdout_lines"] = json!(count_lines(&raw_stdout));
                let stderr_lines = count_lines(&raw_stderr);
                if stderr_lines > 0 {
                    summary["total_stderr_lines"] = json!(stderr_lines);
                }
                summary["hint"] = json!(format!(
                    "Full output stored. Query with: grep/tail/awk/sed {}",
                    output_id
                ));

                // Add warning in warn mode
                if !buffer_only
                    && (security.shell_command_mode == "warn"
                        || security.shell_command_mode.is_empty())
                {
                    summary["warning"] = json!(
                        "Shell commands execute with full user permissions. Only use for build/test commands."
                    );
                }

                Ok(summary)
            } else {
                // Short output — return directly
                let mut result = json!({
                    "stdout": raw_stdout,
                    "stderr": raw_stderr,
                    "exit_code": exit_code,
                });

                // Add warning in warn mode
                if !buffer_only
                    && (security.shell_command_mode == "warn"
                        || security.shell_command_mode.is_empty())
                {
                    result["warning"] = json!(
                        "Shell commands execute with full user permissions. Only use for build/test commands."
                    );
                }

                Ok(result)
            }
        }
        Ok(Err(e)) => {
            Err(super::RecoverableError::new(format!("command execution error: {}", e)).into())
        }
        Err(_) => Ok(json!({
            "timed_out": true,
            "stdout": "",
            "stderr": format!("Command timed out after {} seconds", timeout_secs),
            "exit_code": null
        })),
    }
}

#[allow(dead_code)] // Kept as safety net for byte-level shell_output_limit_bytes config.
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
    use std::sync::Arc;
    use tempfile::tempdir;

    fn lsp() -> Arc<dyn crate::lsp::LspProvider> {
        crate::lsp::LspManager::new_arc()
    }

    async fn project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        // Create some source files for language detection
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("lib.py"), "def hello(): pass").unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: lsp(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
            },
        )
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
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
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
        // Generate output larger than summary threshold (>50 lines).
        // seq 1 100000 produces ~588KB / 100K lines — should be buffered.
        let result = RunCommand
            .call(
                json!({ "command": "seq 1 100000", "timeout_secs": 10 }),
                &ctx,
            )
            .await
            .unwrap();
        // New behavior: large output is buffered, not byte-truncated.
        assert!(
            result["output_id"].as_str().is_some(),
            "large output should be buffered with output_id"
        );
        assert!(result["total_stdout_lines"].as_u64().unwrap() > 50);
        assert!(result["hint"]
            .as_str()
            .unwrap()
            .contains("Full output stored"));
    }

    #[tokio::test]
    async fn execute_shell_command_small_output_not_truncated() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo hello", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        // Short output: no output_id, direct stdout
        assert_eq!(result["output_id"], serde_json::Value::Null);
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
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
        };
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

    #[tokio::test]
    async fn onboarding_includes_system_prompt_draft_field() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Test Project\nA test.").unwrap();
        std::fs::write(dir.path().join("main.py"), "print('hello')").unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
        };
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();

        // system_prompt_draft should be present and be a string
        assert!(
            result.get("system_prompt_draft").is_some(),
            "onboarding output should include system_prompt_draft"
        );
        assert!(
            result["system_prompt_draft"].is_string(),
            "system_prompt_draft should be a string"
        );
        let draft = result["system_prompt_draft"].as_str().unwrap();
        assert!(!draft.is_empty(), "system_prompt_draft should not be empty");
    }

    // -----------------------------------------------------------------------
    // New RunCommand tests (Task 5)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_command_short_output_no_buffer() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo hello" }), &ctx)
            .await
            .unwrap();
        // Short output: direct stdout, no output_id
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(result["output_id"], serde_json::Value::Null);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_long_output_buffered() {
        let (_dir, ctx) = project_ctx().await;
        // seq 1 200 produces 200 lines — well above the 50-line threshold
        let result = RunCommand
            .call(json!({ "command": "seq 1 200", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"].as_str().unwrap();
        assert!(output_id.starts_with("@cmd_"));
        assert_eq!(result["total_stdout_lines"].as_u64().unwrap(), 200);
        assert!(result["hint"].as_str().unwrap().contains(output_id));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_ref_query() {
        let (_dir, ctx) = project_ctx().await;
        // First: generate output and get a buffer handle
        let result = RunCommand
            .call(json!({ "command": "seq 1 200", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"].as_str().unwrap();

        // Second: query the buffered output via grep
        let query = format!("grep '^10$' {}", output_id);
        let result2 = RunCommand
            .call(json!({ "command": query, "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let stdout = result2["stdout"].as_str().unwrap();
        assert!(stdout.contains("10"), "grep should find '10': {}", stdout);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_dangerous_blocked_without_acknowledge() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(
                json!({ "command": "rm -rf /tmp/code_explorer_test_nonexistent" }),
                &ctx,
            )
            .await;
        // Should be an error (dangerous command blocked)
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("dangerous command blocked"),
            "error should mention dangerous: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn run_command_dangerous_allowed_with_acknowledge() {
        let (_dir, ctx) = project_ctx().await;
        // Use a safe command but with acknowledge_risk: true — should succeed
        let result = RunCommand
            .call(
                json!({ "command": "echo safe", "acknowledge_risk": true }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("safe"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_skips_safety() {
        let (_dir, ctx) = project_ctx().await;
        // Store some output in the buffer
        let result = RunCommand
            .call(json!({ "command": "seq 1 200", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"].as_str().unwrap();

        // grep on buffer ref only — should skip both dangerous-command check
        // and shell_command_mode check (buffer_only = true).
        let query = format!("grep '^5$' {}", output_id);
        let result2 = RunCommand
            .call(json!({ "command": query, "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        // No warning should be present when buffer_only
        // (the default mode is "warn" which adds warning for non-buffer commands)
        assert_eq!(
            result2["warning"],
            serde_json::Value::Null,
            "buffer-only queries should not get shell warning"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_cwd_works() {
        let (dir, ctx) = project_ctx().await;
        // Create a subdirectory with a file
        let sub = dir.path().join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("hello.txt"), "world").unwrap();

        let result = RunCommand
            .call(
                json!({ "command": "cat hello.txt", "cwd": "subdir", "timeout_secs": 5 }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "world");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_cwd_rejects_traversal() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(
                json!({ "command": "ls", "cwd": "../../etc", "timeout_secs": 5 }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("escapes project root") || err_msg.contains("not a valid directory"),
            "should reject traversal: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn run_command_dangerous_rejected_without_ack() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({"command": "rm -rf /tmp/ce_nonexistent_test"}), &ctx)
            .await;
        assert!(result.is_err(), "dangerous command should be rejected");
        let err = result.unwrap_err();
        let rec = err
            .downcast_ref::<crate::tools::RecoverableError>()
            .expect("should be a RecoverableError, not a fatal error");
        assert!(
            rec.message.to_lowercase().contains("dangerous"),
            "message should mention dangerous, got: {}",
            rec.message
        );
        assert!(
            rec.hint
                .as_deref()
                .unwrap_or("")
                .contains("acknowledge_risk"),
            "hint should mention acknowledge_risk, got: {:?}",
            rec.hint
        );
    }

    #[tokio::test]
    async fn run_command_safe_command_not_blocked() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({"command": "echo hello"}), &ctx)
            .await;
        assert!(result.is_ok(), "echo should not be blocked: {:?}", result);
    }

    #[tokio::test]
    async fn run_command_cwd_rejects_nonexistent_path() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(
                json!({"command": "ls", "cwd": "definitely_nonexistent_subdir_xyz"}),
                &ctx,
            )
            .await;
        assert!(result.is_err(), "nonexistent cwd should be rejected");
        let err = result.unwrap_err();
        let rec = err
            .downcast_ref::<crate::tools::RecoverableError>()
            .expect("should be RecoverableError");
        assert!(
            rec.message.contains("not accessible") || rec.message.contains("not a valid"),
            "got: {}",
            rec.message
        );
    }

    #[tokio::test]
    async fn run_command_cwd_rejects_path_escaping_root() {
        let (_dir, ctx) = project_ctx().await;
        // Use /tmp directly — it always exists and is outside any temp project root.
        // On Linux, PathBuf::join("/tmp") ignores the existing path and gives /tmp,
        // so root.join("/tmp") canonicalizes to /tmp, which fails starts_with(root).
        let result = RunCommand
            .call(json!({"command": "ls", "cwd": "/tmp"}), &ctx)
            .await;
        assert!(
            result.is_err(),
            "absolute cwd outside root should be rejected"
        );
        let err = result.unwrap_err();
        let rec = err
            .downcast_ref::<crate::tools::RecoverableError>()
            .expect("should be RecoverableError");
        assert!(
            rec.message.contains("escapes project root"),
            "got: {}",
            rec.message
        );
    }

    #[tokio::test]
    async fn run_command_buffer_only_skips_speed_bump() {
        let (_dir, ctx) = project_ctx().await;
        // Store directly in buffer — no need to run a command that may or may not buffer
        // depending on the current buffering threshold.
        let id = ctx
            .output_buffer
            .store("test_cmd".into(), "rm -rf data\n".into(), "".into(), 0);
        // "rm" appears in the buffer content, but the query command is buffer-only.
        // It should NOT be rejected as dangerous.
        let result = RunCommand
            .call(json!({"command": format!("grep rm {}", id)}), &ctx)
            .await;
        // Should succeed (or fail with grep exit 1 "not found") — but NOT as a RecoverableError
        // about dangerous commands.
        match result {
            Ok(v) => {
                assert!(
                    v.get("error")
                        .map(|e| !e
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains("dangerous"))
                        .unwrap_or(true),
                    "buffer-only grep should not be flagged as dangerous"
                );
            }
            Err(e) => {
                let rec = e.downcast_ref::<crate::tools::RecoverableError>();
                assert!(
                    rec.map(|r| !r.message.to_lowercase().contains("dangerous"))
                        .unwrap_or(false),
                    "buffer-only should not fail with dangerous error"
                );
            }
        }
    }

    #[test]
    fn run_command_schema_has_cwd_and_acknowledge_risk() {
        let schema = RunCommand.input_schema();

        let cwd = &schema["properties"]["cwd"];
        assert!(cwd.is_object(), "cwd should be a schema object");
        assert_eq!(cwd["type"], "string", "cwd type should be string");

        let ack = &schema["properties"]["acknowledge_risk"];
        assert!(
            ack.is_object(),
            "acknowledge_risk should be a schema object"
        );
        assert_eq!(
            ack["type"], "boolean",
            "acknowledge_risk type should be boolean"
        );

        let required = schema["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v == "command"),
            "command must remain required"
        );
    }
}
