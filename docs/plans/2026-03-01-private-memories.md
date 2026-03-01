# Private Memories Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a gitignored per-developer private memory store alongside the existing shared store, surfaced every session via the onboarding status response, with LLM routing rules injected into `system-prompt.md` at onboarding time.

**Architecture:** A second `MemoryStore` instance (pointing at `.code-explorer/private-memories/`) is added to `ActiveProject`. Four existing memory tools gain an optional `private` boolean that routes to the right store. The `onboarding` tool's already-onboarded fast path lists private topics alongside shared ones. `build_system_prompt_draft` appends Private Memory Rules to the generated `system-prompt.md`.

**Tech Stack:** Rust, serde_json, tempfile (tests), existing `MemoryStore` / `Agent` / `Tool` patterns.

---

### Task 1: `MemoryStore::open_private` + auto-gitignore

**Files:**
- Modify: `src/memory/mod.rs`

**Background:** `MemoryStore` is a thin wrapper around a `memories_dir: PathBuf`. The `open` constructor points it at `.code-explorer/memories/`. We add `open_private` pointing at `.code-explorer/private-memories/`, plus a private helper `ensure_gitignored` that idempotently adds an entry to `.gitignore`.

**Step 1: Write the failing tests**

Add to the `tests` module in `src/memory/mod.rs` (before the closing `}`):

```rust
#[test]
fn open_private_creates_private_memories_dir() {
    let dir = tempdir().unwrap();
    let _store = MemoryStore::open_private(dir.path()).unwrap();
    assert!(dir.path().join(".code-explorer/private-memories").exists());
}

#[test]
fn open_private_adds_to_gitignore() {
    let dir = tempdir().unwrap();
    MemoryStore::open_private(dir.path()).unwrap();
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains(".code-explorer/private-memories/"));
}

#[test]
fn open_private_does_not_duplicate_gitignore_entry() {
    let dir = tempdir().unwrap();
    MemoryStore::open_private(dir.path()).unwrap();
    MemoryStore::open_private(dir.path()).unwrap();
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    let count = content
        .lines()
        .filter(|l| l.trim() == ".code-explorer/private-memories/")
        .count();
    assert_eq!(count, 1);
}

#[test]
fn open_private_appends_to_existing_gitignore() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    MemoryStore::open_private(dir.path()).unwrap();
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains("target/\n"));
    assert!(content.contains(".code-explorer/private-memories/"));
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test -p code-explorer --lib memory::tests::open_private 2>&1 | grep -E "FAILED|error\[|^error"
```

Expected: compile error — `open_private` not found.

**Step 3: Implement `open_private` and `ensure_gitignored`**

Add to `impl MemoryStore` in `src/memory/mod.rs`, after the existing `open` method:

```rust
/// Open (or create) the private memory store for a project root.
/// Private memories are gitignored — not shared with teammates.
/// Automatically adds `.code-explorer/private-memories/` to `.gitignore`.
pub fn open_private(project_root: &Path) -> Result<Self> {
    let memories_dir = project_root
        .join(".code-explorer")
        .join("private-memories");
    std::fs::create_dir_all(&memories_dir)?;
    Self::ensure_gitignored(project_root, ".code-explorer/private-memories/")?;
    Ok(Self { memories_dir })
}

fn ensure_gitignored(project_root: &Path, entry: &str) -> Result<()> {
    let gitignore_path = project_root.join(".gitignore");
    let existing = if gitignore_path.exists() {
        std::fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(entry);
    content.push('\n');
    std::fs::write(&gitignore_path, content)?;
    Ok(())
}
```

**Step 4: Run tests to confirm they pass**

```bash
cargo test -p code-explorer --lib memory::tests::open_private
```

Expected: 4 tests pass.

**Step 5: Verify no regressions**

```bash
cargo test -p code-explorer --lib memory
```

**Step 6: Commit**

```bash
git add src/memory/mod.rs
git commit -m "feat(memory): add open_private constructor with auto-gitignore"
```

---

### Task 2: Wire `private_memory` into `ActiveProject` and `Agent`

**Files:**
- Modify: `src/agent.rs`

**Background:** `ActiveProject` holds one `memory: MemoryStore`. We add `private_memory: MemoryStore` so all tools can access both stores via `with_project`. Both `Agent::new` (startup) and `Agent::activate` (runtime switch) must create both stores.

**Step 1: Write the failing test**

