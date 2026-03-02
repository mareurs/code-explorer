# System Prompt Onboarding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move system_prompt from inline TOML to `.code-explorer/system-prompt.md`, auto-generate it during onboarding with user confirmation, and include project-specific navigation guidance.

**Architecture:** `project_status()` in `agent.rs` reads the file (TOML fallback). `build_server_instructions()` already handles `system_prompt: Option<String>` — no change needed there. Onboarding returns a `system_prompt_draft` and the prompt instructs the LLM to confirm with the user before writing the file.

**Tech Stack:** Rust, TOML (serde), Markdown files, existing MemoryStore/ToolContext patterns

**Design doc:** `docs/plans/2026-02-28-system-prompt-onboarding-design.md`

---

### Task 1: Read system-prompt.md in `project_status()` with TOML Fallback

**Files:**
- Modify: `src/agent.rs:108-121` (`project_status` method)
- Test: `src/agent.rs` (tests module)

**Step 1: Write the failing test**

Add to the `tests` module in `src/agent.rs`:

```rust
#[tokio::test]
async fn project_status_reads_system_prompt_file() {
    let dir = tempfile::tempdir().unwrap();
    // Create minimal project.toml (no system_prompt field)
    let config_dir = dir.path().join(".code-explorer");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("project.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();
    // Create system-prompt.md
    std::fs::write(
        config_dir.join("system-prompt.md"),
        "Always use pytest.\n",
    )
    .unwrap();

    let agent = Agent::activate(dir.path().to_path_buf()).await.unwrap();
    let status = agent.project_status().await.unwrap();
    assert_eq!(
        status.system_prompt.as_deref(),
        Some("Always use pytest.\n")
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test project_status_reads_system_prompt_file -- --nocapture`
Expected: FAIL — `project_status()` currently reads from `config.project.system_prompt` (None in this case)

**Step 3: Write the failing test for TOML fallback**

```rust
#[tokio::test]
async fn project_status_falls_back_to_toml_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".code-explorer");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("project.toml"),
        "[project]\nname = \"test\"\nsystem_prompt = \"From TOML\"\n",
    )
    .unwrap();
    // No system-prompt.md file

    let agent = Agent::activate(dir.path().to_path_buf()).await.unwrap();
    let status = agent.project_status().await.unwrap();
    assert_eq!(status.system_prompt.as_deref(), Some("From TOML"));
}
```

**Step 4: Write the failing test for file-takes-precedence**

```rust
#[tokio::test]
async fn project_status_file_takes_precedence_over_toml() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".code-explorer");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("project.toml"),
        "[project]\nname = \"test\"\nsystem_prompt = \"From TOML\"\n",
    )
    .unwrap();
    std::fs::write(
        config_dir.join("system-prompt.md"),
        "From file\n",
    )
    .unwrap();

    let agent = Agent::activate(dir.path().to_path_buf()).await.unwrap();
    let status = agent.project_status().await.unwrap();
    assert_eq!(status.system_prompt.as_deref(), Some("From file\n"));
}
```

**Step 5: Implement the change in `project_status()`**

Modify `src/agent.rs:108-121`. Replace the `system_prompt` line with file-first logic:

```rust
pub async fn project_status(&self) -> Option<crate::prompts::ProjectStatus> {
    let inner = self.inner.read().await;
    let project = inner.active_project.as_ref()?;
    let memories = project.memory.list().unwrap_or_default();
    let has_index = crate::embed::index::db_path(&project.root).exists();

    // Read system prompt: file takes precedence over TOML field
    let prompt_file = project
        .root
        .join(".code-explorer")
        .join("system-prompt.md");
    let system_prompt = if prompt_file.exists() {
        std::fs::read_to_string(&prompt_file).ok()
    } else {
        project.config.project.system_prompt.clone()
    };

    Some(crate::prompts::ProjectStatus {
        name: project.config.project.name.clone(),
        path: project.root.display().to_string(),
        languages: project.config.project.languages.clone(),
        memories,
        has_index,
        system_prompt,
    })
}
```

**Step 6: Run all three tests**

Run: `cargo test project_status_reads_system_prompt_file project_status_falls_back_to_toml project_status_file_takes_precedence -- --nocapture`
Expected: All PASS

**Step 7: Run full test suite**

Run: `cargo test`
Expected: All existing tests still pass

**Step 8: Commit**

