# Protected Memories Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make certain memory topics survive `onboarding(force=true)` via anchor-based staleness checks, LLM verification of stale entries, and user approval before writing.

**Architecture:** Config-driven protection list in `project.toml`. Rust code computes staleness data for protected memories and includes it in the onboarding JSON result. The onboarding prompt instructs the LLM to merge/verify/approve rather than blindly overwrite. No changes to `MemoryStore::write()` itself.

**Tech Stack:** Rust, serde, toml, existing anchor/staleness system in `src/memory/anchors.rs`

**Spec:** `docs/superpowers/specs/2026-03-11-protected-memories-design.md`

---

## Chunk 1: Config and Serialization

### Task 1: Add `Serialize` to `AnchorStatus` and `StaleFile`

The onboarding result needs to serialize staleness info to JSON. Currently `AnchorStatus` and `StaleFile` lack `Serialize`.

**Files:**
- Modify: `src/memory/anchors.rs:120-131`

- [ ] **Step 1: Write the failing test**

Add to the existing `tests` module in `src/memory/anchors.rs`:

```rust
#[test]
fn stale_file_serializes_to_json() {
    let sf = super::StaleFile {
        path: "src/foo.rs".to_string(),
        status: super::AnchorStatus::Changed,
    };
    let json = serde_json::to_value(&sf).unwrap();
    assert_eq!(json["path"], "src/foo.rs");
    assert_eq!(json["status"], "changed");

    let sf_deleted = super::StaleFile {
        path: "src/bar.rs".to_string(),
        status: super::AnchorStatus::Deleted,
    };
    let json = serde_json::to_value(&sf_deleted).unwrap();
    assert_eq!(json["status"], "deleted");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test stale_file_serializes_to_json -- --nocapture`
Expected: Compile error — `Serialize` not implemented for `StaleFile` / `AnchorStatus`.

- [ ] **Step 3: Add Serialize derives with rename_all**

In `src/memory/anchors.rs`, change the derives:

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorStatus {
    Changed,
    Deleted,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleFile {
    pub path: String,
    pub status: AnchorStatus,
}
```

`Serialize` is already imported in this file (`use serde::{Deserialize, Serialize};` at line 3).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test stale_file_serializes_to_json -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: Clean

---

### Task 2: Add `protected` field to `MemorySection`

**Files:**
- Modify: `src/config/project.rs:183-213`

- [ ] **Step 1: Write the failing tests**

Add to the existing `tests` module in `src/config/project.rs`:

```rust
#[test]
fn memory_section_default_includes_gotchas() {
    let section = MemorySection::default();
    assert_eq!(section.protected, vec!["gotchas".to_string()]);
}

#[test]
fn memory_section_serde_roundtrip_with_protected() {
    let toml_str = r#"
staleness_drift_threshold = 0.3
protected = ["gotchas", "conventions"]
"#;
    let section: MemorySection = toml::from_str(toml_str).unwrap();
    assert_eq!(section.protected, vec!["gotchas".to_string(), "conventions".to_string()]);

    // Round-trip
    let serialized = toml::to_string_pretty(&section).unwrap();
    let deserialized: MemorySection = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.protected, section.protected);
}