In `src/agent.rs`, add to the `tests` module:

```rust
#[tokio::test]
async fn active_project_has_private_memory() {
    let dir = tempdir().unwrap();
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    agent
        .with_project(|p| {
            p.private_memory.write("pref", "verbose")?;
            assert_eq!(
                p.private_memory.read("pref")?,
                Some("verbose".to_string())
            );
            // private is isolated from shared
            assert_eq!(p.memory.read("pref")?, None);
            Ok(())
        })
        .await
        .unwrap();
}
```

**Step 2: Run test to confirm it fails**

```bash
cargo test -p code-explorer --lib agent::tests::active_project_has_private_memory 2>&1 | grep -E "error|FAILED"
```

Expected: compile error — `private_memory` field not found.

**Step 3: Add `private_memory` field to `ActiveProject`**

In `src/agent.rs`, update the `ActiveProject` struct:

```rust
pub struct ActiveProject {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub memory: MemoryStore,
    pub private_memory: MemoryStore,
    pub library_registry: LibraryRegistry,
}
```

**Step 4: Update `Agent::new`**

In the `Some(root)` branch of `Agent::new`, add after `let memory = MemoryStore::open(&root)?;`:

```rust
let private_memory = MemoryStore::open_private(&root)?;
```

And include it in the `ActiveProject { ... }` literal:

```rust
Some(ActiveProject {
    root,
    config,
    memory,
    private_memory,
    library_registry,
})
```

**Step 5: Update `Agent::activate`**

Find the `activate` method and make the same two changes — add `open_private` and include `private_memory` in the struct literal. The existing pattern is identical to `new`.

**Step 6: Run test to confirm it passes**

```bash
cargo test -p code-explorer --lib agent::tests::active_project_has_private_memory
```

**Step 7: Verify no regressions**

```bash
cargo test -p code-explorer --lib agent
```

**Step 8: Commit**

```bash
git add src/agent.rs
git commit -m "feat(agent): add private_memory field to ActiveProject"
```

---

### Task 3: Add `private` parameter to tool input schemas

**Files:**
- Modify: `src/tools/memory.rs`

**Background:** Tool schemas define what the LLM can pass. Adding `private?: bool` to `write_memory`, `read_memory`, `delete_memory` and `include_private?: bool` to `list_memories` changes nothing functionally yet (routing comes in Task 4) but lets us write schema tests first.

**Step 1: Write the failing tests**

Add to the `tests` module in `src/tools/memory.rs`:

```rust
#[test]
fn write_memory_schema_has_private_field() {
    let schema = WriteMemory.input_schema();
    assert!(schema["properties"]["private"].is_object());
    assert_eq!(schema["properties"]["private"]["type"], "boolean");
}

#[test]
fn read_memory_schema_has_private_field() {
    let schema = ReadMemory.input_schema();
    assert!(schema["properties"]["private"].is_object());
    assert_eq!(schema["properties"]["private"]["type"], "boolean");
}

#[test]
fn delete_memory_schema_has_private_field() {
    let schema = DeleteMemory.input_schema();
    assert!(schema["properties"]["private"].is_object());
    assert_eq!(schema["properties"]["private"]["type"], "boolean");
}

#[test]
fn list_memories_schema_has_include_private_field() {
    let schema = ListMemories.input_schema();
    assert!(schema["properties"]["include_private"].is_object());
    assert_eq!(schema["properties"]["include_private"]["type"], "boolean");
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test -p code-explorer --lib tools::memory::tests::write_memory_schema
cargo test -p code-explorer --lib tools::memory::tests::read_memory_schema
cargo test -p code-explorer --lib tools::memory::tests::delete_memory_schema
cargo test -p code-explorer --lib tools::memory::tests::list_memories_schema
```

**Step 3: Update the four `input_schema` methods**

Replace `WriteMemory::input_schema`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["topic", "content"],
        "properties": {
            "topic": { "type": "string" },
            "content": { "type": "string" },
            "private": {
                "type": "boolean",
                "description": "If true, write to the gitignored private store (personal/machine-specific notes, not shared with teammates)."
            }
        }
    })
}
```

Replace `ReadMemory::input_schema`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["topic"],
        "properties": {
            "topic": { "type": "string" },
            "private": {
                "type": "boolean",
                "description": "If true, read from the private memory store."
            }
        }
    })
}
```

