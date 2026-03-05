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
    /// Path to FEATURES.md if found (relative to project root)
    features_md: Option<String>,
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

    // FEATURES.md — documents implemented capabilities
    for candidate in &["docs/FEATURES.md", "FEATURES.md", "docs/features.md"] {
        if root.join(candidate).exists() {
            ctx.features_md = Some(candidate.to_string());
            break;
        }
    }

    ctx
}

fn language_navigation_hints(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some(
            "- name_path: `StructName/method`, `impl Trait for Type/method`\n\
             - find_symbol(kind=\"struct\") for data types, kind=\"function\" for free fns\n\
             - impl blocks: `find_symbol(\"impl MyStruct\")` or list_symbols shows `impl Trait for Type`\n\
             - Example: `find_symbol(\"Server/handle_request\")` finds a method on Server",
        ),
        "python" => Some(
            "- name_path: `ClassName/method_name`, `module_func`\n\
             - find_symbol(kind=\"class\") for classes, kind=\"function\" for functions/methods\n\
             - Decorators aren't in name_path — search for the function name\n\
             - Example: `find_symbol(\"UserService/create\")` finds a method on UserService",
        ),
        "typescript" | "javascript" | "tsx" | "jsx" => Some(
            "- name_path: `ClassName/method`, `exportedFunction`\n\
             - find_symbol(kind=\"class\") for classes, kind=\"function\" for functions/arrow fns\n\
             - React components are functions — use kind=\"function\" not kind=\"class\"\n\
             - Example: `find_symbol(\"AuthProvider/login\")` finds a class method",
        ),
        "go" => Some(
            "- name_path: `TypeName/MethodName`, `PackageFunc`\n\
             - find_symbol(kind=\"function\") covers both functions and methods\n\
             - Receiver methods: `find_symbol(\"Server/ListenAndServe\")`\n\
             - Interfaces: find_symbol(kind=\"interface\") then list_symbols for signatures",
        ),
        "java" | "kotlin" => Some(
            "- name_path: `ClassName/methodName`, `InnerClass`\n\
             - find_symbol(kind=\"class\") for classes/interfaces, kind=\"function\" for methods\n\
             - Annotations aren't in name_path — search by method name\n\
             - Example: `find_symbol(\"UserRepository/findById\")`",
        ),
        "c" | "cpp" => Some(
            "- name_path: `ClassName/method`, `namespace_func`\n\
             - find_symbol(kind=\"struct\") or kind=\"class\" depending on codebase style\n\
             - Header vs implementation: find_symbol shows both — use path= to narrow",
        ),
        _ => None,
    }
}