```bash
git add src/agent.rs
git commit -m "feat: read system-prompt.md with TOML fallback in project_status"
```

---

### Task 2: Update Onboarding Prompt with System Prompt Template

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`
- Test: `src/prompts/mod.rs` (existing onboarding prompt tests)

**Step 1: Add system prompt template section to onboarding_prompt.md**

Insert before the `## Gathered Project Data` line (at the end of the memories section):

```markdown
### 7. System Prompt — `.code-explorer/system-prompt.md`

After creating the 6 memories above, synthesize a concise system prompt (15-30 lines)
for this project. This prompt is injected into EVERY code-explorer session
automatically — it must be short and high-value. Do NOT repeat information from the
static tool guidance (how to use find_symbol, list_symbols, etc.) — that's already
provided to you separately.

**What to include:**
- Entry points: where to start exploring this codebase (specific files + symbols)
- Key abstractions: 3-5 core types/traits that form the skeleton of this project
- Search tips: semantic_search queries that work well for THIS codebase, and terms to avoid (too broad, too generic)
- Navigation strategy: recommended exploration order for a new task in this project
- Project rules: conventions the AI should always follow that aren't captured by linters

**What NOT to include (already covered elsewhere):**
- How code-explorer tools work (the static tool guidance handles this)
- Full architecture details (the `architecture` memory covers this)
- Command lists, glossary, detailed conventions (memories cover these)
- Anything over 30 lines (keep it concise — this is injected every session)

**Template:**
```
# [Project Name] — Code Explorer Guidance

## Entry Points
[Where to start. Specific files + symbols, not module descriptions.]

## Key Abstractions
[3-5 core types with file paths. What to understand first.]

## Search Tips
[Concrete query examples that work well. Terms to avoid.]

## Navigation Strategy
[Recommended exploration order for new tasks.]

## Project Rules
[Conventions the AI should always follow.]
```

**Process:** Present the draft to the user and ask: "Does this system prompt look
right? I'll save it to `.code-explorer/system-prompt.md`." After confirmation, write
the file using `create_file`. Inform the user they can edit it anytime.

## After Everything Is Created

After confirming all 6 memories and the system prompt with the user, deliver this:

---

**Your code-explorer setup is complete.**

- **System prompt** (`.code-explorer/system-prompt.md`) — always-on project guidance,
  injected into every session. Edit anytime to refine how AI navigates your codebase.
- **Memories** — reference material read on demand via `read_memory(topic)`. Update
  with `write_memory(topic, content)`.
- **Quick start for new tasks:**
  1. `read_memory("architecture")` — orient yourself
  2. `list_symbols("src/")` — see the module structure
  3. `semantic_search("your concept")` — find relevant code
  4. `find_symbol("Name", include_body=true)` — read the implementation

---
```

**Step 2: Run existing onboarding prompt tests**

Run: `cargo test onboarding -- --nocapture`
Expected: All pass (the prompt content changed but tests check for structural things, not exact content)

**Step 3: Commit**

```bash
git add src/prompts/onboarding_prompt.md
git commit -m "feat: add system prompt template and post-onboarding guide to onboarding prompt"
```

---

### Task 3: Add `system_prompt_draft` to Onboarding Output

**Files:**
- Modify: `src/tools/workflow.rs:158-298` (`impl Tool for Onboarding / call`)
- Test: `src/tools/workflow.rs` (tests module)

**Step 1: Write the failing test**