Replace `DeleteMemory::input_schema`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["topic"],
        "properties": {
            "topic": { "type": "string" },
            "private": {
                "type": "boolean",
                "description": "If true, delete from the private memory store."
            }
        }
    })
}
```

Replace `ListMemories::input_schema`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "include_private": {
                "type": "boolean",
                "description": "If true, also list private memory topics. Returns { shared, private } instead of { topics }."
            }
        }
    })
}
```

**Step 4: Run tests to confirm they pass**

```bash
cargo test -p code-explorer --lib tools::memory::tests
```

**Step 5: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat(tools/memory): add private/include_private params to schemas"
```

---

### Task 4: Route tool calls to the private store

**Files:**
- Modify: `src/tools/memory.rs`

**Background:** Now wire up the `private` flag so calls actually hit `p.private_memory`. The pattern is: read `input["private"].as_bool().unwrap_or(false)`, then branch on `if private { &p.private_memory } else { &p.memory }` (for reads) or `if private { p.private_memory.write(...) } else { p.memory.write(...) }` (for writes).

**Step 1: Write the failing tests**

Add to the `tests` module in `src/tools/memory.rs`:

```rust
#[tokio::test]
async fn write_private_goes_to_private_store() {
    let (_dir, ctx) = test_ctx_with_project();
    WriteMemory
        .call(json!({"topic": "prefs", "content": "verbose", "private": true}), &ctx)
        .await
        .unwrap();
    // not in shared store
    let shared = ctx
        .agent
        .with_project(|p| p.memory.read("prefs"))
        .await
        .unwrap();
    assert_eq!(shared, None);
    // is in private store
    let private = ctx
        .agent
        .with_project(|p| p.private_memory.read("prefs"))
        .await
        .unwrap();
    assert_eq!(private, Some("verbose".to_string()));
}

#[tokio::test]
async fn read_private_reads_from_private_store() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| p.private_memory.write("wip", "issue-42"))
        .await
        .unwrap();
    let result = ReadMemory
        .call(json!({"topic": "wip", "private": true}), &ctx)
        .await
        .unwrap();
    assert_eq!(result["content"], "issue-42");
}

#[tokio::test]
async fn read_private_does_not_see_shared() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| p.memory.write("shared-topic", "data"))
        .await
        .unwrap();
    let result = ReadMemory
        .call(json!({"topic": "shared-topic", "private": true}), &ctx)
        .await
        .unwrap();
    assert!(result["content"].is_null());
}

#[tokio::test]
async fn delete_private_removes_from_private_store() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| p.private_memory.write("tmp", "gone"))
        .await
        .unwrap();
    DeleteMemory
        .call(json!({"topic": "tmp", "private": true}), &ctx)
        .await
        .unwrap();
    let result = ctx
        .agent
        .with_project(|p| p.private_memory.read("tmp"))
        .await
        .unwrap();
    assert_eq!(result, None);
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test -p code-explorer --lib "tools::memory::tests::write_private\|read_private\|delete_private" 2>&1 | grep -E "FAILED|error"
```

Expected: tests compile but fail — private routing not implemented yet.

**Step 3: Implement routing in `WriteMemory::call`**

Replace the existing `call` body:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    let topic = super::require_str_param(&input, "topic")?;
    let content = super::require_str_param(&input, "content")?;
    let private = input["private"].as_bool().unwrap_or(false);
    ctx.agent
        .with_project(|p| {
            if private {
                p.private_memory.write(topic, content)?;
            } else {
                p.memory.write(topic, content)?;
            }
            Ok(json!({ "status": "ok", "topic": topic }))
        })
        .await
}
```

**Step 4: Implement routing in `ReadMemory::call`**

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    let topic = super::require_str_param(&input, "topic")?;
    let private = input["private"].as_bool().unwrap_or(false);
    ctx.agent
        .with_project(|p| {
            let store = if private { &p.private_memory } else { &p.memory };
            match store.read(topic)? {
                Some(content) => Ok(json!({ "topic": topic, "content": content })),
                None => Ok(json!({ "topic": topic, "content": null, "message": "not found" })),
            }
        })
        .await
}
```

**Step 5: Implement routing in `DeleteMemory::call`**

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    let topic = super::require_str_param(&input, "topic")?;
    let private = input["private"].as_bool().unwrap_or(false);
    ctx.agent
        .with_project(|p| {
            if private {
                p.private_memory.delete(topic)?;
            } else {
                p.memory.delete(topic)?;
            }
            Ok(json!({ "status": "ok", "topic": topic }))
        })
        .await
}
```

