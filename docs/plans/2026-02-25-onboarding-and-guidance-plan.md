# Onboarding & Claude Code Guidance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add rich server instructions and an enhanced onboarding prompt so Claude Code uses code-explorer's semantic tools effectively instead of falling back to raw Read/Grep/Glob.

**Architecture:** Two prompt layers compiled into binary via `include_str!`. Server instructions are dynamically generated (static template + project state) in `get_info()`. Onboarding tool returns an instruction prompt alongside its existing JSON data. `check_onboarding_performed` returns memory list with read-on-demand guidance.

**Tech Stack:** Rust, rmcp (MCP), include_str! for markdown templates, existing Agent/MemoryStore APIs.

---

### Task 1: Create the prompts module with server instructions template

**Files:**
- Create: `src/prompts/mod.rs`
- Create: `src/prompts/server_instructions.md`
- Modify: `src/main.rs` (add `mod prompts;` to module tree — check if modules are declared there or in `lib.rs`)

**Step 1: Check where modules are declared**

Read `src/main.rs` and `src/lib.rs` (if it exists) to find where `mod` declarations live.

Run: `head -20 src/main.rs && ls src/lib.rs 2>/dev/null`

**Step 2: Create the server instructions markdown template**

Create `src/prompts/server_instructions.md`:

```markdown
code-explorer MCP server: high-performance semantic code intelligence for Claude Code.

## How to Explore Code

You have access to semantic tools that understand code structure. PREFER these over Claude Code's built-in Read/Grep/Glob for source files.

### Symbol-Level Navigation (most token-efficient)
- `find_symbol(pattern)` — find functions, classes, methods by name substring
- `get_symbols_overview(path)` — see all symbols in a file or directory (like a table of contents)
- `find_referencing_symbols(name_path, file)` — find all usages of a symbol across the project
- `list_functions(path)` — quick function/method signatures via tree-sitter (no LSP needed)

### Reading Source Code
- `find_symbol(pattern, include_body=true)` — read a specific symbol's full source code
- `get_symbols_overview(path, depth=1)` — see structure with direct children before diving in
- `read_file(path, start_line, end_line)` — targeted line ranges when you already know where to look

### Discovery & Search
- `semantic_search(query)` — find code by natural language description ("how are errors handled")
- `search_for_pattern(pattern)` — regex search across the project (for literal strings, config values)
- `find_file(pattern)` — find files by glob pattern (e.g. "**/*.rs", "src/**/mod.rs")

### Editing Code
- `replace_symbol_body(name_path, file, new_body)` — replace a function/method body
- `insert_before_symbol(name_path, file, code)` — insert code before a symbol
- `insert_after_symbol(name_path, file, code)` — insert code after a symbol
- `rename_symbol(name_path, file, new_name)` — rename across the codebase
- `replace_content(path, old, new)` — find-and-replace text in a file
- `create_text_file(path, content)` — create or overwrite a file

### Git Integration
- `git_blame(path)` — who changed each line and when
- `git_log(path?)` — commit history for a file or the whole project
- `git_diff(commit?, path?)` — show uncommitted changes or diff against a commit

### Project Memory
- `write_memory(topic, content)` — persist knowledge about the project
- `read_memory(topic)` — retrieve a stored memory entry
- `list_memories()` — see all available memory topics
- `delete_memory(topic)` — remove a memory entry

## Workflow Patterns

### Understand Before Editing
1. `get_symbols_overview(file)` — see what's in the file
2. `find_symbol(name, include_body=true)` — read the specific symbol you need
3. Edit using `replace_symbol_body` or `insert_after_symbol`

### Find Usages Before Refactoring
1. `find_symbol(name)` — locate the symbol definition
2. `find_referencing_symbols(name_path, file)` — find all references
3. `rename_symbol(name_path, file, new_name)` — safe cross-project rename

### Discover Then Drill Down
1. `semantic_search("how does X work")` — find relevant code by intent
2. `get_symbols_overview(found_file)` — understand the file structure
3. `find_symbol(specific_name, include_body=true)` — read the details

### Explore Unfamiliar Code
1. `list_dir(path, recursive=false)` — see directory structure
2. `get_symbols_overview(interesting_file)` — map the symbols
3. `find_symbol(key_type, include_body=true)` — read core abstractions
4. `find_referencing_symbols(key_type, file)` — see how it's used

## Rules

- PREFER `get_symbols_overview` + `find_symbol(include_body=true)` over reading entire source files
- Use `read_file` for non-code files (README, configs, docs, TOML, JSON, YAML) or targeted line ranges
- Use `semantic_search` for "how does X work?" questions; use `find_symbol` for "where is X defined?"
- Use `list_functions` for a quick overview when you just need signatures, not full symbol trees
- Use `extract_docstrings` to understand a file's API documentation
```