fn build_system_prompt_draft(
    languages: &[String],
    entry_points: &[String],
    project_root: Option<&Path>,
) -> String {
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

    // Language-specific navigation hints
    let hints: Vec<_> = languages
        .iter()
        .filter_map(|lang| language_navigation_hints(lang).map(|h| (lang.as_str(), h)))
        .collect();
    if !hints.is_empty() {
        draft.push_str("## Language Navigation\n");
        for (lang, hint) in &hints {
            draft.push_str(&format!("**{}:**\n{}\n\n", lang, hint));
        }
    }

    // Navigation strategy
    draft.push_str("## Navigation Strategy\n");
    draft.push_str("1. `memory(action=\"read\", topic=\"architecture\")` — orient yourself\n");
    if !entry_points.is_empty() {
        draft.push_str(&format!(
            "2. `list_symbols(\"{}\")` — see main structure\n",
            entry_points[0]
        ));
    } else {
        draft.push_str("2. `list_symbols(\"src/\")` — see main structure\n");
    }
    draft.push_str("3. `semantic_search(\"your concept\")` — find relevant code\n");
    draft.push_str("4. `find_symbol(\"Name\", include_body=true)` — read implementation\n");
    draft
        .push_str("5. `memory(action=\"recall\", query=\"...\")` — search memories by meaning\n\n");

    // Project rules — placeholder
    draft.push_str("## Project Rules\n");
    draft.push_str("- [Add project-specific conventions here]\n");

    // Private memory rules
    draft.push_str("\n## Private Memory Rules\n\n");
    draft.push_str(
        "Private memories are gitignored — personal to this developer, not shared with the team.\n\
         They live in `.code-explorer/private-memories/`.\n\n\
         **Write to the private store** (`memory(action=\"write\", topic=..., content=..., private=true)`) for:\n\
         - Personal preferences and workflow rules for this developer\n\
         - Machine-specific config (local ports, paths, GPU type, env quirks)\n\
         - WIP notes and in-progress debugging context\n\
         - Personal debugging history specific to this setup\n\n\
         **Write to the shared store** (`memory(action=\"write\", topic=..., content=...)`) for:\n\
         - Architecture, conventions, design patterns — knowledge useful to ALL contributors\n\
         - When in doubt: private first, promote to shared only if universally applicable\n\n\
         **Each session:** `memory(action=\"list\", include_private=true)` to see what's available.\n",
    );

    // Semantic memories section
    draft.push_str("\n## Semantic Memories\n\n");
    draft.push_str(
        "Use `memory(action=\"remember\", content=\"...\")` to store knowledge that doesn't fit\n\
         a named topic. Search with `memory(action=\"recall\", query=\"...\")`. Buckets:\n\
         code, system, preferences, unstructured (auto-classified if omitted).\n\
         Delete with `memory(action=\"forget\", id=N)` using IDs from recall results.\n",
    );

    // Auto-inject preferences from semantic memory (best-effort)
    if let Some(root) = project_root {
        if let Ok(conn) = crate::embed::index::open_db(root) {
            if crate::embed::index::ensure_vec_memories(&conn).is_ok() {
                let mut prefs = Vec::new();
                if let Ok(mut stmt) = conn.prepare(
                    "SELECT title, content FROM memories WHERE bucket = 'preferences' \
                     ORDER BY updated_at DESC LIMIT 10",
                ) {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    }) {
                        for row in rows.flatten() {
                            prefs.push(row);
                        }
                    }
                }
                if !prefs.is_empty() {
                    draft.push_str("\n## User Preferences\n\n");
                    for (title, content) in &prefs {
                        // Use title as heading, content as body — keep it compact
                        let summary = if content.len() > 200 {
                            format!("{}...", &content[..200])
                        } else {
                            content.clone()
                        };
                        draft.push_str(&format!("- **{}:** {}\n", title, summary));
                    }
                }
            }
        }
    }

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
                    let private_memories = p.private_memory.list()?;
                    Ok((
                        has_config,
                        has_onboarding_memory,
                        memories,
                        private_memories,
                    ))
                })
                .await?;
            let (has_config, has_onboarding_memory, memories, private_memories) = status;
            if has_config && has_onboarding_memory {
                let mut message = format!(
                    "Onboarding already performed. Available shared memories: {}. \
                     Use `memory(action=\"read\", topic=...)` to read relevant ones as needed for your current task. \
                     Do not read all memories at once — only read those relevant to what you're working on. \
                     Use `memory(action=\"recall\", query=\"...\")` to search memories by meaning when the topic name isn't known.",
                    memories.join(", ")
                );
                if !private_memories.is_empty() {
                    message.push_str(&format!(
                        " Private memories: {}. Read with `memory(action=\"read\", topic=..., private=true)`.",
                        private_memories.join(", ")
                    ));
                }
                let mut response = json!({
                    "onboarded": true,
                    "has_config": true,
                    "has_onboarding_memory": true,
                    "memories": memories,
                    "message": message,
                });
                if !private_memories.is_empty() {
                    response["private_memories"] = json!(private_memories);
                }
                return Ok(response);
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
        let system_prompt_draft =
            build_system_prompt_draft(&lang_list, &gathered.entry_points, Some(&root));

        let features_suggestion = gathered.features_md.is_none().then_some(
            "No FEATURES.md found. Consider creating docs/FEATURES.md to document \
             implemented capabilities — helps agents understand what's already built \
             and avoid re-suggesting existing features.",
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
            "features_md": gathered.features_md,
            "features_suggestion": features_suggestion,
            "instructions": prompt,
            "system_prompt_draft": system_prompt_draft,
        }))
    }

    async fn call_content(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<Vec<rmcp::model::Content>> {
        let val = self.call(input, ctx).await?;

        // The "already onboarded" path returns a plain string message.
        if let Some(msg) = val.as_str() {
            return Ok(vec![rmcp::model::Content::text(msg.to_string())]);
        }

        // Full onboarding: always inline `instructions` and `system_prompt_draft`.
        // The default call_content buffers large JSON and shows only format_compact,
        // which drops these fields entirely — the LLM never sees what to do next.
        let compact = format_onboarding(&val);
        let instructions = val["instructions"].as_str().unwrap_or("");
        let system_prompt_draft = val["system_prompt_draft"].as_str().unwrap_or("");

        let mut response = format!("{}\n\n{}", compact, instructions);
        if !system_prompt_draft.is_empty() {
            response.push_str(&format!(
                "\n\n## System Prompt Draft\n\n```\n{}\n```",
                system_prompt_draft
            ));
        }
        if let Some(suggestion) = val["features_suggestion"].as_str() {
            response.push_str(&format!("\n\n> {}", suggestion));
        }

        Ok(vec![rmcp::model::Content::text(response)])
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_onboarding(result))
    }
}
#[async_trait::async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a shell command in the project root. Large output is buffered with a smart summary \
         — query it with Unix tools via @output_id refs (e.g. grep pattern @cmd_abc). \
         ALREADY IN THE PROJECT ROOT — do NOT prefix with `cd /abs/path &&`. \
         Use the `cwd` parameter for subdirectories."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute. NEVER prefix with `cd /abs/path &&` — already in the project root. May reference stored output buffers with @output_id syntax (e.g. grep FAILED @cmd_a1b2c3)."
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
                    "description": "Bypass dangerous-command check directly. Prefer the @ack_* handle protocol: \
                                    when a dangerous command is detected a pending_ack handle is returned — \
                                    re-run as run_command(\"@ack_<id>\") to execute without repeating the full command."
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

        // --- Early dispatch: @ack_* handle ---
        if looks_like_ack_handle(command) {
            // Cross-tool guard: edit_file also issues @ack_ handles (pending_edits store).
            // If the caller passed an edit ack to run_command, give a targeted error rather
            // than a misleading "expired" message.
            if ctx.output_buffer.get_pending_edit(command).is_some() {
                return Err(super::RecoverableError::with_hint(
                    "this ack handle belongs to edit_file, not run_command",
                    format!(
                        "Re-run as edit_file(\"{command}\") to execute the deferred edit, \
                         or re-issue the original edit_file call with acknowledge_risk: true"
                    ),
                )
                .into());
            }
            let stored = ctx.output_buffer.get_dangerous(command).ok_or_else(|| {
                super::RecoverableError::with_hint(
                    "ack handle expired or unknown",
                    "Re-run the original command to get a fresh handle.",
                )
            })?;
            return run_command_inner(
                &stored.command,
                &stored.command,
                stored.timeout_secs,
                true, // acknowledge_risk
                stored.cwd.as_deref(),
                false, // buffer_only
                &root,
                &security,
                ctx,
            )
            .await;
        }

        // --- Step 1: Resolve @cmd_ buffer references ---
        let (resolved_command, temp_files, buffer_only, refreshed_handles) =
            ctx.output_buffer.resolve_refs(command)?;

        // Helper: run inner logic then always clean up temp files.
        let mut result = run_command_inner(
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

        // Inject refresh indicator into stdout when any @file_* handle was auto-refreshed.
        if !refreshed_handles.is_empty() {
            if let Ok(ref mut val) = result {
                let prefix: String = refreshed_handles
                    .iter()
                    .map(|id| {
                        format!(
                            "↻ {} refreshed from disk (file changed since last read)\n",
                            id
                        )
                    })
                    .collect();
                // Note: silently skips injection if "stdout" is absent (e.g. pending_ack
                // shape or buffered-output summary). These cases are extremely unlikely
                // to co-occur with a @file_* refresh, but worth noting.
                if let Some(stdout) = val["stdout"].as_str() {
                    val["stdout"] = serde_json::json!(format!("{}{}", prefix, stdout));
                }
            }
        }

        result
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_run_command(result))
    }
}

fn format_onboarding(result: &Value) -> String {
    let langs = result["languages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "?".to_string());
    let created = result["config_created"].as_bool().unwrap_or(false);
    let config_note = if created { " · config created" } else { "" };
    format!("[{langs}]{config_note}")
}

fn format_run_command(result: &Value) -> String {
    if result["output_id"].is_string() {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        let output_id = result["output_id"].as_str().unwrap_or("");
        match result["type"].as_str() {
            Some("test") => {
                let passed = result["passed"].as_u64().unwrap_or(0);
                let failed = result["failed"].as_u64().unwrap_or(0);
                let ignored = result["ignored"].as_u64().unwrap_or(0);
                let mut s = format!("{check} exit {exit} · {passed} passed");
                if failed > 0 {
                    s.push_str(&format!(" · {failed} FAILED"));
                }
                if ignored > 0 {
                    s.push_str(&format!(" · {ignored} ignored"));
                }
                s.push_str(&format!("  (query {output_id})"));
                s
            }
            Some("build") => {
                let errors = result["errors"].as_u64().unwrap_or(0);
                if errors > 0 {
                    format!("{check} exit {exit} · {errors} errors  (query {output_id})")
                } else {
                    format!("{check} exit {exit}  (query {output_id})")
                }
            }
            _ => format!("{check} exit {exit}  (query {output_id})"),
        }
    } else if result["timed_out"].as_bool().unwrap_or(false) {
        "✗ timed out".to_string()
    } else {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let stdout_lines = result["stdout"]
            .as_str()
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        format!("{check} exit {exit} · {stdout_lines} lines")
    }
}

/// Returns true when `command` is a bare `@ack_<8hex>` handle.
fn looks_like_ack_handle(command: &str) -> bool {
    let s = command.trim();
    if !s.starts_with("@ack_") {
        return false;
    }
    let suffix = &s[5..]; // after "@ack_"
    suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit())
}

