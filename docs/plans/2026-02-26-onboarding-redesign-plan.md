# Onboarding Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the `onboarding` tool gather rich project context (README, build files, CLAUDE.md, entry points, test dirs, CI config) and return a prompt that guides the LLM to create 6 high-quality, templated memories — replacing the current "ls output + hope the LLM follows through" approach.

**Architecture:** The Rust `Onboarding.call()` method gains a `gather_project_context()` helper that pre-reads key files. The gathered data is passed to a rewritten `build_onboarding_prompt()` which embeds it into a redesigned `onboarding_prompt.md`. The prompt now includes per-memory templates, quality anti-patterns, and CLAUDE.md awareness. The onboarding marker memory becomes a structured summary instead of ls output.

**Tech Stack:** Rust, serde_json, std::fs, ignore crate (existing deps)

---

### Task 1: Add `GatheredContext` struct and `gather_project_context()` helper

**Files:**
- Modify: `src/tools/workflow.rs` (add struct + helper function before `impl Tool for Onboarding`)

**Step 1: Write the failing test**

Add to the `tests` module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn gather_context_reads_readme_and_build_file() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("README.md"), "# My Project\nA test project.").unwrap();
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test gather_context_reads_readme_and_build_file -- --nocapture`
Expected: FAIL — `gather_project_context` not found

**Step 3: Write the implementation**

Add this struct and function in `src/tools/workflow.rs` after the struct declarations (line 9) and before the `impl Tool for Onboarding` block:

```rust
/// Context gathered from well-known project files during onboarding.
/// Provides the LLM with pre-read data so it can synthesize memories
/// without making dozens of extra tool calls.
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