**Step 3: Create the prompts module**

Create `src/prompts/mod.rs`:

```rust
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
        instructions.push_str(&format!("- **Project:** {} at `{}`\n", status.name, status.path));
        if !status.languages.is_empty() {
            instructions.push_str(&format!("- **Languages:** {}\n", status.languages.join(", ")));
        }
        if !status.memories.is_empty() {
            instructions.push_str(&format!(
                "- **Available memories:** {} — use `read_memory(topic)` to read relevant ones as needed for your current task\n",
                status.memories.join(", ")
            ));
        } else {
            instructions.push_str("- **Memories:** None yet — run `onboarding` to create project memories\n");
        }
        if status.has_index {
            instructions.push_str("- **Semantic index:** Built — `semantic_search` is ready to use\n");
        } else {
            instructions.push_str("- **Semantic index:** Not built — run `index_project` to enable `semantic_search`\n");
        }
    }

    instructions
}

/// Dynamic project status used to build server instructions.
pub struct ProjectStatus {
    pub name: String,
    pub path: String,
    pub languages: Vec<String>,
    pub memories: Vec<String>,
    pub has_index: bool,
}
```

**Step 4: Register the module**

Add `pub mod prompts;` to the module tree (in `src/lib.rs` or `src/main.rs`, wherever the other `pub mod` declarations live).

**Step 5: Build to verify compilation**

Run: `cargo build 2>&1 | tail -20`
Expected: Compiles successfully.

**Step 6: Commit**

```bash
git add src/prompts/
git add src/lib.rs  # or src/main.rs
git commit -m "feat: add prompts module with server instructions template"
```

---

### Task 2: Write tests for `build_server_instructions`

**Files:**
- Modify: `src/prompts/mod.rs` (add tests)

**Step 1: Write the tests**

Add to the bottom of `src/prompts/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_instructions_contain_key_sections() {
        assert!(SERVER_INSTRUCTIONS.contains("## How to Explore Code"));
        assert!(SERVER_INSTRUCTIONS.contains("## Workflow Patterns"));
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
        };
        let result = build_server_instructions(Some(&status));
        assert!(result.contains("run `onboarding`"));
        assert!(result.contains("run `index_project`"));
    }
}
```

**Step 2: Run the tests**

Run: `cargo test prompts::tests -v`
Expected: All 4 tests pass.

**Step 3: Commit**

```bash
git add src/prompts/mod.rs
git commit -m "test: add tests for build_server_instructions"
```

---

### Task 3: Wire dynamic server instructions into `get_info()`

**Files:**
- Modify: `src/server.rs:98-110` (the `get_info` method)
- Modify: `src/server.rs:37-41` (add `instructions` field to `CodeExplorerServer`)

**Step 1: Add a method to Agent for getting project status**

Add to `src/agent.rs`:

```rust
use crate::prompts::ProjectStatus;
use crate::embed::index::db_path;

impl Agent {
    /// Get the current project status for building server instructions.
    pub async fn project_status(&self) -> Option<ProjectStatus> {
        let inner = self.inner.read().await;
        let project = inner.active_project.as_ref()?;
        let memories = project.memory.list().unwrap_or_default();
        let has_index = db_path(&project.root).exists();
        Some(ProjectStatus {
            name: project.config.project.name.clone(),
            path: project.root.display().to_string(),
            languages: project.config.project.languages.clone(),
            memories,
            has_index,
        })
    }
}
```

**Step 2: Store computed instructions in `CodeExplorerServer`**