**Step 6: Run tests to confirm they pass**

```bash
cargo test -p code-explorer --lib tools::memory::tests
```

**Step 7: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat(tools/memory): route private=true calls to private store"
```

---

### Task 5: `list_memories` private output + `format_list_memories`

**Files:**
- Modify: `src/tools/memory.rs`
- Modify: `src/tools/user_format.rs`

**Background:** `list_memories` currently returns `{ "topics": [...] }`. When `include_private=true`, return `{ "shared": [...], "private": [...] }` instead. `format_list_memories` in `user_format.rs` must handle both shapes.

**Step 1: Write the failing tests**

Add to `src/tools/memory.rs` tests:

```rust
#[tokio::test]
async fn list_memories_default_returns_topics_key() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| p.memory.write("arch", "..."))
        .await
        .unwrap();
    let result = ListMemories
        .call(json!({}), &ctx)
        .await
        .unwrap();
    assert!(result["topics"].is_array());
    assert!(result["shared"].is_null()); // old shape preserved by default
}

#[tokio::test]
async fn list_memories_include_private_returns_shared_and_private_keys() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| {
            p.memory.write("arch", "...")?;
            p.private_memory.write("prefs", "...")?;
            Ok(())
        })
        .await
        .unwrap();
    let result = ListMemories
        .call(json!({"include_private": true}), &ctx)
        .await
        .unwrap();
    assert!(result["shared"].is_array());
    assert!(result["private"].is_array());
    assert!(result["topics"].is_null()); // new shape, no "topics" key
    let shared: Vec<_> = result["shared"].as_array().unwrap().iter()
        .filter_map(|v| v.as_str()).collect();
    assert!(shared.contains(&"arch"));
    let private: Vec<_> = result["private"].as_array().unwrap().iter()
        .filter_map(|v| v.as_str()).collect();
    assert!(private.contains(&"prefs"));
}

#[tokio::test]
async fn list_memories_include_private_empty_private_store() {
    let (_dir, ctx) = test_ctx_with_project();
    ctx.agent
        .with_project(|p| p.memory.write("arch", "..."))
        .await
        .unwrap();
    let result = ListMemories
        .call(json!({"include_private": true}), &ctx)
        .await
        .unwrap();
    let private = result["private"].as_array().unwrap();
    assert!(private.is_empty());
}
```

Add to `src/tools/user_format.rs` tests (in the `tests` module):

```rust
#[test]
fn format_list_memories_include_private_shows_both() {
    let result = json!({ "shared": ["arch", "conventions"], "private": ["prefs"] });
    let out = format_list_memories(&result);
    assert!(out.contains("2 shared"));
    assert!(out.contains("1 private"));
    assert!(out.contains("arch"));
    assert!(out.contains("prefs"));
}