#[test]
fn memory_section_missing_protected_uses_default() {
    let toml_str = r#"
staleness_drift_threshold = 0.3
"#;
    let section: MemorySection = toml::from_str(toml_str).unwrap();
    assert_eq!(section.protected, vec!["gotchas".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test memory_section_default_includes_gotchas memory_section_serde_roundtrip memory_section_missing_protected -- --nocapture`
Expected: Compile error — `protected` field does not exist.

- [ ] **Step 3: Add the field and default function**

In `src/config/project.rs`, add the default function near the other default functions (after line 203, where `default_semantic_anchor_top_n` ends):

```rust
fn default_protected_topics() -> Vec<String> {
    vec!["gotchas".to_string()]
}
```

Add the field to `MemorySection` struct (inside the struct body, after `semantic_anchor_top_n`):

```rust
    /// Memory topics protected from blind overwrite during force re-onboarding.
    /// Protected topics go through a staleness-check + merge + user-approval flow.
    #[serde(default = "default_protected_topics")]
    pub protected: Vec<String>,
```

Update the `Default` impl for `MemorySection` to include the new field:

```rust
impl Default for MemorySection {
    fn default() -> Self {
        Self {
            staleness_drift_threshold: default_staleness_drift_threshold(),
            semantic_anchor_min_similarity: default_semantic_anchor_min_similarity(),
            semantic_anchor_top_n: default_semantic_anchor_top_n(),
            protected: default_protected_topics(),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test memory_section_default memory_section_serde memory_section_missing -- --nocapture`
Expected: All 3 PASS

- [ ] **Step 5: Run full test suite + clippy**

Run: `cargo clippy -- -D warnings && cargo test`
Expected: Clean. No existing tests should break — the new field has a serde default so existing TOML files deserialize fine.

- [ ] **Step 6: Commit**

```bash
git add src/memory/anchors.rs src/config/project.rs
git commit -m "feat: add protected memory topics to config and serialize staleness types

Add Serialize to AnchorStatus/StaleFile for JSON output.
Add protected field to MemorySection with default ['gotchas'].
Three-test coverage for serde roundtrip, default, and missing field."
```

---

## Chunk 2: Onboarding Staleness Gathering

### Task 3: Build `gather_protected_memory_state` helper

Extract the staleness-gathering logic into a testable helper function rather than inlining it in the large `call()` method.

**Files:**
- Modify: `src/tools/workflow.rs`

- [ ] **Step 1: Write the failing test for existing protected topic with anchors**

Add to the `tests` module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn onboarding_includes_protected_memories_for_existing_topic() {
    let (dir, ctx) = project_ctx().await;

    // Pre-populate a protected memory with content
    let memories_dir = dir.path().join(".codescout").join("memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::write(
        memories_dir.join("gotchas.md"),
        "# Gotchas\n\n- **Problem:** foo\n  **Fix:** bar\n",
    )
    .unwrap();

    // Create config with protected = ["gotchas"]
    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"gotchas\"]\n",
    )
    .unwrap();

    // Force onboarding
    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let pm = &result["protected_memories"]["gotchas"];
    assert_eq!(pm["exists"], true);
    assert!(pm["content"].as_str().unwrap().contains("# Gotchas"));
    // No anchors file → untracked
    assert_eq!(pm["staleness"]["untracked"], true);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test onboarding_includes_protected_memories_for_existing_topic -- --nocapture`
Expected: FAIL — `protected_memories` field does not exist in the result.

- [ ] **Step 3: Write the `gather_protected_memory_state` function**

Add this function in `src/tools/workflow.rs` (near the other helper functions, before the `impl Tool for Onboarding` block):

```rust
/// Gather staleness state for protected memory topics.
/// Returns a JSON object keyed by topic name, suitable for inclusion
/// in the onboarding result.
fn gather_protected_memory_state(
    memory: &crate::memory::MemoryStore,
    memories_dir: &std::path::Path,
    project_root: &std::path::Path,
    protected: &[String],
) -> Value {
    use crate::memory::anchors::{
        anchor_path_for_topic, check_path_staleness, read_anchor_file,
    };

    // Programmatic topics are always machine-generated — exclude from protection
    const PROGRAMMATIC: &[&str] = &["onboarding", "language-patterns"];

    let mut result = serde_json::Map::new();

    for topic in protected {
        if PROGRAMMATIC.contains(&topic.as_str()) {
            continue;
        }

        let content = match memory.read(topic) {
            Ok(Some(c)) => c,
            _ => {
                // Topic doesn't exist — signal to create fresh
                result.insert(
                    topic.clone(),
                    json!({ "exists": false }),
                );
                continue;
            }
        };

        let anchor_path = anchor_path_for_topic(memories_dir, topic);
        let staleness = if anchor_path.exists() {
            match read_anchor_file(&anchor_path)
                .and_then(|af| check_path_staleness(project_root, &af))
            {
                Ok(report) => json!({
                    "stale_files": report.stale_files,
                    "untracked": false,
                }),
                Err(_) => json!({
                    "stale_files": [],
                    "untracked": true,
                }),
            }
        } else {
            json!({
                "stale_files": [],
                "untracked": true,
            })
        };

        result.insert(
            topic.clone(),
            json!({
                "exists": true,
                "content": content,
                "staleness": staleness,
            }),
        );
    }

    Value::Object(result)
}
```

- [ ] **Step 4: Wire into `Onboarding::call()`**

In the `call()` method, after the programmatic memory writes (~line 882, after the `p.memory.write("language-patterns", ...)` block) and before building the key-files manifest, add:

```rust
// Gather protected memory state for the LLM merge flow
let protected_memories = ctx
    .agent
    .with_project(|p| {
        let memories_dir = p.root.join(".codescout").join("memories");
        let protected = &p.config.memory.protected;
        Ok(gather_protected_memory_state(
            &p.memory,
            &memories_dir,
            &p.root,
            protected,
        ))
    })
    .await?;
```

Then add `"protected_memories": protected_memories,` to the final `json!({...})` return value (alongside `"languages"`, `"instructions"`, etc.).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test onboarding_includes_protected_memories_for_existing_topic -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: gather protected memory staleness in onboarding result

Add gather_protected_memory_state() helper and wire into Onboarding::call().
Protected topics get exists/content/staleness in the JSON result for LLM merge flow."
```

---

### Task 4: Test edge cases for protected memory gathering

**Files:**
- Modify: `src/tools/workflow.rs` (tests only)

- [ ] **Step 1: Test — protected topic that doesn't exist yet**

```rust
#[tokio::test]
async fn onboarding_protected_memory_missing_topic() {
    let (dir, ctx) = project_ctx().await;

    // Config protects "gotchas" but no gotchas.md exists
    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"gotchas\"]\n",
    )
    .unwrap();

    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let pm = &result["protected_memories"]["gotchas"];
    assert_eq!(pm["exists"], false);
    assert!(pm.get("content").is_none());
}
```

- [ ] **Step 2: Test — programmatic topics excluded**

```rust
#[tokio::test]
async fn onboarding_excludes_programmatic_from_protected() {
    let (dir, ctx) = project_ctx().await;

    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"onboarding\", \"language-patterns\", \"gotchas\"]\n",
    )
    .unwrap();

    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let pm = &result["protected_memories"];
    // Programmatic topics excluded
    assert!(pm.get("onboarding").is_none());
    assert!(pm.get("language-patterns").is_none());
    // Non-programmatic topic still present
    assert!(pm.get("gotchas").is_some());
}
```

- [ ] **Step 3: Test — protected topic with no anchor sidecar (untracked)**

```rust
#[tokio::test]
async fn onboarding_protected_memory_untracked_no_anchors() {
    let (dir, ctx) = project_ctx().await;

    let memories_dir = dir.path().join(".codescout").join("memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::write(
        memories_dir.join("gotchas.md"),
        "# Gotchas\n\n- Some gotcha referencing src/main.rs\n",
    )
    .unwrap();
    // No .anchors.toml file created

    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"gotchas\"]\n",
    )
    .unwrap();

    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let staleness = &result["protected_memories"]["gotchas"]["staleness"];
    assert_eq!(staleness["untracked"], true);
    assert_eq!(staleness["stale_files"].as_array().unwrap().len(), 0);
}
```

- [ ] **Step 4: Test — protected topic with stale anchors (file changed)**

This is the core path: anchor sidecar exists, referenced file has changed since the memory was written.

```rust
#[tokio::test]
async fn onboarding_protected_memory_stale_anchors() {
    let (dir, ctx) = project_ctx().await;

    // Write a source file and compute its hash
    let src_file = dir.path().join("main.rs");
    std::fs::write(&src_file, "fn main() {}").unwrap();
    let original_hash = crate::embed::index::hash_file(&src_file).unwrap();

    // Create a protected memory referencing that file
    let memories_dir = dir.path().join(".codescout").join("memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::write(
        memories_dir.join("gotchas.md"),
        "# Gotchas\n\n- **Problem:** main.rs has issue\n  **Fix:** fix it\n",
    )
    .unwrap();

    // Create anchor sidecar with the original hash
    use crate::memory::anchors::{AnchorFile, PathAnchor, write_anchor_file, anchor_path_for_topic};
    let anchor_file = AnchorFile {
        anchors: vec![PathAnchor {
            path: "main.rs".to_string(),
            hash: original_hash,
        }],
    };
    let anchor_path = anchor_path_for_topic(&memories_dir, "gotchas");
    write_anchor_file(&anchor_path, &anchor_file).unwrap();

    // Now modify the source file so the hash changes
    std::fs::write(&src_file, "fn main() { println!(\"changed\"); }").unwrap();

    // Config
    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"gotchas\"]\n",
    )
    .unwrap();

    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let staleness = &result["protected_memories"]["gotchas"]["staleness"];
    assert_eq!(staleness["untracked"], false);
    let stale_files = staleness["stale_files"].as_array().unwrap();
    assert_eq!(stale_files.len(), 1);
    assert_eq!(stale_files[0]["path"], "main.rs");
    assert_eq!(stale_files[0]["status"], "changed");
}
```

- [ ] **Step 5: Test — protected topic with fresh anchors (all files unchanged)**

```rust
#[tokio::test]
async fn onboarding_protected_memory_fresh_anchors() {
    let (dir, ctx) = project_ctx().await;

    // Write a source file and compute its hash
    let src_file = dir.path().join("main.rs");
    std::fs::write(&src_file, "fn main() {}").unwrap();
    let current_hash = crate::embed::index::hash_file(&src_file).unwrap();

    // Create a protected memory referencing that file
    let memories_dir = dir.path().join(".codescout").join("memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::write(
        memories_dir.join("gotchas.md"),
        "# Gotchas\n\n- **Problem:** main.rs has issue\n  **Fix:** fix it\n",
    )
    .unwrap();

    // Create anchor sidecar with the CURRENT hash (file hasn't changed)
    use crate::memory::anchors::{AnchorFile, PathAnchor, write_anchor_file, anchor_path_for_topic};
    let anchor_file = AnchorFile {
        anchors: vec![PathAnchor {
            path: "main.rs".to_string(),
            hash: current_hash,
        }],
    };
    let anchor_path = anchor_path_for_topic(&memories_dir, "gotchas");
    write_anchor_file(&anchor_path, &anchor_file).unwrap();

    // Do NOT modify the source file — it stays the same

    // Config
    let config_path = dir.path().join(".codescout").join("project.toml");
    std::fs::write(
        &config_path,
        "[project]\nname = \"test\"\nlanguages = [\"rust\"]\n\n[memory]\nprotected = [\"gotchas\"]\n",
    )
    .unwrap();

    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    let staleness = &result["protected_memories"]["gotchas"]["staleness"];
    assert_eq!(staleness["untracked"], false);
    assert_eq!(staleness["stale_files"].as_array().unwrap().len(), 0);
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test onboarding_protected_memory onboarding_excludes_programmatic -- --nocapture`
Expected: All 5 PASS

- [ ] **Step 7: Run full suite + clippy**

Run: `cargo clippy -- -D warnings && cargo test`
Expected: Clean

- [ ] **Step 8: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "test: edge cases for protected memory gathering

Cover missing topic, programmatic exclusion, untracked anchors,
stale anchors (file changed), and fresh anchors (file unchanged)."
```

---

## Chunk 3: Onboarding Prompt Update

### Task 5: Update onboarding prompt with merge flow

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`

- [ ] **Step 1: Read the current Phase 2 section**

Read `src/prompts/onboarding_prompt.md` heading `## Phase 2: Write the 6 Memories` to understand the current structure. The new conditional flow inserts **before** the "### Memories to Create" subsection.

- [ ] **Step 2: Add the protected memory flow**

After the "### Rules" subsection (line ~295) and before "### Memories to Create" (line ~296), insert:

```markdown
### Protected Memories

Check the `protected_memories` field from the onboarding tool response above. For
each memory you are about to write, check whether it appears there:

**If `protected_memories[topic].exists == false`:** Create fresh as normal.

**If `protected_memories[topic].exists == true` AND `staleness.untracked == false`
AND `staleness.stale_files` is empty:** The memory is fresh — all anchored source
files are unchanged. **Skip writing this topic entirely.** Tell the user:
> "Kept `[topic]` unchanged (all references still valid)."

**If `protected_memories[topic].exists == true` AND (`staleness.untracked == true`
OR `staleness.stale_files` is non-empty):** Run the merge flow:

1. The existing content is in `protected_memories[topic].content`.
2. For entries referencing files listed in `staleness.stale_files` (or all
   entries if `untracked`): use `find_symbol`, `read_file`, `search_pattern`
   to verify whether each entry is still accurate.
3. Identify new discoveries from your Phase 1 exploration that belong in
   this memory.
4. Present a diff-style summary to the user:
   - **Stale (recommend removing):** [entries no longer accurate, with reason]
   - **Still valid (keeping):** [verified entries]
   - **New findings:** [discoveries from exploration]
   - **Proposed merged version:** [full content]
5. **Wait for user approval** before calling `memory(action="write")`.

**If a topic is NOT in `protected_memories`:** Write it as normal (overwrite).

The protected topics list is configured in `project.toml` under `[memory] protected`.
Users can add custom topics. The programmatic memories (`onboarding`, `language-patterns`)
are always excluded from protection.
```

- [ ] **Step 3: Update the "### 6. `gotchas`" section**

In the existing `gotchas` template section (~line 440), add a note at the top:

```markdown
> **Note:** `gotchas` is protected by default. If it already exists and the
> onboarding result shows it in `protected_memories`, follow the Protected
> Memories flow above instead of overwriting.
```

- [ ] **Step 4: Verify the prompt compiles into the binary**

Run: `cargo build`
Expected: Success. The prompt is embedded via `include_str!` so any syntax issues in the template are caught at build time only if the file is missing — but this confirms nothing is broken.

- [ ] **Step 5: Commit**

```bash
git add src/prompts/onboarding_prompt.md
git commit -m "feat: add protected memory merge flow to onboarding prompt

Instruct LLM to check protected_memories field, skip fresh topics,
verify stale/untracked entries against codebase, and present diff-style
merge summary for user approval before writing."
```

---

### Task 6: Final integration test + cleanup

**Files:**
- Modify: `src/tools/workflow.rs` (test only)

- [ ] **Step 1: Write integration test — force onboarding with protected memory produces all expected fields**

```rust
#[tokio::test]
async fn onboarding_force_with_protected_memory_full_flow() {
    let (dir, ctx) = project_ctx().await;

    // First onboarding — creates everything fresh
    let _ = Onboarding.call(json!({}), &ctx).await.unwrap();

    // Manually write a gotchas memory to simulate user curation
    let memories_dir = dir.path().join(".codescout").join("memories");
    std::fs::write(
        memories_dir.join("gotchas.md"),
        "# Gotchas\n\n- **Problem:** custom user gotcha\n  **Fix:** do the thing\n",
    )
    .unwrap();

    // Force re-onboarding
    let result = Onboarding
        .call(json!({ "force": true }), &ctx)
        .await
        .unwrap();

    // Should have both standard fields and protected_memories
    assert!(result.get("languages").is_some());
    assert!(result.get("instructions").is_some());
    assert!(result.get("protected_memories").is_some());

    let pm = &result["protected_memories"]["gotchas"];
    assert_eq!(pm["exists"], true);
    assert!(pm["content"].as_str().unwrap().contains("custom user gotcha"));
    // No anchor sidecar was created, so staleness should be untracked
    assert_eq!(pm["staleness"]["untracked"], true);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test onboarding_force_with_protected_memory_full_flow -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run full suite + clippy + fmt**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: All clean, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "test: integration test for protected memories in force onboarding"
```