The `get_info` method is not async and doesn't have access to `&self` mutably, so we need to compute the instructions at construction time. Modify `CodeExplorerServer`:

```rust
pub struct CodeExplorerServer {
    agent: Agent,
    lsp: Arc<LspManager>,
    tools: Vec<Arc<dyn Tool>>,
    instructions: String,  // Pre-computed at construction
}
```

In `from_parts`, compute the instructions:

```rust
pub async fn from_parts(agent: Agent, lsp: Arc<LspManager>) -> Self {
    let status = agent.project_status().await;
    let instructions = crate::prompts::build_server_instructions(status.as_ref());
    // ... tools registration stays the same ...
    Self { agent, lsp, tools, instructions }
}
```

Note: `from_parts` becomes `async`. Update call sites in `run()` accordingly (add `.await`).

**Step 3: Use `self.instructions` in `get_info()`**

```rust
fn get_info(&self) -> ServerInfo {
    ServerInfo {
        instructions: Some(self.instructions.clone().into()),
        capabilities: ServerCapabilities::builder().enable_tools().build(),
        ..Default::default()
    }
}
```

**Step 4: Update `new()` to also be async**

```rust
pub async fn new(agent: Agent) -> Self {
    Self::from_parts(agent, Arc::new(LspManager::new())).await
}
```

**Step 5: Update call sites in `run()`**

In `src/server.rs:run()`, change:
- `CodeExplorerServer::from_parts(agent, lsp)` → `CodeExplorerServer::from_parts(agent.clone(), lsp.clone()).await`
- Same for the SSE handler inside `tokio::spawn`

**Step 6: Build to verify**

Run: `cargo build 2>&1 | tail -20`
Expected: Compiles.

**Step 7: Run all tests**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests pass.

**Step 8: Commit**

```bash
git add src/server.rs src/agent.rs
git commit -m "feat: wire dynamic server instructions into MCP get_info"
```

---

### Task 4: Create the onboarding prompt template

**Files:**
- Create: `src/prompts/onboarding_prompt.md`
- Modify: `src/prompts/mod.rs` (add `ONBOARDING_PROMPT` constant and builder)

**Step 1: Create the onboarding prompt markdown**

Create `src/prompts/onboarding_prompt.md`:

```markdown
You are viewing this project for the first time. Your task is to explore it and create memories that will help you (and future conversations) work effectively with this codebase.

## What to Explore

Use code-explorer's semantic tools to gather information efficiently. Do NOT read entire source files — use symbol-level tools.

### 1. Project Purpose
- Read `README.md` or similar top-level documentation
- Identify what the project does and who it's for

### 2. Tech Stack
- Check build files (Cargo.toml, package.json, pyproject.toml, go.mod, etc.)
- Note key dependencies, frameworks, and runtime requirements

### 3. Code Architecture
- Run `get_symbols_overview("src")` (or equivalent source directory) to map the structure
- For key modules, go deeper: `get_symbols_overview("src/module", depth=1)`
- Identify the entry point(s) and how the application starts

### 4. Key Abstractions
- Find the core types, traits, interfaces, or classes that define the architecture
- Use `find_symbol(name, include_body=true)` on the most important ones
- Note inheritance/implementation hierarchies

### 5. Code Conventions
- Look for linting config (.eslintrc, .clippy.toml, .ruff.toml, etc.)
- Look for formatting config (.prettierrc, rustfmt.toml, etc.)
- Note naming conventions, error handling patterns, test organization
- Check for a CONTRIBUTING.md or similar style guide

### 6. Development Commands
- Find test, lint, format, build, and run commands from build configs
- Check CI configuration (.github/workflows/, .gitlab-ci.yml, etc.)
- Note any special setup steps or prerequisites

### 7. Architectural Patterns
- Identify design patterns in use (dependency injection, layered architecture, event-driven, etc.)
- Note how modules communicate (direct calls, messages, events, shared state)
- Look at the dependency graph between modules

Read only the necessary files — use symbol-level tools, not full-file reads. If something is unclear from the code alone, ask the user.

## Memories to Create

After exploring, call `write_memory` for each of these topics:

### `project-overview`
Project purpose, tech stack, key dependencies, runtime requirements.

### `architecture`
Module structure, key abstractions (with file locations), data flow between components, design patterns in use, entry points.

### `conventions`
Code style rules, naming conventions, error handling patterns, testing patterns, documentation conventions.

### `development-commands`
Build, test, lint, format, run commands. Include prerequisites and any environment setup needed.

### `task-completion-checklist`
What to do when finishing a task: which tests to run, how to format, how to lint, what to check. Be specific about commands.

Use "/" in memory names for deeper organization if needed (e.g., "architecture/data-flow").

**IMPORTANT:** After creating all memories, confirm with the user that the information looks accurate.
```