/// Read key project files up-front so the LLM prompt can include them.
fn gather_project_context(root: &std::path::Path) -> GatheredContext {
    let mut ctx = GatheredContext::default();
    const MAX_FILE_BYTES: u64 = 32_000; // ~200 lines cap

    // Helper: read file if it exists and is small enough
    let read_capped = |path: &std::path::Path| -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > MAX_FILE_BYTES {
            let content = std::fs::read_to_string(path).ok()?;
            let truncated: String = content.chars().take(MAX_FILE_BYTES as usize).collect();
            Some(format!("{}\n\n[... truncated at {} bytes ...]", truncated, MAX_FILE_BYTES))
        } else {
            std::fs::read_to_string(path).ok()
        }
    };

    // README (try common names)
    for name in &["README.md", "README.rst", "README.txt", "README"] {
        let path = root.join(name);
        if let Some(content) = read_capped(&path) {
            ctx.readme = Some(content);
            break;
        }
    }

    // CLAUDE.md
    if let Some(content) = read_capped(&root.join("CLAUDE.md")) {
        ctx.claude_md = Some(content);
    }

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
        let path = root.join(name);
        if let Some(content) = read_capped(&path) {
            ctx.build_file_name = Some(name.to_string());
            ctx.build_file_content = Some(content);
            break;
        }
    }

    // CI config files (just names, not contents)
    let ci_dirs = [".github/workflows", ".gitlab", ".circleci"];
    for dir in &ci_dirs {
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
    let test_candidates = [
        "tests",
        "test",
        "spec",
        "src/test",
        "src/tests",
        "__tests__",
    ];
    for candidate in &test_candidates {
        if root.join(candidate).is_dir() {
            ctx.test_dirs.push(candidate.to_string());
        }
    }

    ctx
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test gather_context_reads_readme_and_build_file -- --nocapture`
Expected: PASS

**Step 5: Write additional tests for gather_project_context**

```rust
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
```

**Step 6: Run all tests to verify**

Run: `cargo test gather_context -- --nocapture`
Expected: All 4 gather_context tests PASS

**Step 7: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(onboarding): add GatheredContext struct and gather_project_context helper"
```

---

### Task 2: Rewrite onboarding_prompt.md with templates and quality guidelines

**Files:**
- Rewrite: `src/prompts/onboarding_prompt.md`

**Step 1: Replace the entire file content**

The new prompt has three sections:
1. Instructions telling the LLM it has pre-gathered data to synthesize
2. Per-memory templates with structure, examples, and anti-patterns
3. Rules about CLAUDE.md deduplication and confirmation

```markdown
You have just onboarded this project. Below you'll find pre-gathered context from key project files. Your job is to **synthesize this into 6 memories** using `write_memory(topic, content)`.

## Rules

1. **Do NOT duplicate CLAUDE.md** — If `claude_md` is provided below, assume it's loaded every session. Memories should contain knowledge that *supplements* CLAUDE.md, not repeats it. If CLAUDE.md already covers dev commands comprehensively, your `development-commands` memory should only add what CLAUDE.md misses.
2. **Be specific** — Include file paths, exact command names, concrete patterns. "Uses clean architecture" is useless. "api/ → service/ → repository/ with interface+impl pattern in each" is useful.
3. **Be concise** — Each memory should be 15-40 lines. If it's longer, you're including too much detail.
4. **Explore before writing** — The gathered data gives you a head start, but use code-explorer tools to verify and fill gaps: `get_symbols_overview("src")` for architecture, `find_symbol` for key abstractions, `list_functions` for API surface.
5. **Confirm with the user** — After creating all memories, summarize what you wrote and ask if anything needs correction.

## Memories to Create

### 1. `project-overview`

**What:** Project purpose, tech stack, key dependencies, runtime requirements.
**Template:**
```
# [Project Name]

## Purpose
[1-2 sentences: what does this project do and who is it for?]

## Tech Stack
- **Language:** [lang] [version if known]
- **Framework:** [framework] [version]
- **Database:** [if any]
- **Key deps:** [3-5 most important dependencies]

## Runtime Requirements
[What's needed to run: Node 20+, Java 21+, Docker, specific env vars, etc.]
```
**Anti-patterns:**
- Do NOT list every dependency from the build file
- Do NOT include directory listings
- Do NOT copy the README verbatim

### 2. `architecture`

**What:** Module structure, key abstractions with file locations, data flow, design patterns, entry points.
**Template:**
```
# Architecture

## Layer Structure
[Describe the main modules/layers and their responsibilities]
[Include file paths: `src/services/` → business logic]

## Key Abstractions
[The 3-5 most important types/traits/interfaces that define the architecture]
[Include name + file path for each]

## Data Flow
[How does a typical request/operation flow through the system?]
[Entry point → layer 1 → layer 2 → ... → output]

## Design Patterns
[DI pattern, repository pattern, event-driven, etc. — only what's actually used]
```
**Anti-patterns:**
- Do NOT list every file in the project
- Do NOT describe standard library types
- DO include file paths for every abstraction you mention

### 3. `conventions`

**What:** Code style rules, naming conventions, error handling, testing patterns.
**Template:**
```
# Conventions

## Naming
[Table: entity type → convention → example]

## Patterns
[Key patterns: error handling, DI, async, testing]
[Include short code examples where helpful]

## Code Quality
[Linter, formatter, type checker — with exact commands]

## Testing
[Framework, test organization, how to write a new test]
```
**Anti-patterns:**
- Do NOT describe language-standard conventions everyone knows
- DO focus on project-specific conventions that differ from defaults

### 4. `development-commands`

**What:** Build, test, lint, format, run commands with gotchas. Includes task completion checklist.
**Template:**
```
# Development Commands

## Build & Run
[command] — [what it does] [any gotchas]

## Test
[command] — [what it does]

## Quality
[lint, format, type-check commands]

## Before Completing Work
1. [Step 1: specific command]
2. [Step 2: specific command]
...
```
**Anti-patterns:**
- Do NOT duplicate commands already in CLAUDE.md (reference them instead)
- DO include non-obvious gotchas (e.g., "port 5433 not 5432", "use heredoc for passwords with !")

### 5. `domain-glossary`

**What:** Project-specific terms, abbreviations, and concepts that aren't obvious from code alone.
**Template:**
```
# Domain Glossary

**[Term]** — [1-sentence definition]. [File/module where it lives if relevant.]
**[Term]** — [1-sentence definition].
...
```
**What to include:**
- Domain model names that have specific meaning (e.g., "Stage1" = date assignment solver phase)
- Abbreviations used in code (e.g., "DTO", "SSE" as used in this project)
- Concepts that require context (e.g., "OutputGuard" = progressive disclosure enforcer)
**Anti-patterns:**
- Do NOT define standard programming terms (API, REST, etc.)
- DO define terms that are project-specific or used in a project-specific way

### 6. `gotchas`

**What:** Known issues, common mistakes, things that trip people up.
**Template:**
```
# Gotchas & Known Issues

## [Category]
- **Problem:** [what goes wrong]
  **Fix:** [what to do instead]

## [Category]
- **Problem:** [what goes wrong]
  **Fix:** [what to do instead]
```
**What to include:**
- Configuration pitfalls (wrong port, missing env var)
- Framework-specific traps (wrong import, deprecated API)
- Build/test gotchas (flaky tests, slow builds, order-dependent tests)
- If nothing is obviously wrong, write "No known gotchas discovered during onboarding. Update this memory as issues are found."
**Anti-patterns:**
- Do NOT invent problems that don't exist
- DO note if tests are failing or there are TODOs in the code

## Gathered Project Data

The data below was collected automatically. Use it as your starting point, then use code-explorer tools to fill gaps.
```

**Step 2: Run prompt tests to see what breaks**

Run: `cargo test onboarding_prompt -- --nocapture`
Expected: `onboarding_prompt_contains_key_sections` will FAIL because it checks for old section headers

**Step 3: Update the prompt test**

In `src/prompts/mod.rs`, update `onboarding_prompt_contains_key_sections`:

```rust
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
```

**Step 4: Run tests**

Run: `cargo test onboarding_prompt_contains_key_sections -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/prompts/onboarding_prompt.md src/prompts/mod.rs
git commit -m "feat(onboarding): rewrite prompt with per-memory templates and quality guidelines"
```

---

### Task 3: Update `build_onboarding_prompt()` to accept `GatheredContext`

**Files:**
- Modify: `src/prompts/mod.rs` — change `build_onboarding_prompt` signature and body

**Step 1: Write the failing test**

Add to `src/prompts/mod.rs` tests module:

```rust
#[test]
fn build_onboarding_includes_gathered_context() {
    let result = build_onboarding_prompt(
        &["rust".into(), "python".into()],
        &["src/".into(), "tests/".into()],
        Some("# My Project\nA cool thing."),    // readme
        Some(("Cargo.toml", "[package]\nname = \"cool\"")),  // build_file
        None,                                     // claude_md
        &[".github/workflows/ci.yml".into()],    // ci_files
        &["src/main.rs".into()],                  // entry_points
        &["tests".into()],                        // test_dirs
    );
    assert!(result.contains("# My Project"));
    assert!(result.contains("Cargo.toml"));
    assert!(result.contains("ci.yml"));
    assert!(result.contains("src/main.rs"));
    assert!(result.contains("Detected languages"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test build_onboarding_includes_gathered_context -- --nocapture`
Expected: FAIL — signature mismatch

**Step 3: Update the function signature and body**

Replace `build_onboarding_prompt` in `src/prompts/mod.rs`:

```rust
#[allow(clippy::too_many_arguments)]
pub fn build_onboarding_prompt(
    languages: &[String],
    top_level: &[String],
    readme: Option<&str>,
    build_file: Option<(&str, &str)>,  // (name, content)
    claude_md: Option<&str>,
    ci_files: &[String],
    entry_points: &[String],
    test_dirs: &[String],
) -> String {
    let mut prompt = ONBOARDING_PROMPT.to_string();

    // Append gathered data section
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
        prompt.push_str(&format!(
            "**CI config files:** {}\n\n",
            ci_files.join(", ")
        ));
    }

    if let Some(content) = readme {
        prompt.push_str(&format!(
            "**README.md:**\n```\n{}\n```\n\n",
            content
        ));
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
```

**Step 4: Fix existing tests that call old signature**

Update `build_onboarding_includes_languages`:
```rust
#[test]
fn build_onboarding_includes_languages() {
    let result = build_onboarding_prompt(
        &["rust".into(), "python".into()],
        &["src/".into(), "tests/".into()],
        None, None, None, &[], &[], &[],
    );
    assert!(result.contains("rust, python"));
    assert!(result.contains("src/"));
}
```

Update `build_onboarding_handles_empty`:
```rust
#[test]
fn build_onboarding_handles_empty() {
    let result = build_onboarding_prompt(&[], &[], None, None, None, &[], &[], &[]);
    assert!(result.contains("## Memories to Create"));
    assert!(!result.contains("Detected languages"));
}
```

**Step 5: Run all prompt tests**

Run: `cargo test --lib prompts -- --nocapture`
Expected: All PASS

**Step 6: Commit**

```bash
git add src/prompts/mod.rs
git commit -m "feat(onboarding): update build_onboarding_prompt to accept gathered context"
```

---

### Task 4: Wire `gather_project_context()` into `Onboarding.call()` and update memory marker

**Files:**
- Modify: `src/tools/workflow.rs` — update `Onboarding.call()` body

**Step 1: Update the call method**

Replace the body of `Onboarding.call()` (lines 22-107 in workflow.rs). The key changes:
1. Call `gather_project_context(&root)` after language detection
2. Pass gathered data to `build_onboarding_prompt`
3. Save a structured onboarding marker instead of ls output
4. Return gathered data in JSON response

```rust
async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    let root = ctx.agent.require_project_root().await?;

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

    // Store structured onboarding marker in memory
    let lang_list: Vec<String> = languages.iter().cloned().collect();
    ctx.agent
        .with_project(|p| {
            let summary = format!(
                "Languages: {}\nOnboarded: {}\nHas README: {}\nHas CLAUDE.md: {}\nBuild file: {}\nEntry points: {}\nTest dirs: {}",
                lang_list.join(", "),
                chrono::Local::now().format("%Y-%m-%d"),
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

    // Build the onboarding instruction prompt with gathered data
    let prompt = crate::prompts::build_onboarding_prompt(
        &lang_list,
        &top_level,
        gathered.readme.as_deref(),
        gathered.build_file_name.as_deref()
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
```

**Note about chrono:** Check if chrono is already a dependency. If not, use a simpler date approach — see step 2.

**Step 2: Check chrono dependency; if missing, use alternative**

Run: `grep chrono Cargo.toml`

If chrono is not present, replace the date line with:
```rust
// Use a simple date without chrono
let date = {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple days-since-epoch formatting
    format!("{}", now / 86400) // just epoch-days, good enough for a marker
};
```

Or better: just omit the date field entirely. The file modification timestamp serves the same purpose.

**Step 3: Run existing tests to see what breaks**

Run: `cargo test --lib tools::workflow -- --nocapture`
Expected: All existing onboarding tests should still pass (the JSON response is a superset of the old one)

**Step 4: Add a test for the new response fields**

```rust
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
    assert!(result["entry_points"].as_array().unwrap().iter().any(|v| v == "src/main.rs") == false);
    // main.rs is at root, not src/main.rs — entry_points checks src/main.rs
    assert!(result["test_dirs"].as_array().unwrap().iter().any(|v| v == "tests"));
}
```

**Step 5: Run full test suite**

Run: `cargo test -- --nocapture`
Expected: All PASS

**Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(onboarding): wire gathered context into Onboarding.call and improve marker memory"
```

---

### Task 5: Update description and clean up

**Files:**
- Modify: `src/tools/workflow.rs` — update tool description

**Step 1: Update the Onboarding tool description**

Change the `description()` method:
```rust
fn description(&self) -> &str {
    "Perform initial project discovery: detect languages, read key files \
     (README, build config, CLAUDE.md), and return instructions for creating \
     project memories. Requires an active project."
}
```

**Step 2: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- -D warnings`
Expected: Clean

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All pass

**Step 4: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "chore(onboarding): update tool description to reflect new capabilities"
```

---

### Task 6: Final integration test and cleanup

**Files:**
- All modified files: `src/tools/workflow.rs`, `src/prompts/mod.rs`, `src/prompts/onboarding_prompt.md`

**Step 1: Run the full test suite**

Run: `cargo test`
Expected: All 232+ tests pass (some new ones added)

**Step 2: Run clippy with strict warnings**

Run: `cargo clippy -- -D warnings`
Expected: Clean

**Step 3: Run fmt check**

Run: `cargo fmt -- --check`
Expected: Clean

**Step 4: Manual smoke test**

Run: `cargo run -- start --project .`
Then from another terminal, send an MCP onboarding request to verify the tool returns the new fields and prompt.

Alternatively, inspect the test output to verify the `instructions` field contains the new prompt sections.

**Step 5: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "test(onboarding): add integration tests for redesigned onboarding flow"
```
