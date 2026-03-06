# Pre-Onboarding Index Gate — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Onboarding detects semantic index status and lets the user choose to build it, skip, or build from CLI before exploration begins.

**Architecture:** Rust probes `embeddings.db` (read-only SQLite query), passes status to prompt via `build_onboarding_prompt`. Prompt handles UX with Phase 0 decision flow. Exploration steps unchanged — Step 6 already has `search_pattern` fallback.

**Tech Stack:** Rust, SQLite (rusqlite), serde_json, Markdown prompt template

---

### Task 1: Add index status probe to `Onboarding::call`

**Files:**
- Modify: `src/tools/workflow.rs:420-469` (inside `Onboarding::call`, after `gather_project_context`)

**Step 1: Add the index probe**

After `let gathered = gather_project_context(&root);` (~line 420) and before the
`build_onboarding_prompt` call (~line 437), insert:

```rust
// Probe embedding index status (read-only, no network)
let index_status = {
    let db_path = root.join(".codescout").join("embeddings.db");
    if db_path.exists() {
        match crate::embed::index::open_db(&root)
            .and_then(|conn| crate::embed::index::index_stats(&conn))
        {
            Ok(stats) => json!({
                "ready": stats.chunk_count > 0,
                "files": stats.file_count,
                "chunks": stats.chunk_count,
            }),
            Err(_) => json!({ "ready": false, "files": 0, "chunks": 0 }),
        }
    } else {
        json!({ "ready": false, "files": 0, "chunks": 0 })
    }
};
```

**Step 2: Pass index info to `build_onboarding_prompt`**

Update the `build_onboarding_prompt` call to include index status:

```rust
let prompt = crate::prompts::build_onboarding_prompt(
    &lang_list,
    &top_level,
    &key_files,
    &gathered.ci_files,
    &gathered.entry_points,
    &gathered.test_dirs,
    index_status["ready"].as_bool().unwrap_or(false),
    index_status["files"].as_u64().unwrap_or(0) as usize,
    index_status["chunks"].as_u64().unwrap_or(0) as usize,
);
```

**Step 3: Add `index_status` to the response JSON**

In the final `Ok(json!({...}))` block, add after `"features_suggestion"`:

```rust
"index_status": index_status,
```

**Step 4: Run `cargo check`**