/// Reassemble a buffered command summary with a stable, reader-friendly field order.
///
/// Dynamic field appending (`obj["key"] = val`) always places fields last, which
/// caused `output_id` (the buffer reference) to land after `stdout`/`failures`/
/// `first_error` (the bulk content). Correct order:
///   type → exit_code → output_id → [counts] → [content]
fn rebuild_buffered_summary(raw: Value, output_id: &str) -> Value {
    // These are large text fields — always go last.
    const CONTENT_FIELDS: &[&str] = &["stdout", "failures", "first_error"];

    let mut map = serde_json::Map::new();

    // 1. Status identity
    if let Some(v) = raw.get("type") {
        map.insert("type".into(), v.clone());
    }
    if let Some(v) = raw.get("exit_code") {
        map.insert("exit_code".into(), v.clone());
    }

    // 2. Buffer reference — most action-relevant, agent needs this to query results
    map.insert("output_id".into(), json!(output_id));

    // 3. Type-specific compact fields (counts, not content)
    let raw_obj = raw.as_object().expect("summary is always an object");
    for (k, v) in raw_obj {
        if !["type", "exit_code"].contains(&k.as_str()) && !CONTENT_FIELDS.contains(&k.as_str()) {
            map.insert(k.clone(), v.clone());
        }
    }

    // 4. Content fields last — bulk payload
    for field in CONTENT_FIELDS {
        if let Some(v) = raw_obj.get(*field) {
            map.insert((*field).into(), v.clone());
        }
    }

    Value::Object(map)
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
        count_lines, detect_command_type, detect_terminal_filter, needs_summary,
        summarize_build_output, summarize_generic, summarize_test_output, truncate_lines,
        truncate_lines_and_bytes, CommandType, BUFFER_QUERY_INLINE_CAP, SUMMARY_LINE_THRESHOLD,
    };
    use crate::util::path_security::is_dangerous_command;

    // --- Step 2: Dangerous command speed bump ---
    if !buffer_only && !acknowledge_risk {
        // Use resolved_command (with @refs substituted) so buffer-only grep/awk
        // commands don't get flagged for patterns in the buffer content.
        if let Some(reason) = is_dangerous_command(resolved_command, security) {
            let handle = ctx.output_buffer.store_dangerous(
                resolved_command.to_string(),
                cwd_param.map(str::to_string),
                timeout_secs,
            );
            return Ok(serde_json::json!({
                "pending_ack": handle,
                "reason": reason,
                "hint": format!("run_command(\"{handle}\") to execute")
            }));
        }
    }

    // --- Step 2.5: Source file access block ---
    if !buffer_only && !acknowledge_risk {
        if let Some(hint) = crate::util::path_security::check_source_file_access(resolved_command) {
            return Err(super::RecoverableError::with_hint(
                "shell access to source files is blocked",
                &hint,
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
        let under_project = canonical.starts_with(canonical_root.as_path());
        let under_tmp = canonical.starts_with("/tmp");
        if !under_project && !under_tmp {
            return Err(super::RecoverableError::with_hint(
                format!("cwd '{}' escapes project root", rel),
                "The cwd must be a subdirectory within the project, or a path under /tmp.",
            )
            .into());
        }
        canonical
    } else {
        root.to_path_buf()
    };

    // --- Step 4.5: Tee injection for terminal filter commands ---
    // When the last pipe stage is a known filter (grep, head, tail, sed, awk, etc.),
    // inject `tee /tmp/codescout-unfiltered-XXXX` before the filter so the caller
    // can surface the unfiltered stream as a buffer ref without re-running the command.

    // RAII guard: deletes the named tmpfile when dropped, ensuring cleanup on all
    // exit paths (success, error, and timeout arms of the match below).
    struct TmpfileGuard(String);
    impl Drop for TmpfileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let (effective_command, unfiltered_tmpfile): (String, Option<TmpfileGuard>) = if !buffer_only {
        if let Some(pipe_pos) = detect_terminal_filter(resolved_command) {
            // Use tempfile::NamedTempFile for unpredictable path (SF-3).
            // persist() converts it to a regular file we manage via TmpfileGuard.
            let named = tempfile::Builder::new()
                .prefix("codescout-unfiltered-")
                .tempfile()?;
            let tmppath = named.into_temp_path();
            let tmpfile = tmppath.to_string_lossy().to_string();
            // Keep the file on disk — TmpfileGuard handles cleanup.
            tmppath.keep()?;
            // Safety (SF-4): the path is generated by tempfile under $TMPDIR
            // and contains only alphanumeric chars, hyphens, and dots — no
            // shell metacharacters. We document this invariant rather than
            // adding a shell-escape dependency.
            debug_assert!(
                tmpfile.chars().all(|c| c.is_alphanumeric()
                    || c == '/'
                    || c == '-'
                    || c == '_'
                    || c == '.'),
                "tmpfile path contains unexpected characters: {tmpfile}"
            );
            let cmd = format!(
                "{} | tee {} | {}",
                resolved_command[..pipe_pos].trim_end(),
                tmpfile,
                resolved_command[pipe_pos + 1..].trim_start()
            );
            (cmd, Some(TmpfileGuard(tmpfile)))
        } else {
            (resolved_command.to_string(), None)
        }
    } else {
        (resolved_command.to_string(), None)
    };

    // --- Step 5: Execute command ---
    #[cfg(unix)]
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&effective_command)
        .current_dir(&work_dir)
        .output();

    #[cfg(windows)]
    let child = tokio::process::Command::new("cmd")
        .arg("/C")
        .arg(&effective_command)
        .current_dir(&work_dir)
        .output();

    // Heartbeat: send elapsed-seconds progress every 3s while the command runs.
    // AbortOnDrop guarantees the task is cancelled even when early `return`s fire.
    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let progress_clone = ctx.progress.clone();
    let _heartbeat = AbortOnDrop(tokio::spawn(async move {
        let start = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if let Some(p) = &progress_clone {
                p.report(start.elapsed().as_secs() as u32, None).await;
            }
        }
    }));

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child).await {
        Ok(Ok(output)) => {
            let raw_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let raw_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let exit_code = output.status.code().unwrap_or(-1);

            // --- Step 6.5: Read tee capture and store as unfiltered_output ref ---
            let unfiltered_ref: Option<(String, bool)> =
                if let Some(ref tmpfile) = unfiltered_tmpfile {
                    let capture = std::fs::read_to_string(&tmpfile.0).ok();
                    // tmpfile dropped at end of enclosing match arm — TmpfileGuard::drop() removes it
                    capture.map(|content| {
                        let line_count = count_lines(&content);
                        let (stored, truncated) = if line_count > SUMMARY_LINE_THRESHOLD {
                            let capped = content
                                .lines()
                                .take(SUMMARY_LINE_THRESHOLD)
                                .collect::<Vec<_>>()
                                .join("\n");
                            (capped, true)
                        } else {
                            (content, false)
                        };
                        let ref_id = ctx.output_buffer.store(
                            original_command.to_string(),
                            stored,
                            String::new(), // unfiltered capture is stdout-only; stderr belongs to the main buffer
                            exit_code,
                        );
                        (ref_id, truncated)
                    })
                } else {
                    None
                };

            // --- Step 6: Decide whether to buffer + summarize ---
            let mut result = if needs_summary(&raw_stdout, &raw_stderr) {
                // When the command was querying a buffer ref (e.g. `sed @cmd_A`),
                // creating a *new* buffer causes an infinite loop: the agent sees
                // a fresh ref, queries it again, gets another ref, and so on.
                // Break the cycle by returning an error that guides the agent
                // toward a more targeted query instead.
                if buffer_only {
                    // Truncate to BUFFER_QUERY_INLINE_CAP lines total (stderr priority: up to 20,
                    // remainder goes to stdout) and return inline. Do NOT create a new buffer
                    // ref — that would cause an infinite query loop.
                    const STDERR_BUDGET: usize = 20;
                    // For buffer-only commands (e.g. `cat @cmd_A`), the shell command
                    // produces empty stderr. Augment with the original buffer entry's
                    // stored stderr so the agent gets the full picture on replay.
                    let buffer_stderr: String = if raw_stderr.is_empty() {
                        original_command
                            .find("@cmd_")
                            .or_else(|| original_command.find("@file_"))
                            .and_then(|pos| {
                                original_command[pos..]
                                    .split_whitespace()
                                    .next()
                                    .and_then(|tok| ctx.output_buffer.get(tok))
                            })
                            .map(|e| e.stderr)
                            .unwrap_or_default()
                    } else {
                        raw_stderr.clone()
                    };
                    let stderr_budget = STDERR_BUDGET.min(count_lines(&buffer_stderr));
                    let stdout_budget = BUFFER_QUERY_INLINE_CAP - stderr_budget;

                    // Compute stderr first so we know its byte size for the stdout budget.
                    let (stderr_out, stderr_shown, stderr_total) =
                        truncate_lines(&buffer_stderr, STDERR_BUDGET);

                    // Byte budget: ensure the final JSON stays under TOOL_OUTPUT_BUFFER_THRESHOLD
                    // so call_content() does not immediately re-buffer the result as @tool_*.
                    // That re-buffering creates an infinite query loop:
                    //   grep @cmd_A → inline JSON → >10KB → @tool_B → jq @tool_B → same → @tool_C…
                    // Overhead ≈ 300 bytes for JSON keys, stderr content, and truncation fields.
                    const JSON_OVERHEAD: usize = 300;
                    let stdout_byte_budget = crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD
                        .saturating_sub(JSON_OVERHEAD)
                        .saturating_sub(stderr_out.len());

                    let (stdout_out, stdout_shown, stdout_total) =
                        truncate_lines_and_bytes(&raw_stdout, stdout_budget, stdout_byte_budget);

                    let was_truncated = stdout_shown < stdout_total || stderr_shown < stderr_total;

                    let mut result = json!({
                        "stdout": stdout_out,
                        "stderr": stderr_out,
                        "exit_code": exit_code,
                    });
                    if was_truncated {
                        result["truncated"] = json!(true);
                        result["stdout_shown"] = json!(stdout_shown);
                        result["stdout_total"] = json!(stdout_total);
                        if stderr_total > 0 {
                            result["stderr_shown"] = json!(stderr_shown);
                            result["stderr_total"] = json!(stderr_total);
                        }
                        let stderr_note = if stderr_total > 0 {
                            format!(", stderr {stderr_shown}/{stderr_total}")
                        } else {
                            String::new()
                        };
                        let next_start = stdout_shown + 1;
                        let next_end = stdout_shown + BUFFER_QUERY_INLINE_CAP;
                        result["hint"] = json!(format!(
                            "Output capped at {BUFFER_QUERY_INLINE_CAP} lines \
                             (stdout {stdout_shown}/{stdout_total}{stderr_note}). \
                             Next page: sed -n '{next_start},{next_end}p' @ref. \
                             Or grep 'keyword' @ref for targeted search.",
                        ));
                    }
                    // buffer_only => tee injection was skipped entirely (unfiltered_tmpfile is None),
                    // so no unfiltered_output field injection is needed before this early return.
                    return Ok(result);
                }

                let output_id = ctx.output_buffer.store(
                    original_command.to_string(),
                    raw_stdout.clone(),
                    raw_stderr.clone(),
                    exit_code,
                );

                let cmd_type = detect_command_type(original_command);
                let cmd_summary = match cmd_type {
                    CommandType::Test => summarize_test_output(&raw_stdout, &raw_stderr, exit_code),
                    CommandType::Build => {
                        summarize_build_output(&raw_stdout, &raw_stderr, exit_code)
                    }
                    CommandType::Generic => summarize_generic(&raw_stdout, &raw_stderr, exit_code),
                };

                // Rebuild with correct field order so output_id (the buffer reference
                // the agent needs) appears before content fields (stdout/failures/first_error).
                rebuild_buffered_summary(cmd_summary, &output_id)
            } else {
                // Short output — but for buffer-only queries, a single grep match
                // inside a compact-JSON @tool_* ref can be thousands of bytes even
                // with just 1 line.  That would push the result JSON over
                // TOOL_OUTPUT_BUFFER_THRESHOLD and cause call_content to store it
                // as a *new* @tool_* ref, creating an infinite query loop:
                //   grep @tool_A → giant line → @tool_B → read_file @tool_B → @tool_C…
                // Apply the same byte budget used in the needs_summary+buffer_only
                // path so that never happens.
                if buffer_only
                    && raw_stdout.len() + raw_stderr.len()
                        > crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD
                            .saturating_sub(300 /* JSON overhead */)
                {
                    const JSON_OVERHEAD: usize = 300;
                    let byte_budget = crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD
                        .saturating_sub(JSON_OVERHEAD)
                        .saturating_sub(raw_stderr.len());
                    let (stdout_out, stdout_shown, stdout_total) =
                        truncate_lines_and_bytes(&raw_stdout, BUFFER_QUERY_INLINE_CAP, byte_budget);
                    let mut r = json!({
                        "stdout": stdout_out,
                        "stderr": raw_stderr,
                        "exit_code": exit_code,
                    });
                    if stdout_shown < stdout_total {
                        r["truncated"] = json!(true);
                        r["hint"] = json!(
                            "Match truncated: a single grep match inside a @tool_* ref \
                             contains compact JSON (one very long line). \
                             Use read_file(@tool_abc, json_path=\"$.field\") to extract \
                             a specific field, or read_file(@tool_abc, start_line=N, \
                             end_line=M) to browse sections of the pretty-printed result."
                        );
                    }
                    r
                } else {
                    json!({
                        "stdout": raw_stdout,
                        "stderr": raw_stderr,
                        "exit_code": exit_code,
                    })
                }
            };

            // Attach unfiltered_output ref if we captured via tee
            if let Some((ref ref_id, truncated)) = unfiltered_ref {
                result["unfiltered_output"] = json!(ref_id);
                if truncated {
                    result["unfiltered_truncated"] = json!(true);
                }
            }

            Ok(result)
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
    use crate::tools::command_summary::{BUFFER_QUERY_INLINE_CAP, SUMMARY_LINE_THRESHOLD};
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
                progress: None,
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
            progress: None,
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

    #[tokio::test]
    async fn onboarding_status_includes_private_memories_when_present() {
        let (_dir, ctx) = project_ctx().await;

        // Run full onboarding first (creates config + onboarding memory)
        Onboarding.call(json!({}), &ctx).await.unwrap();

        // Seed a private memory
        ctx.agent
            .with_project(|p| p.private_memory.write("my-prefs", "verbose"))
            .await
            .unwrap();

        // Fast-path status call should include private memories
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        assert!(result["onboarded"].as_bool().unwrap_or(false));
        let private = result["private_memories"].as_array().unwrap();
        assert!(private.iter().any(|v| v.as_str() == Some("my-prefs")));
        assert!(result["message"].as_str().unwrap().contains("my-prefs"));
    }

    #[tokio::test]
    async fn onboarding_status_omits_private_memories_field_when_empty() {
        let (_dir, ctx) = project_ctx().await;

        // Run full onboarding first (creates config + onboarding memory), no private memory
        Onboarding.call(json!({}), &ctx).await.unwrap();

        // Fast-path status call should NOT include private_memories field
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        assert!(result["onboarded"].as_bool().unwrap_or(false));
        assert!(result["private_memories"].is_null());
        assert!(!result["message"].as_str().unwrap().contains("private"));
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
        let result = RunCommand
            .call(
                json!({ "command": "seq 1 100000", "timeout_secs": 10 }),
                &ctx,
            )
            .await
            .unwrap();
        // Large output is buffered, not byte-truncated.
        assert!(
            result["output_id"].as_str().is_some(),
            "large output should be buffered with output_id"
        );
        assert!(result["hint"].is_null(), "hint field should be absent");
        assert!(
            result["total_stdout_lines"].is_null(),
            "total_stdout_lines should be absent"
        );
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
    async fn run_command_does_not_include_warning() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({ "command": "echo test", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        assert!(
            result["warning"].is_null(),
            "run_command should not emit a warning field"
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
            progress: None,
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
            progress: None,
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

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_dangerous_blocked_without_acknowledge() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(
                json!({ "command": "rm -rf /tmp/codescout_test_nonexistent" }),
                &ctx,
            )
            .await
            .expect("dangerous command should return Ok with pending_ack");
        // Now returns a pending_ack handle instead of an error
        assert!(
            result.get("pending_ack").is_some(),
            "should have pending_ack key: {:?}",
            result
        );
        assert!(
            result["pending_ack"].as_str().unwrap().starts_with("@ack_"),
            "pending_ack should start with @ack_: {:?}",
            result["pending_ack"]
        );
        assert!(result.get("reason").is_some(), "should have reason key");
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
            .await
            .expect("dangerous command should return Ok with pending_ack, not Err");
        // Previously returned Err(RecoverableError); now returns Ok with a pending_ack handle.
        assert!(
            result.get("pending_ack").is_some(),
            "should have pending_ack key: {:?}",
            result
        );
        assert!(
            result["pending_ack"].as_str().unwrap().starts_with("@ack_"),
            "pending_ack should start with @ack_: {:?}",
            result["pending_ack"]
        );
        assert!(
            result.get("reason").is_some(),
            "should have reason key: {:?}",
            result
        );
        assert!(
            result.get("hint").is_some(),
            "should have hint key: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn dangerous_command_returns_ack_handle() {
        let (dir, ctx) = project_ctx().await;
        let root = dir.path().to_path_buf();
        let security = Default::default();
        let result = run_command_inner(
            "rm -rf /dist",
            "rm -rf /dist",
            30,
            false, // acknowledge_risk
            None,  // cwd_param
            false, // buffer_only
            &root,
            &security,
            &ctx,
        )
        .await
        .expect("should return Ok with pending_ack, not Err");

        assert!(
            result.get("pending_ack").is_some(),
            "should have pending_ack key"
        );
        assert!(
            result["pending_ack"].as_str().unwrap().starts_with("@ack_"),
            "pending_ack should start with @ack_: {:?}",
            result["pending_ack"]
        );
        assert!(result.get("reason").is_some(), "should have reason key");
        assert!(result.get("hint").is_some(), "should have hint key");
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
    async fn run_command_blocks_cat_on_source_file() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({"command": "cat src/main.rs"}), &ctx)
            .await;
        let err = result.unwrap_err();
        let rec = err
            .downcast_ref::<crate::tools::RecoverableError>()
            .expect("should be a RecoverableError");
        assert!(
            rec.message.contains("source files is blocked"),
            "expected source-file block message, got: {}",
            rec.message
        );
    }

    #[tokio::test]
    async fn run_command_source_block_bypassed_with_acknowledge_risk() {
        let (dir, ctx) = project_ctx().await;
        std::fs::write(dir.path().join("tiny.rs"), "fn main() {}\n").unwrap();
        let result = RunCommand
            .call(
                json!({"command": "cat tiny.rs", "acknowledge_risk": true}),
                &ctx,
            )
            .await;
        assert!(
            result.is_ok(),
            "acknowledge_risk should bypass source block"
        );
    }

    #[tokio::test]
    async fn run_command_source_block_not_triggered_for_markdown() {
        let (dir, ctx) = project_ctx().await;
        std::fs::write(dir.path().join("README.md"), "# hello\n").unwrap();
        let result = RunCommand
            .call(json!({"command": "cat README.md"}), &ctx)
            .await;
        assert!(result.is_ok(), "cat on markdown should not be blocked");
    }

    #[tokio::test]
    async fn run_command_source_block_not_triggered_for_non_source() {
        let (dir, ctx) = project_ctx().await;
        std::fs::write(dir.path().join("data.txt"), "hello\n").unwrap();
        let result = RunCommand
            .call(json!({"command": "cat data.txt"}), &ctx)
            .await;
        assert!(result.is_ok(), "cat on .txt should not be blocked");
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
        // Use /var — it always exists, is outside any temp project root, and is
        // not under /tmp (which is now an allowed cwd root).
        let result = RunCommand
            .call(json!({"command": "ls", "cwd": "/var"}), &ctx)
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

    // Task 4 TDD regression tests — buffer-backed smart summaries + buffer ref execution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_command_short_output_returned_directly() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(
            result.get("output_id").is_none(),
            "short output should not buffer: got output_id {:?}",
            result.get("output_id")
        );
        assert!(
            result["stdout"].as_str().unwrap().contains("hello"),
            "stdout should contain 'hello': {:?}",
            result["stdout"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_large_output_stored_in_buffer() {
        let (_dir, ctx) = project_ctx().await;
        let result = RunCommand
            .call(json!({"command": "seq 1 100"}), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"]
            .as_str()
            .expect("large output should have output_id");
        assert!(
            output_id.starts_with("@cmd_"),
            "output_id should start with @cmd_: {}",
            output_id
        );
        assert!(result["hint"].is_null(), "hint field should be absent");
        assert!(
            result["total_stdout_lines"].is_null(),
            "total_stdout_lines should be absent"
        );
        let entry = ctx.output_buffer.get(output_id).unwrap();
        assert!(
            entry.stdout.contains("50\n"),
            "buffered stdout should contain '50\\n'"
        );
        assert!(
            entry.stdout.contains("100\n"),
            "buffered stdout should contain '100\\n'"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_ref_executes_correctly() {
        let (_dir, ctx) = project_ctx().await;
        let r1 = RunCommand
            .call(json!({"command": "seq 1 100"}), &ctx)
            .await
            .unwrap();
        let output_id = r1["output_id"].as_str().unwrap();
        let r2 = RunCommand
            .call(
                json!({"command": format!("grep '^50$' {}", output_id)}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(r2["exit_code"], 0, "grep should find '50': {:?}", r2);
        assert_eq!(
            r2["stdout"].as_str().unwrap().trim(),
            "50",
            "stdout should be exactly '50'"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_above_threshold_truncates_inline() {
        // BUFFER_QUERY_INLINE_CAP + 1 lines — strictly above the inline cap.
        // Must return Ok with truncated content, NOT an error or a new buffer ref.
        let (_dir, ctx) = project_ctx().await;
        let content: String = (1..=BUFFER_QUERY_INLINE_CAP + 1)
            .map(|i| format!("{i}\n"))
            .collect();
        let id = ctx.output_buffer.store("cmd".into(), content, "".into(), 0);
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected Ok with truncated inline output");
        assert_eq!(
            result["truncated"], true,
            "should be truncated: {:?}",
            result
        );
        assert_eq!(
            result["stdout_shown"], BUFFER_QUERY_INLINE_CAP,
            "stdout_shown should equal inline cap: {:?}",
            result
        );
        assert_eq!(
            result["stdout_total"],
            BUFFER_QUERY_INLINE_CAP + 1,
            "stdout_total should be full count: {:?}",
            result
        );
        assert!(
            result.get("output_id").is_none(),
            "must not create a new buffer ref: {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_at_threshold_returns_inline() {
        // Exactly SUMMARY_LINE_THRESHOLD lines — the check is `>` not `>=`, so this
        // must return content inline, not error.
        let (_dir, ctx) = project_ctx().await;
        let content: String = (1..=SUMMARY_LINE_THRESHOLD)
            .map(|i| format!("{i}\n"))
            .collect();
        let id = ctx.output_buffer.store("cmd".into(), content, "".into(), 0);
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected inline output at threshold");
        assert!(
            result.get("stdout").is_some(),
            "expected stdout field: {:?}",
            result
        );
        assert!(
            result.get("output_id").is_none(),
            "should not be buffered: {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_large_single_line_does_not_rebuffer() {
        // Regression: grep on a @tool_* ref returns the entire compact-JSON blob as
        // one line.  Even with line count = 1 (< SUMMARY_LINE_THRESHOLD), the byte
        // size can exceed TOOL_OUTPUT_BUFFER_THRESHOLD.  The result must be truncated
        // inline — never stored as a new @tool_* ref (which would create an infinite
        // query loop: grep @tool_A → @tool_B → grep @tool_B → @tool_C…).
        let (_dir, ctx) = project_ctx().await;

        // Create a @cmd_* buffer whose content is one very long line (>5 KB).
        let long_line = "x".repeat(crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD + 1000);
        let id = ctx
            .output_buffer
            .store("cmd".into(), long_line, "".into(), 0);

        // cat @cmd_* triggers buffer_only; the single-line stdout exceeds the byte budget.
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("should return truncated inline result, not error");

        // Must be inline (no output_id) and must be truncated with a hint.
        assert!(
            result.get("output_id").is_none(),
            "must not create new buffer ref: {:?}",
            result
        );
        assert!(
            result.get("stdout").is_some(),
            "must have stdout field: {:?}",
            result
        );
        assert_eq!(
            result.get("truncated").and_then(|v| v.as_bool()),
            Some(true),
            "must be marked truncated: {:?}",
            result
        );
        let hint = result["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("read_file"),
            "hint should guide to read_file: {}",
            hint
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_large_output_no_new_ref() {
        // Regression: `sed @cmd_A` that reproduces a large buffer must
        // return truncated inline content, NOT a new @cmd_B reference.
        // Use 150 lines (> BUFFER_QUERY_INLINE_CAP=100) to trigger truncation.
        let (_dir, ctx) = project_ctx().await;

        let large_content: String = (1..=250).map(|i| format!("{i}\n")).collect();
        let id = ctx
            .output_buffer
            .store("original_cmd".into(), large_content, "".into(), 0);

        let result = RunCommand
            .call(
                json!({ "command": format!("sed -n '1,250p' {}", id) }),
                &ctx,
            )
            .await
            .expect("expected Ok with truncated inline output");

        assert!(
            result.get("output_id").is_none(),
            "must not create a new buffer ref: {:?}",
            result
        );
        assert_eq!(
            result["truncated"], true,
            "should be truncated: {:?}",
            result
        );
        assert_eq!(
            result["stdout_total"], 250usize,
            "stdout_total: {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_long_lines_fit_under_threshold() {
        // Regression: buffer-only queries with long lines (e.g. Java/Kotlin log output
        // with timestamps and class names, ~200 chars/line) must produce a response JSON
        // that stays under TOOL_OUTPUT_BUFFER_THRESHOLD.  Before the fix, a 100-line cap
        // on 200-char lines produced ~20 KB of stdout, which call_content() re-buffered
        // as @tool_* — creating an infinite query loop:
        //   grep @cmd_A → inline JSON (>10KB) → @tool_B → jq @tool_B → same → @tool_C…
        let (_dir, ctx) = project_ctx().await;

        // 200-char lines: typical Java log output with timestamp + class + message.
        let long_line = "x".repeat(200);
        let content: String = (0..=BUFFER_QUERY_INLINE_CAP)
            .map(|_| format!("{long_line}\n"))
            .collect();
        let id = ctx.output_buffer.store("cmd".into(), content, "".into(), 0);

        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected Ok");

        // Core assertion: the serialized JSON must fit under the re-buffering threshold.
        let json_size = serde_json::to_string(&result).unwrap().len();
        assert!(
            json_size <= crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD,
            "buffer_only response ({json_size} bytes) must not exceed TOOL_OUTPUT_BUFFER_THRESHOLD \
             ({} bytes) — would cause infinite @tool_* re-buffering loop",
            crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD,
        );

        // Must also avoid creating a new buffer ref.
        assert!(
            result.get("output_id").is_none(),
            "must not create a new buffer ref: {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_stderr_gets_priority() {
        // stderr = 25 lines (> 20 cap) + stdout = 250 lines (> remaining budget).
        // Expected: stderr_shown = 20, stdout_shown = 80 (BUFFER_QUERY_INLINE_CAP - 20).
        let (_dir, ctx) = project_ctx().await;
        let stdout: String = (1..=250).map(|i| format!("out{i}\n")).collect();
        let stderr: String = (1..=25).map(|i| format!("err{i}\n")).collect();
        let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected Ok");
        assert_eq!(
            result["stderr_shown"], 20usize,
            "stderr_shown: {:?}",
            result
        );
        assert_eq!(
            result["stderr_total"], 25usize,
            "stderr_total: {:?}",
            result
        );
        assert_eq!(
            result["stdout_shown"],
            BUFFER_QUERY_INLINE_CAP - 20,
            "stdout_shown: {:?}",
            result
        );
        assert_eq!(
            result["stdout_total"], 250usize,
            "stdout_total: {:?}",
            result
        );
        assert_eq!(result["truncated"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_short_stderr_gives_budget_to_stdout() {
        // stderr = 10 lines (< 20 cap) + stdout = 250 lines (> remaining budget).
        // Expected: stderr_shown = 10, stdout_shown = 90 (BUFFER_QUERY_INLINE_CAP - 10).
        let (_dir, ctx) = project_ctx().await;
        let stdout: String = (1..=250).map(|i| format!("out{i}\n")).collect();
        let stderr: String = (1..=10).map(|i| format!("err{i}\n")).collect();
        let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected Ok");
        assert_eq!(
            result["stdout_shown"],
            BUFFER_QUERY_INLINE_CAP - 10,
            "stdout_shown: {:?}",
            result
        );
        assert_eq!(
            result["stdout_total"], 250usize,
            "stdout_total: {:?}",
            result
        );
        assert_eq!(result["truncated"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffer_only_within_limit_no_truncation_fields() {
        // combined = 45 lines (< 50 threshold) — must NOT add truncated/shown/total fields.
        // needs_summary returns false, so we fall through to the short-output branch.
        let (_dir, ctx) = project_ctx().await;
        let stdout: String = (1..=30).map(|i| format!("out{i}\n")).collect();
        let stderr: String = (1..=15).map(|i| format!("err{i}\n")).collect();
        let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .expect("expected Ok");
        assert!(
            result.get("truncated").is_none(),
            "no truncated field: {:?}",
            result
        );
        assert!(
            result.get("stdout_shown").is_none(),
            "no stdout_shown: {:?}",
            result
        );
        assert!(
            result.get("output_id").is_none(),
            "no buffer ref: {:?}",
            result
        );
    }

    #[test]
    fn language_hints_covers_main_languages() {
        for lang in &[
            "rust",
            "python",
            "typescript",
            "javascript",
            "go",
            "java",
            "kotlin",
            "c",
            "cpp",
            "tsx",
            "jsx",
        ] {
            assert!(
                language_navigation_hints(lang).is_some(),
                "expected hints for '{}'",
                lang
            );
        }
    }

    #[test]
    fn language_hints_returns_none_for_unsupported() {
        // "bash" and "markdown" are real detect_language() values, just without hints
        assert!(language_navigation_hints("markdown").is_none());
        assert!(language_navigation_hints("bash").is_none());
        assert!(language_navigation_hints("unknown_lang").is_none());
    }

    #[test]
    fn system_prompt_draft_includes_language_hints() {
        let langs = vec!["rust".to_string(), "python".to_string()];
        let draft = build_system_prompt_draft(&langs, &[], None);
        assert!(
            draft.contains("## Language Navigation"),
            "should have Language Navigation section"
        );
        assert!(draft.contains("**rust:**"), "should have rust hints");
        assert!(draft.contains("**python:**"), "should have python hints");
        assert!(
            draft.contains("name_path"),
            "hints should mention name_path"
        );
    }

    #[test]
    fn system_prompt_draft_omits_hints_for_unsupported_languages() {
        let langs = vec!["markdown".to_string()];
        let draft = build_system_prompt_draft(&langs, &[], None);
        assert!(
            !draft.contains("## Language Navigation"),
            "should not have Language Navigation for markdown-only"
        );
    }

    #[test]
    fn system_prompt_draft_isolates_hints_per_language() {
        let langs = vec!["python".to_string()];
        let draft = build_system_prompt_draft(&langs, &[], None);
        assert!(draft.contains("**python:**"), "should have python hints");
        assert!(
            !draft.contains("impl Trait for Type"),
            "rust hints should not leak into python-only draft"
        );
    }

    #[test]
    fn system_prompt_draft_includes_private_memory_rules() {
        let draft = build_system_prompt_draft(&[], &[], None);
        assert!(
            draft.contains("Private Memory Rules"),
            "draft should include Private Memory Rules section"
        );
        assert!(
            draft.contains("private=true"),
            "draft should mention private=true parameter"
        );
        assert!(
            draft.contains("gitignored"),
            "draft should explain that private memories are gitignored"
        );
        // Ensure string continuation escapes did not embed leading whitespace.
        // Each bullet line must start with '-', not with spaces.
        for line in draft.lines() {
            if line.contains("Personal preferences") || line.contains("Machine-specific config") {
                assert!(
                    line.starts_with('-'),
                    "bullet line must not have leading spaces; got: {:?}",
                    line
                );
            }
        }
    }

    #[test]
    fn run_command_format_compact_test_result() {
        let tool = RunCommand;
        let result = json!({
            "type": "test", "exit_code": 0,
            "passed": 533, "failed": 0, "ignored": 0,
            "output_id": "@cmd_abc123"
        });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("533"), "got: {text}");
        assert!(text.contains("passed"), "got: {text}");
    }

    #[test]
    fn run_command_format_compact_short_output() {
        let tool = RunCommand;
        let result = json!({ "stdout": "hello\nworld", "stderr": "", "exit_code": 0 });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("exit 0"), "got: {text}");
    }

    // Fix A: buffer-only queries should use BUFFER_QUERY_INLINE_CAP (200), not
    // SUMMARY_LINE_THRESHOLD (50). A 100-line result should be returned fully inline.
    #[tokio::test]
    async fn buffer_query_returns_up_to_200_lines_inline() {
        let (_dir, ctx) = project_ctx().await;
        // Create a buffer with 100 lines (> SUMMARY_LINE_THRESHOLD=50, so it buffers)
        let result = RunCommand
            .call(json!({ "command": "seq 1 100", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"].as_str().unwrap().to_string();

        // Query the buffer — 100 lines is within the 200-line inline cap
        let query = format!("cat {output_id}");
        let result2 = RunCommand
            .call(json!({ "command": query, "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let stdout = result2["stdout"].as_str().unwrap_or("");
        let line_count = stdout.lines().count();
        assert_eq!(
            line_count, 100,
            "buffer query of 100 lines should return all 100 inline (got {line_count})"
        );
        assert!(
            result2["truncated"].is_null(),
            "should not be truncated when within 200-line cap"
        );
    }

    // Fix B: the truncation hint for buffer queries should show the *next* page range,
    // not always start from line 1.
    #[tokio::test]
    async fn buffer_query_truncation_hint_shows_next_page() {
        let (_dir, ctx) = project_ctx().await;
        // Create a buffer with 150 lines (> BUFFER_QUERY_INLINE_CAP=100)
        let result = RunCommand
            .call(json!({ "command": "seq 1 300", "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let output_id = result["output_id"].as_str().unwrap().to_string();

        // Query it — output exceeds 100-line cap, so hint should show next-page command
        let query = format!("cat {output_id}");
        let result2 = RunCommand
            .call(json!({ "command": query, "timeout_secs": 5 }), &ctx)
            .await
            .unwrap();
        let hint = result2["hint"].as_str().unwrap_or("");
        // Hint must guide to the NEXT page (line 101 onwards), not back to line 1
        assert!(
            hint.contains("101"),
            "hint should show next-page start (101), got: {hint}"
        );
        assert!(
            !hint.contains("'1,"),
            "hint must not restart from line 1, got: {hint}"
        );
    }

    // Fix C: when the first run_command looks like a plain file read (cat file),
    // the buffer creation hint should suggest read_file as an alternative.
    #[tokio::test]
    async fn cat_file_no_hint_field() {
        let (dir, ctx) = project_ctx().await;
        let md_path = dir.path().join("big_plan.md");
        let content: String = (1..=60).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&md_path, content).unwrap();

        let result = RunCommand
            .call(
                json!({ "command": "cat big_plan.md", "timeout_secs": 5 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result["hint"].is_null(), "hint field should be absent");
    }

    #[tokio::test]
    async fn ack_handle_executes_stored_command() {
        let (_dir, ctx) = project_ctx().await;
        let handle = ctx
            .output_buffer
            .store_dangerous("echo hello_ack".to_string(), None, 30);

        let tool = RunCommand;
        let input = serde_json::json!({ "command": handle });
        let result = tool
            .call(input, &ctx)
            .await
            .expect("ack call should succeed");

        let stdout = result["stdout"].as_str().unwrap_or("");
        assert!(
            stdout.contains("hello_ack"),
            "expected 'hello_ack' in stdout, got: {stdout}"
        );
    }

    #[tokio::test]
    async fn ack_handle_unknown_returns_recoverable_error() {
        let (_dir, ctx) = project_ctx().await;
        let tool = RunCommand;
        let input = serde_json::json!({ "command": "@ack_deadbeef" });
        let err = tool
            .call(input, &ctx)
            .await
            .expect_err("unknown ack handle should return Err");
        assert!(
            err.to_string().contains("expired"),
            "error should mention 'expired', got: {err}"
        );
    }

    #[tokio::test]
    async fn run_command_rejects_edit_file_ack_handle_with_clear_error() {
        // A handle stored by edit_file (pending_edits) must not silently appear "expired"
        // when passed to run_command — it should produce a targeted cross-tool error.
        let (_dir, ctx) = project_ctx().await;
        let handle = ctx.output_buffer.store_pending_edit(
            "src/lib.rs".to_string(),
            "old\nline".to_string(),
            "new\nline".to_string(),
            false,
        );

        let err = RunCommand
            .call(serde_json::json!({ "command": handle }), &ctx)
            .await
            .expect_err("edit_file ack passed to run_command should return Err");

        let msg = err.to_string();
        assert!(
            msg.contains("edit_file"),
            "error should name the correct tool, got: {msg}"
        );
        assert!(
            !msg.contains("expired"),
            "should not say 'expired' (that implies a run_command ack), got: {msg}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_prepends_refresh_indicator_for_stale_file_handle() {
        use std::fs;
        let (dir, ctx) = project_ctx().await;

        let path = dir.path().join("data.txt");
        fs::write(&path, "original").unwrap();
        let id = ctx
            .output_buffer
            .store_file(path.to_string_lossy().to_string(), "original".to_string());

        // Make the file look newer than the cached entry
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(future)).unwrap();

        let result = RunCommand
            .call(json!({ "command": format!("cat {}", id) }), &ctx)
            .await
            .unwrap();

        let stdout = result["stdout"].as_str().unwrap();
        assert!(
            stdout.starts_with(&format!("↻ {} refreshed from disk", id)),
            "expected refresh indicator, got: {:?}",
            stdout
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_buffered_output_has_output_id_before_stdout() {
        // Regression: output_id (the buffer reference the agent needs to query results)
        // was appended dynamically after the summary object was built, placing it AFTER
        // stdout/content fields. It must appear before content.
        let (_dir, ctx) = project_ctx().await;
        // seq 100 produces 100 lines, exceeding SUMMARY_LINE_THRESHOLD (50) to trigger buffering.
        let result = RunCommand
            .call(json!({ "command": "seq 100" }), &ctx)
            .await
            .unwrap();

        assert!(
            result["output_id"].is_string(),
            "expected buffered output (output_id present) for 100-line command, got: {result:?}"
        );

        let keys: Vec<&str> = result
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();

        let output_id_pos = keys.iter().position(|k| *k == "output_id").unwrap();
        // stdout is the content field in generic summaries; failures/first_error in others.
        // We assert output_id appears before any content-heavy field.
        let stdout_pos = keys
            .iter()
            .position(|k| *k == "stdout")
            .unwrap_or(keys.len());

        assert!(
            output_id_pos < stdout_pos,
            "output_id must appear before stdout (content payload), got key order: {keys:?}"
        );
    }

    #[tokio::test]
    async fn piped_grep_returns_unfiltered_ref() {
        let (dir, ctx) = project_ctx().await;
        // Create a file with several lines; grep for just one
        std::fs::write(
            dir.path().join("items.txt"),
            "apple\nbanana\ncherry\ndates\nelderberry\n",
        )
        .unwrap();
        let result = RunCommand
            .call(json!({ "command": "cat items.txt | grep apple" }), &ctx)
            .await
            .unwrap();

        // unfiltered_output ref should be present
        assert!(
            result["unfiltered_output"].is_string(),
            "expected unfiltered_output field, got: {result}"
        );
        let ref_id = result["unfiltered_output"].as_str().unwrap();

        // Query the buffer: full content should include banana (filtered out by grep)
        let full = RunCommand
            .call(json!({ "command": format!("cat {ref_id}") }), &ctx)
            .await
            .unwrap();
        let stdout = full["stdout"].as_str().unwrap_or("");
        assert!(
            stdout.contains("banana"),
            "unfiltered output missing 'banana': {stdout}"
        );
        assert!(
            stdout.contains("apple"),
            "unfiltered output missing 'apple': {stdout}"
        );
    }

    #[tokio::test]
    async fn non_filter_pipe_no_unfiltered_ref() {
        let (_dir, ctx) = project_ctx().await;
        // Second stage is not a known filter — no unfiltered_output
        let result = RunCommand
            .call(json!({ "command": "echo hello | cat" }), &ctx)
            .await
            .unwrap();
        assert!(
            result.get("unfiltered_output").is_none(),
            "unexpected unfiltered_output for non-filter pipe: {result}"
        );
    }

    #[tokio::test]
    async fn unfiltered_truncated_when_over_threshold() {
        use crate::tools::command_summary::SUMMARY_LINE_THRESHOLD;
        let (dir, ctx) = project_ctx().await;
        // Write SUMMARY_LINE_THRESHOLD + 10 lines; grep for just one
        let line_count = SUMMARY_LINE_THRESHOLD + 10;
        let content: String = (0..line_count).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();
        let result = RunCommand
            .call(json!({ "command": "cat big.txt | grep line0" }), &ctx)
            .await
            .unwrap();
        // truncated flag should be set (SUMMARY_LINE_THRESHOLD + 10 lines > SUMMARY_LINE_THRESHOLD)
        assert_eq!(
            result["unfiltered_truncated"],
            json!(true),
            "expected truncated flag: {result}"
        );
    }
}