#[test]
fn format_list_memories_include_private_empty_private() {
    let result = json!({ "shared": ["arch"], "private": [] });
    let out = format_list_memories(&result);
    assert!(out.contains("1 shared"));
    assert!(out.contains("0 private"));
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test -p code-explorer --lib "list_memories_include_private\|format_list_memories_include" 2>&1 | grep -E "FAILED|error"
```

**Step 3: Update `ListMemories::call`**

Replace the call body in `src/tools/memory.rs`:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    let include_private = input["include_private"].as_bool().unwrap_or(false);
    ctx.agent
        .with_project(|p| {
            if include_private {
                let shared = p.memory.list()?;
                let private = p.private_memory.list()?;
                Ok(json!({ "shared": shared, "private": private }))
            } else {
                let topics = p.memory.list()?;
                Ok(json!({ "topics": topics }))
            }
        })
        .await
}
```

**Step 4: Update `format_list_memories` in `src/tools/user_format.rs`**

Replace the existing `format_list_memories` function:

```rust
pub fn format_list_memories(result: &Value) -> String {
    // include_private=true path: { shared: [...], private: [...] }
    if let (Some(shared), Some(private)) = (
        result["shared"].as_array(),
        result["private"].as_array(),
    ) {
        let mut out = format!("{} shared, {} private", shared.len(), private.len());
        for t in shared {
            if let Some(name) = t.as_str() {
                out.push_str(&format!("\n  {name}"));
            }
        }
        if !private.is_empty() {
            out.push_str("\n  -- private --");
            for t in private {
                if let Some(name) = t.as_str() {
                    out.push_str(&format!("\n  {name}"));
                }
            }
        }
        return out;
    }
    // Default path: { topics: [...] }
    let topics = match result["topics"].as_array() {
        Some(t) if !t.is_empty() => t,
        _ => return "0 topics".to_string(),
    };
    let mut out = format!("{} topics", topics.len());
    for topic in topics.iter() {
        if let Some(name) = topic.as_str() {
            out.push_str(&format!("\n  {name}"));
        }
    }
    out
}
```

**Step 5: Run all memory tool tests**

```bash
cargo test -p code-explorer --lib tools::memory
cargo test -p code-explorer --lib tools::user_format::tests::format_list_memories
```

**Step 6: Commit**

```bash
git add src/tools/memory.rs src/tools/user_format.rs
git commit -m "feat(tools/memory): list_memories include_private returns shared+private shape"
```

---

### Task 6: Surface private memories in the onboarding status response

**Files:**
- Modify: `src/tools/workflow.rs`

**Background:** The `onboarding` tool's fast path (already-onboarded) currently builds a `message` listing shared memories and returns early. Extend it to also list private memories in the response and message — but only when any private memories exist (no noise when the store is empty).

**Step 1: Write the failing tests**

Find the existing test `onboarding_status_includes_memories_and_message` in `src/tools/workflow.rs` to understand the pattern, then add:

```rust
#[tokio::test]
async fn onboarding_status_includes_private_memories_when_present() {
    let (dir, ctx) = project_ctx();
    // Set up: write onboarding memory (triggers fast path) + a private memory
    ctx.agent
        .with_project(|p| {
            p.memory.write("onboarding", "done")?;
            p.private_memory.write("my-prefs", "verbose")
        })
        .await
        .unwrap();
    // Create project.toml (also required for fast path)
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
    std::fs::write(
        dir.path().join(".code-explorer/project.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();
    let result = Onboarding.call(json!({}), &ctx).await.unwrap();
    assert!(result["onboarded"].as_bool().unwrap_or(false));
    let private = result["private_memories"].as_array().unwrap();
    assert!(private.iter().any(|v| v.as_str() == Some("my-prefs")));
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("my-prefs"));
}

#[tokio::test]
async fn onboarding_status_omits_private_memories_field_when_empty() {
    let (dir, ctx) = project_ctx();
    ctx.agent
        .with_project(|p| p.memory.write("onboarding", "done"))
        .await
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
    std::fs::write(
        dir.path().join(".code-explorer/project.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();
    let result = Onboarding.call(json!({}), &ctx).await.unwrap();
    assert!(result["onboarded"].as_bool().unwrap_or(false));
    assert!(result["private_memories"].is_null());
    assert!(!result["message"].as_str().unwrap().contains("private"));
}
```

**Step 2: Run to confirm they fail**

```bash
cargo test -p code-explorer --lib "tools::workflow::tests::onboarding_status_includes_private\|omits_private" 2>&1 | grep -E "FAILED|error"
```

**Step 3: Update the fast path in `Onboarding::call`**

Find the already-onboarded block (around line 277 in `src/tools/workflow.rs`). Replace the `with_project` call and the response:

```rust
let status = ctx
    .agent
    .with_project(|p| {
        let has_config = p.root.join(".code-explorer").join("project.toml").exists();
        let memories = p.memory.list()?;
        let has_onboarding_memory = memories.iter().any(|m| m == "onboarding");
        let private_memories = p.private_memory.list()?;
        Ok((has_config, has_onboarding_memory, memories, private_memories))
    })
    .await?;
let (has_config, has_onboarding_memory, memories, private_memories) = status;
if has_config && has_onboarding_memory {
    let mut message = format!(
        "Onboarding already performed. Available shared memories: {}. \
         Use `read_memory(topic)` to read relevant ones as needed for your current task. \
         Do not read all memories at once — only read those relevant to what you're working on.",
        memories.join(", ")
    );
    if !private_memories.is_empty() {
        message.push_str(&format!(
            " Private memories: {}. Read with `read_memory(topic, private=true)`.",
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
```

**Step 4: Run tests**

```bash
cargo test -p code-explorer --lib tools::workflow::tests::onboarding_status
```

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(onboarding): surface private memories in already-onboarded status response"
```

---

### Task 7: Add Private Memory Rules to `build_system_prompt_draft`

**Files:**
- Modify: `src/tools/workflow.rs`

**Background:** `build_system_prompt_draft` generates the scaffold for `.code-explorer/system-prompt.md` during a fresh `onboarding`. We append a "Private Memory Rules" section so every session (where `system-prompt.md` is loaded as custom instructions) teaches the LLM when to use each store.

**Step 1: Write the failing test**

Add to `src/tools/workflow.rs` tests:

```rust
#[tokio::test]
async fn system_prompt_draft_includes_private_memory_rules() {
    let (dir, ctx) = project_ctx();
    // Fresh onboarding (no prior config)
    let result = Onboarding.call(json!({}), &ctx).await.unwrap();
    let draft = result["system_prompt_draft"].as_str().unwrap();
    assert!(draft.contains("Private Memory Rules"));
    assert!(draft.contains("private=true"));
    assert!(draft.contains("gitignored"));
}
```

**Step 2: Run to confirm it fails**

```bash
cargo test -p code-explorer --lib tools::workflow::tests::system_prompt_draft_includes_private
```

**Step 3: Append the section in `build_system_prompt_draft`**

At the end of `build_system_prompt_draft` in `src/tools/workflow.rs`, just before the final `draft` is returned, add:

```rust
// Private memory rules
draft.push_str("## Private Memory Rules\n\n");
draft.push_str(
    "Private memories are gitignored — personal to this developer, not shared with the team.\n\
     They live in `.code-explorer/private-memories/`.\n\n\
     **Write to the private store** (`write_memory(topic, content, private=true)`) for:\n\
     - Personal preferences and workflow rules for this developer\n\
     - Machine-specific config (local ports, paths, GPU type, env quirks)\n\
     - WIP notes and in-progress debugging context\n\
     - Personal debugging history specific to this setup\n\n\
     **Write to the shared store** (`write_memory(topic, content)`) for:\n\
     - Architecture, conventions, design patterns — knowledge useful to ALL contributors\n\
     - When in doubt: private first, promote to shared only if universally applicable\n\n\
     **Each session:** `list_memories(include_private=true)` to see what's available.\n"
);
```

**Step 4: Run tests**

```bash
cargo test -p code-explorer --lib tools::workflow::tests::system_prompt_draft
```

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(onboarding): inject Private Memory Rules into system_prompt_draft"
```

---

### Task 8: Update `onboarding_prompt.md` with private memory addendum

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`

**Background:** The onboarding prompt guides the LLM to create 6 shared memories during a fresh onboarding run. We add a brief optional section at the end prompting the LLM to also consider creating personal private memories now.

**Step 1: No test needed**

This is a markdown template change. The content is validated indirectly by reading the rendered prompt in existing onboarding tests.

**Step 2: Append to `src/prompts/onboarding_prompt.md`**

Add at the very end of the file:

```markdown
---

## Optional: Private Memories

After creating the 6 shared memories above, check if any personal context is worth
capturing now. Use `write_memory(topic, content, private=true)` for anything specific
to your setup — local machine config, personal workflow preferences, or current WIP
context. This is optional; skip if nothing personal applies yet.
```

**Step 3: Verify the onboarding tests still pass**

```bash
cargo test -p code-explorer --lib tools::workflow::tests::onboarding
```

**Step 4: Commit**

```bash
git add src/prompts/onboarding_prompt.md
git commit -m "docs(onboarding): add optional private memory prompt to onboarding instructions"
```

---

### Task 9: Full verification

**Step 1: Run the complete test suite**

```bash
cargo test
```

Expected: all tests pass (533 + new tests).

**Step 2: Clippy**

```bash
cargo clippy -- -D warnings
```

Fix any warnings before continuing.

**Step 3: Format**

```bash
cargo fmt
```

**Step 4: Final commit if any fmt changes**

```bash
git add -p
git commit -m "style: cargo fmt"
```

**Step 5: Smoke test manually (optional but recommended)**

```bash
cargo run -- start --project .
```

In a second terminal, call `onboarding` via MCP. Verify:
1. Fresh onboarding: `system_prompt_draft` contains "Private Memory Rules"
2. Write a private memory: `write_memory("test", "hello", private=true)`
3. `list_memories(include_private=true)` shows `{ shared: [...], private: ["test"] }`
4. `.code-explorer/private-memories/test.md` exists
5. `.gitignore` contains `.code-explorer/private-memories/`
6. Re-run `onboarding` (fast path): response contains `private_memories: ["test"]`
