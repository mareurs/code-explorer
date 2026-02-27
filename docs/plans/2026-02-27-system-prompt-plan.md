# system_prompt Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let users add custom instructions to the MCP server prompt via `system_prompt` in `.code-explorer/project.toml`.

**Architecture:** Add `system_prompt: Option<String>` to `ProjectSection` (config) and `ProjectStatus` (prompt builder). Thread the value from config → agent → prompt builder, appending it under a `## Custom Instructions` header after the project status block.

**Tech Stack:** Rust, serde (TOML deserialization), existing prompt builder in `src/prompts/mod.rs`.

**Design doc:** `docs/plans/2026-02-27-system-prompt-design.md`

---

### Task 1: Add `system_prompt` field to config

**Files:**
- Modify: `src/config/project.rs:19-27` (`ProjectSection` struct)
- Modify: `src/config/project.rs:212-224` (`default_for` method)

**Step 1: Write the failing test**

Add to the `tests` module in `src/config/project.rs` (after the last test at line 284):

```rust
    #[test]
    fn system_prompt_defaults_to_none() {
        let toml = "[project]\nname = \"test\"";
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(cfg.project.system_prompt.is_none());
    }

    #[test]
    fn system_prompt_parses_from_toml() {
        let toml = "[project]\nname = \"test\"\nsystem_prompt = \"Use pytest for testing.\"";
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.project.system_prompt.as_deref(),
            Some("Use pytest for testing.")
        );
    }
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::project::tests::system_prompt`
Expected: compilation error — `system_prompt` field doesn't exist on `ProjectSection`.

**Step 3: Add the field to `ProjectSection`**

Add after the `tool_timeout_secs` field (line 26) in `ProjectSection`:

```rust
    #[serde(default)]
    pub system_prompt: Option<String>,
```

And add the field to `default_for()` in the `ProjectSection` struct literal (after `tool_timeout_secs`):

```rust
                system_prompt: None,
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::project::tests::system_prompt`
Expected: both `system_prompt_defaults_to_none` and `system_prompt_parses_from_toml` PASS.

**Step 5: Run full test suite**

Run: `cargo test`
Expected: all tests pass, no regressions.

---

### Task 2: Thread `system_prompt` through `ProjectStatus` and prompt builder

**Files:**
- Modify: `src/prompts/mod.rs:52-58` (`ProjectStatus` struct)
- Modify: `src/prompts/mod.rs:12-48` (`build_server_instructions` function)
- Modify: `src/agent.rs:108-120` (`project_status` method)

**Step 1: Write the failing tests**

Add to the `tests` module in `src/prompts/mod.rs` (after the last test):

```rust
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
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib prompts::tests::build_with`
Expected: compilation error — `system_prompt` field doesn't exist on `ProjectStatus`.

**Step 3: Add `system_prompt` field to `ProjectStatus`**

Add after `has_index` (line 57) in `ProjectStatus`:

```rust
    pub system_prompt: Option<String>,
```

**Step 4: Fix existing test compilation**

The existing test `build_with_project_appends_status` (line 152) constructs a `ProjectStatus` without `system_prompt`. Add `system_prompt: None,` to that struct literal (after `has_index: true,`).

Similarly fix `build_with_no_memories_suggests_onboarding` (line 169) — add `system_prompt: None,` to its `ProjectStatus`.

**Step 5: Append custom instructions in `build_server_instructions`**

At the end of `build_server_instructions()`, inside the `if let Some(status)` block (before the closing `}`), add:

```rust
        if let Some(prompt) = &status.system_prompt {
            instructions.push_str("\n\n## Custom Instructions\n\n");
            instructions.push_str(prompt);
            instructions.push('\n');
        }
```

**Step 6: Thread the field in `Agent::project_status()`**

In `src/agent.rs`, in the `ProjectStatus` struct literal inside `project_status()` (after `has_index,`), add:

```rust
            system_prompt: project.config.project.system_prompt.clone(),
```

**Step 7: Run tests to verify they pass**

Run: `cargo test --lib prompts::tests`
Expected: all prompts tests pass including the two new ones.

**Step 8: Run full test suite + clippy + fmt**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: all clean, all tests pass.

**Step 9: Commit**

```bash
git add src/config/project.rs src/prompts/mod.rs src/agent.rs
git commit -m "feat(config): add system_prompt field for user-customizable server instructions"
```

---

### Task 3: Final verification

**Step 1: Manual smoke test**

Create a test project config:

```bash
mkdir -p /tmp/test-system-prompt/.code-explorer
cat > /tmp/test-system-prompt/.code-explorer/project.toml << 'EOF'
[project]
name = "test-project"
system_prompt = "This is a test project. Always prefer functional patterns."
EOF
```

Run the server briefly to verify instructions include the custom prompt:

```bash
cargo run -- start --project /tmp/test-system-prompt --transport stdio
```

Send an MCP `initialize` request and verify the server info instructions contain `## Custom Instructions` and the user's text.

**Step 2: Clean up**

```bash
rm -rf /tmp/test-system-prompt
```