Add to the `tests` module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn onboarding_includes_system_prompt_draft_field() {
    let dir = tempfile::tempdir().unwrap();
    // Create a README so onboarding has something to work with
    std::fs::write(dir.path().join("README.md"), "# Test Project\nA test.").unwrap();
    std::fs::write(dir.path().join("main.py"), "print('hello')").unwrap();
    let lsp = lsp();
    let ctx = project_ctx(dir.path(), lsp);
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
    assert!(
        !draft.is_empty(),
        "system_prompt_draft should not be empty"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test onboarding_includes_system_prompt_draft_field -- --nocapture`
Expected: FAIL — `system_prompt_draft` field not present in output

**Step 3: Implement the change**

In the `call` method of `impl Tool for Onboarding` (`src/tools/workflow.rs`), after
the `let prompt = ...` line (around line 272), add system prompt draft generation.
Then include it in the final JSON output.

Add a helper function before the `impl Tool for Onboarding`:

```rust
fn build_system_prompt_draft(
    languages: &[String],
    entry_points: &[String],
    readme: Option<&str>,
    build_file_name: Option<&str>,
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
    draft.push_str("<!-- The onboarding AI should replace this with 3-5 core types -->\n");
    draft.push_str("- [Discover with `list_symbols` on main source directories]\n\n");

    // Search tips
    draft.push_str("## Search Tips\n");
    if !languages.is_empty() {
        draft.push_str(&format!(
            "- This is a {} project\n",
            languages.join("/")
        ));
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
    if build_file_name.is_some() {
        draft.push_str("- Check build/test commands in `development-commands` memory\n");
    }
    draft.push_str("- [Add project-specific conventions here]\n");

    draft
}
```

Then in the `call` method, after `let gathered = gather_project_context(&root);`
(around line 251), add:

```rust
let system_prompt_draft = build_system_prompt_draft(
    &lang_list,
    &gathered.entry_points,
    gathered.readme.as_deref(),
    gathered.build_file_name.as_deref(),
);
```

And add to the final JSON output:

```rust
"system_prompt_draft": system_prompt_draft,
```

**Step 4: Run tests**

Run: `cargo test onboarding_includes_system_prompt_draft -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test`
Expected: All pass

**Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: onboarding returns system_prompt_draft for user confirmation"
```

---

### Task 4: Update Onboarding Description to Mention System Prompt

**Files:**
- Modify: `src/tools/workflow.rs:141-146` (`description` method of Onboarding)

**Step 1: Read current description**

The current `description()` method returns the onboarding tool description. Update it
to mention that onboarding also generates a system prompt draft.

**Step 2: Update the description**

In the `description` method of `impl Tool for Onboarding`, update to include mention
of the system prompt:

```rust
fn description(&self) -> &'static str {
    "Perform initial project discovery: detect languages, read key files \
     (README, build config, CLAUDE.md), and return instructions for creating \
     project memories and a system prompt draft. Requires an active project. \
     Returns status if already onboarded (use force=true to re-scan)."
}
```

**Step 3: Run tests**

Run: `cargo test`
Expected: All pass

**Step 4: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "docs: update onboarding tool description to mention system prompt"
```

---

### Task 5: Deprecation Comment on TOML `system_prompt` Field

**Files:**
- Modify: `src/config/project.rs:28` (the `system_prompt` field)

**Step 1: Add deprecation doc comment**

On the `system_prompt` field in `ProjectSection`, add a comment:

```rust
    /// Deprecated: use `.code-explorer/system-prompt.md` instead.
    /// This field is still read as a fallback if the file doesn't exist.
    /// Will be removed in a future version.
    #[serde(default)]
    pub system_prompt: Option<String>,
```

**Step 2: Run tests**

Run: `cargo test`
Expected: All pass (doc comment doesn't affect behavior)

**Step 3: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: Clean

**Step 4: Commit**

```bash
git add src/config/project.rs
git commit -m "docs: deprecate system_prompt TOML field in favor of system-prompt.md"
```

---

### Task 6: Update Server Instructions to Mention system-prompt.md

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Add mention of system-prompt.md to the server instructions**

In the `## Project Status` section template (built dynamically), no change is needed —
`build_server_instructions()` already handles `system_prompt` regardless of source.

However, update `server_instructions.md` to mention the file in its Rules or
introductory section. Add a line like:

```markdown
## Project Customization

If `.code-explorer/system-prompt.md` exists, its contents appear below as
"Custom Instructions" — project-specific guidance from the user.
```

Insert this before `## Rules` at the end of the static content.

**Step 2: Run tests**

Run: `cargo test`
Expected: All pass

**Step 3: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs: mention system-prompt.md in server instructions"
```

---

### Task 7: Final Verification and Cleanup

**Step 1: Run full quality checks**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Expected: All clean, all pass.

**Step 2: Verify onboarding flow manually (optional)**

```bash
cargo run -- start --project /tmp/test-project
```

Then call `onboarding()` and verify:
- `system_prompt_draft` is present in the output
- The instructions prompt includes the system prompt template section
- The post-onboarding guide is included

**Step 3: Squash or batch commits if desired**

The 6 commits from tasks 1-6 can be squashed into 1-2 logical commits:
- `feat: system-prompt.md support (file-first with TOML fallback)`
- `feat: onboarding generates system prompt draft`

Or kept as-is for granularity.