**Step 2: Add the constant and builder function to `src/prompts/mod.rs`**

```rust
/// Onboarding prompt template — instructs Claude what to explore and what memories to create.
pub const ONBOARDING_PROMPT: &str = include_str!("onboarding_prompt.md");

/// Build the onboarding prompt, substituting detected project information.
pub fn build_onboarding_prompt(languages: &[String], top_level: &[String]) -> String {
    let mut prompt = ONBOARDING_PROMPT.to_string();
    prompt.push_str("\n\n---\n\n## Detected Project Information\n\n");
    if !languages.is_empty() {
        prompt.push_str(&format!("**Detected languages:** {}\n\n", languages.join(", ")));
    }
    if !top_level.is_empty() {
        prompt.push_str(&format!("**Top-level structure:**\n```\n{}\n```\n", top_level.join("\n")));
    }
    prompt
}
```

**Step 3: Build to verify**

Run: `cargo build 2>&1 | tail -20`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/prompts/onboarding_prompt.md src/prompts/mod.rs
git commit -m "feat: add onboarding prompt template"
```

---

### Task 5: Write tests for `build_onboarding_prompt`

**Files:**
- Modify: `src/prompts/mod.rs` (add tests to existing test module)

**Step 1: Add tests**

Add to the `mod tests` block in `src/prompts/mod.rs`:

```rust
    #[test]
    fn onboarding_prompt_contains_key_sections() {
        assert!(ONBOARDING_PROMPT.contains("## What to Explore"));
        assert!(ONBOARDING_PROMPT.contains("## Memories to Create"));
        assert!(ONBOARDING_PROMPT.contains("project-overview"));
        assert!(ONBOARDING_PROMPT.contains("architecture"));
        assert!(ONBOARDING_PROMPT.contains("conventions"));
        assert!(ONBOARDING_PROMPT.contains("development-commands"));
        assert!(ONBOARDING_PROMPT.contains("task-completion-checklist"));
    }

    #[test]
    fn build_onboarding_includes_languages() {
        let result = build_onboarding_prompt(
            &["rust".into(), "python".into()],
            &["src/".into(), "tests/".into()],
        );
        assert!(result.contains("rust, python"));
        assert!(result.contains("src/"));
        assert!(result.contains("Detected Project Information"));
    }

    #[test]
    fn build_onboarding_handles_empty() {
        let result = build_onboarding_prompt(&[], &[]);
        assert!(result.contains("## What to Explore"));
        // No language or structure sections appended
        assert!(!result.contains("Detected languages"));
    }
```

**Step 2: Run the tests**

Run: `cargo test prompts::tests -v`
Expected: All 7 tests pass (4 existing + 3 new).

**Step 3: Commit**

```bash
git add src/prompts/mod.rs
git commit -m "test: add tests for build_onboarding_prompt"
```

---

### Task 6: Enhance `Onboarding::call()` to return instruction prompt

**Files:**
- Modify: `src/tools/workflow.rs:11-103` (the `Onboarding` impl)

**Step 1: Modify `Onboarding::call()` to include the prompt**

After the existing mechanical work (language detection, config creation, memory write), add:

```rust
    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let root = ctx.agent.require_project_root().await?;

        // ... existing language detection and top-level listing code stays the same ...

        // ... existing config creation code stays the same ...

        // ... existing memory write stays the same ...

        // Build the onboarding instruction prompt
        let lang_list: Vec<String> = languages.iter().cloned().collect();
        let prompt = crate::prompts::build_onboarding_prompt(&lang_list, &top_level);

        Ok(json!({
            "languages": lang_list,
            "top_level": top_level,
            "config_created": created_config,
            "instructions": prompt,
        }))
    }