Run: `cargo check 2>&1 | tail -20`
Expected: Errors about `build_onboarding_prompt` signature mismatch (we haven't updated it yet). That's fine — Task 2 fixes it.

**Step 5: Commit (hold — will commit with Task 2)**

---

### Task 2: Update `build_onboarding_prompt` signature and injection

**Files:**
- Modify: `src/prompts/mod.rs:72-128`

**Step 1: Add index parameters to function signature**

```rust
pub fn build_onboarding_prompt(
    languages: &[String],
    top_level: &[String],
    key_files: &[String],
    ci_files: &[String],
    entry_points: &[String],
    test_dirs: &[String],
    index_ready: bool,
    index_files: usize,
    index_chunks: usize,
) -> String {
```

**Step 2: Inject index status into the gathered data section**

After the existing `key_files` block (just before the final `prompt`) add:

```rust
if index_ready {
    prompt.push_str(&format!(
        "**Semantic index:** ready ({} files, {} chunks)\n\n",
        index_files, index_chunks
    ));
} else {
    prompt.push_str("**Semantic index:** not built\n\n");
}
```

**Step 3: Run `cargo check`**

Run: `cargo check 2>&1 | tail -20`
Expected: Errors in tests — existing calls to `build_onboarding_prompt` need the new params.

---

### Task 3: Fix existing tests for new signature

**Files:**
- Modify: `src/prompts/mod.rs` (tests module, ~line 200+)

**Step 1: Update all `build_onboarding_prompt` test calls**

Every existing call needs three new trailing arguments: `false, 0, 0` (index not ready).

`build_onboarding_includes_languages` (~line 200):
```rust
let result = build_onboarding_prompt(
    &["rust".into(), "python".into()],
    &["src/".into(), "tests/".into()],
    &[], &[], &[], &[],
    false, 0, 0,
);
```

`build_onboarding_handles_empty` (~line 214):
```rust
let result = build_onboarding_prompt(&[], &[], &[], &[], &[], &[], false, 0, 0);
```

`build_onboarding_includes_gathered_context` (~line 220):
```rust
let result = build_onboarding_prompt(
    &["rust".into(), "python".into()],
    &["src/".into(), "tests/".into()],
    &["README.md".into(), "Cargo.toml".into(), "CLAUDE.md".into()],
    &[".github/workflows/ci.yml".into()],
    &["src/main.rs".into()],
    &["tests".into()],
    false, 0, 0,
);
```

**Step 2: Run `cargo check`**

Run: `cargo check 2>&1 | tail -5`
Expected: Clean compilation.

**Step 3: Run tests**

Run: `cargo test -p codescout --lib prompts 2>&1 | tail -20`
Expected: All existing prompt tests pass.

**Step 4: Commit**

```bash
git add src/tools/workflow.rs src/prompts/mod.rs
git commit -m "feat: probe embedding index status during onboarding

- Add read-only SQLite query for index stats in Onboarding::call
- Pass index_ready/files/chunks to build_onboarding_prompt
- Include index_status in onboarding JSON response
- Inject 'Semantic index: ready/not built' into gathered data"
```

---

### Task 4: Add new tests for index status

**Files:**
- Modify: `src/prompts/mod.rs` (tests module)

**Step 1: Write test for index-ready rendering**

```rust
#[test]
fn build_onboarding_shows_index_ready() {
    let result = build_onboarding_prompt(
        &["rust".into()], &[], &[], &[], &[], &[],
        true, 42, 350,
    );
    assert!(result.contains("Semantic index:** ready (42 files, 350 chunks)"));
}
```

**Step 2: Write test for index-not-ready rendering**

```rust
#[test]
fn build_onboarding_shows_index_not_built() {
    let result = build_onboarding_prompt(
        &["rust".into()], &[], &[], &[], &[], &[],
        false, 0, 0,
    );
    assert!(result.contains("Semantic index:** not built"));
}
```

**Step 3: Run tests**

Run: `cargo test -p codescout --lib prompts 2>&1 | tail -20`
Expected: All pass including both new tests.

**Step 4: Commit**

```bash
git add src/prompts/mod.rs
git commit -m "test: verify index status rendering in onboarding prompt"
```

---

### Task 5: Add Phase 0 to onboarding prompt

**Files:**
- Modify: `src/prompts/onboarding_prompt.md` (insert between Iron Law and Phase 1, ~line 26)

**Step 1: Insert Phase 0 section**

After the `---` separator on line 26, before `## Phase 1: Explore the Code`, insert:

```markdown
## Phase 0: Semantic Index Check

Check the **Semantic index** line in the Gathered Project Data below.

### If the index is READY:

Announce to the user:

> "Semantic index is ready ({files} files, {chunks} chunks). I'll use
> `semantic_search` for concept-level exploration in Phase 1."

Proceed to Phase 1.

### If the index is NOT BUILT:

Semantic search is **strongly recommended** for thorough onboarding. Present
this to the user:

> **Semantic search is not set up yet.**
>
> The embedding index powers concept-level code exploration (`semantic_search`),
> which finds code by meaning — not just by name or text pattern. Without it,
> onboarding relies on symbol tools and regex search, which work but may miss
> non-obvious connections.
>
> **Options:**
> 1. **Build now** — I'll call `index_project` and wait for it to finish.
>    Requires an embedding backend (Ollama is the default — see
>    `docs/manual/src/configuration/embedding-backends.md` for setup).
>    Takes 1–5 minutes depending on codebase size.
> 2. **Build from CLI** — Run `codescout index --project .` in another
>    terminal, then restart onboarding with `onboarding(force: true)`.
> 3. **Skip** — Proceed without semantic search. Exploration will use
>    `search_pattern` (regex) instead of `semantic_search`. You can always
>    build the index later.

Wait for the user's choice before proceeding.

- **Option 1:** Call `index_project({})`. Poll `index_status({})` every 15
  seconds until the response shows completion or failure. If it fails, inform
  the user of the error and fall back to option 3.
- **Option 2:** Stop and wait for the user to return.
- **Option 3:** Proceed to Phase 1. Step 6 will use `search_pattern` instead
  of `semantic_search`.

---
```

**Step 2: Verify prompt loads correctly**

Run: `cargo test -p codescout --lib prompts::tests::onboarding_prompt_contains_key_sections 2>&1 | tail -10`
Expected: PASS (existing test still finds all key sections).

---

### Task 6: Add Phase 0 test assertion

**Files:**
- Modify: `src/prompts/mod.rs` (test `onboarding_prompt_contains_key_sections`)

**Step 1: Add Phase 0 assertion**

Add to the existing `onboarding_prompt_contains_key_sections` test:

```rust
assert!(ONBOARDING_PROMPT.contains("## Phase 0: Semantic Index Check"));
```

**Step 2: Run all tests**

Run: `cargo test -p codescout --lib prompts 2>&1 | tail -20`
Expected: All pass.

**Step 3: Run full suite + clippy**

Run: `cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -5`
Run: `cargo test 2>&1 | grep "test result"` — all suites pass.

**Step 4: Commit**

```bash
git add src/prompts/onboarding_prompt.md src/prompts/mod.rs
git commit -m "feat: add Phase 0 semantic index check to onboarding prompt

Users are now asked whether to build the semantic index before
exploration begins. Three options: build now, build from CLI, or skip.
The prompt adapts Step 6 tool choice based on index availability."
```

---

## Verification Checklist

After all tasks:

- [ ] `cargo fmt --check` — clean
- [ ] `cargo clippy -- -D warnings` — clean
- [ ] `cargo test` — all pass (1037+ unit, 22 integration)
- [ ] `onboarding(force: true)` on a project WITH index → Phase 0 says "ready"
- [ ] `onboarding(force: true)` on a project WITHOUT index → Phase 0 presents options
- [ ] JSON response includes `"index_status": {"ready": ..., "files": ..., "chunks": ...}`