```

The key change: adding the `"instructions"` field with the onboarding prompt to the return JSON.

**Step 2: Run existing tests to verify nothing breaks**

Run: `cargo test workflow::tests -v`
Expected: All existing onboarding tests pass.

**Step 3: Add a test verifying the prompt is in the response**

Add to `mod tests` in `src/tools/workflow.rs`:

```rust
    #[tokio::test]
    async fn onboarding_returns_instruction_prompt() {
        let (_dir, ctx) = project_ctx().await;
        let result = Onboarding.call(json!({}), &ctx).await.unwrap();
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("## What to Explore"));
        assert!(instructions.contains("## Memories to Create"));
        assert!(instructions.contains("rust"));  // detected language
    }
```

**Step 4: Run the new test**

Run: `cargo test workflow::tests::onboarding_returns_instruction_prompt -v`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: onboarding tool returns instruction prompt for Claude Code"
```

---

### Task 7: Enhance `CheckOnboardingPerformed::call()` with memory guidance

**Files:**
- Modify: `src/tools/workflow.rs:105-129` (the `CheckOnboardingPerformed` impl)

**Step 1: Enhance the response to include memory list and guidance**

```rust
    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        ctx.agent
            .with_project(|p| {
                let has_config = p.root.join(".code-explorer").join("project.toml").exists();
                let memories = p.memory.list()?;
                let has_onboarding_memory = memories.iter().any(|m| m == "onboarding");
                let onboarded = has_config && has_onboarding_memory;

                let message = if onboarded {
                    if memories.is_empty() {
                        "Onboarding was performed but no memories were created. Consider running `onboarding` again.".to_string()
                    } else {
                        format!(
                            "Onboarding already performed. Available memories: {}. \
                             Use `read_memory(topic)` to read relevant ones as needed for your current task. \
                             Do not read all memories at once — only read those relevant to what you're working on.",
                            memories.join(", ")
                        )
                    }
                } else {
                    "Onboarding not performed yet. Call the `onboarding` tool to discover the project \
                     and create memories that will help you work effectively.".to_string()
                };

                Ok(json!({
                    "onboarded": onboarded,
                    "has_config": has_config,
                    "has_onboarding_memory": has_onboarding_memory,
                    "memories": memories,
                    "message": message,
                }))
            })
            .await
    }
```

**Step 2: Update existing test to account for new fields**

The existing test `check_onboarding_before_and_after` checks `result["onboarded"]` which still works. Add a test for the message:

```rust
    #[tokio::test]
    async fn check_onboarding_returns_guidance_message() {
        let (_dir, ctx) = project_ctx().await;

        // Before onboarding
        let result = CheckOnboardingPerformed.call(json!({}), &ctx).await.unwrap();
        assert!(result["message"].as_str().unwrap().contains("not performed yet"));

        // Run onboarding
        Onboarding.call(json!({}), &ctx).await.unwrap();

        // After onboarding
        let result = CheckOnboardingPerformed.call(json!({}), &ctx).await.unwrap();
        let msg = result["message"].as_str().unwrap();
        assert!(msg.contains("already performed"));
        assert!(result["memories"].as_array().unwrap().len() > 0);
    }
```

**Step 3: Run all workflow tests**

Run: `cargo test workflow::tests -v`
Expected: All pass.

**Step 4: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: check_onboarding_performed returns memory list and guidance"
```

---

### Task 8: Final verification — clippy, fmt, full test suite

**Files:** None (verification only)

**Step 1: Format**

Run: `cargo fmt`

**Step 2: Clippy**

Run: `cargo clippy -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

**Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests pass (60+ existing + ~10 new).

**Step 4: Quick smoke test — run the server and check instructions**

Run: `cargo run -- start --project . 2>&1 &` then kill it. Alternatively just build and verify the binary runs.

Run: `cargo run -- start --project . &` (kills after a second to verify startup)

**Step 5: Commit any fmt/clippy fixes**

```bash
git add -A
git commit -m "chore: fmt and clippy fixes for onboarding enhancement"
```
