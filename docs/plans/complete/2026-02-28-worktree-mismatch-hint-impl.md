# Worktree Mismatch Hint — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an advisory `worktree_hint` field to all 5 write-tool responses when git linked worktrees exist, so agents know when they may have written to the wrong project.

**Architecture:** New `worktree_hint(project_root)` helper in `path_security.rs`; each write tool calls it after resolving `root` and merges the `Option<String>` into its success JSON.

**Tech Stack:** Rust, pure filesystem I/O (no new dependencies), `serde_json::json!`, `tempfile` crate (already used in tests).

---

### Task 0: Commit the unrelated server.rs fix first

This change is already staged-in-progress and must be committed separately before touching anything else.

**Step 1: Stage and commit server.rs**

```bash
git add src/server.rs
git commit -m "fix(server): treat LSP RequestCancelled (-32800) as recoverable error

Kotlin-lsp and other IntelliJ-based servers cancel requests under concurrent
load. This must not produce isError:true, which would abort all sibling
parallel tool calls in Claude Code.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

Expected: clean commit, no other files touched.

**Step 2: Verify**

```bash
git status
```

Expected: `nothing to commit, working tree clean` (except the untracked docs/plans files).

---

### Task 1: Add `list_git_worktrees` + `worktree_hint` to path_security.rs (TDD)

**Files:**
- Modify: `src/util/path_security.rs` (add functions after line 239, add tests after line 662)

**Step 1: Write the two failing tests**

Add these two tests inside the existing `mod tests { ... }` block at the bottom of `src/util/path_security.rs`, after the last test (`library_paths_default_is_empty`, line 659–662):

```rust
    #[test]
    fn worktree_hint_none_when_no_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        // No .git/worktrees/ directory — common case, must be fast with None
        let hint = super::worktree_hint(dir.path());
        assert!(hint.is_none());
    }

    #[test]
    fn worktree_hint_some_when_worktrees_exist() {
        let dir = tempfile::tempdir().unwrap();
        // Simulate: project_root/.git/worktrees/feat/gitdir -> /wt_root/.git
        let wt_root = tempfile::tempdir().unwrap();
        let wt_entry = dir.path().join(".git").join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_entry).unwrap();
        let gitdir_content = format!("{}/.git\n", wt_root.path().display());
        std::fs::write(wt_entry.join("gitdir"), &gitdir_content).unwrap();

        let hint = super::worktree_hint(dir.path());
        assert!(hint.is_some(), "should return hint when worktrees exist");
        let msg = hint.unwrap();
        assert!(
            msg.contains(wt_root.path().to_str().unwrap()),
            "hint should contain the worktree path"
        );
        assert!(msg.contains("activate_project"), "hint should mention activate_project");
    }
```

**Step 2: Run tests to verify they fail**

```bash
cargo test worktree_hint -- --nocapture
```

Expected: FAIL — `worktree_hint` not found.

**Step 3: Implement `list_git_worktrees` and `worktree_hint`**

Add the following two public functions to `src/util/path_security.rs`, after the closing `}` of `validate_write_path` (after line 239):

```rust
/// List the root paths of all linked git worktrees for `project_root`.
///
/// Reads `.git/worktrees/<name>/gitdir` files, which contain absolute paths
/// like `/path/to/worktree/.git`. Returns the parent (the worktree root).
/// Returns an empty vec if no worktrees exist (the common case).
pub fn list_git_worktrees(project_root: &Path) -> Vec<PathBuf> {
    let worktrees_dir = project_root.join(".git").join("worktrees");
    if !worktrees_dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(&worktrees_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let gitdir_file = entry.path().join("gitdir");
        if let Ok(content) = std::fs::read_to_string(&gitdir_file) {
            let worktree_git = PathBuf::from(content.trim());
            if let Some(worktree_root) = worktree_git.parent() {
                paths.push(worktree_root.to_path_buf());
            }
        }
    }
    paths
}

/// Returns an advisory hint string if git linked worktrees exist under
/// `project_root`. Intended to be included in write-tool responses so an
/// agent knows it may have written to the main repo instead of a worktree.
///
/// Returns `None` if no worktrees exist (zero-overhead fast path).
pub fn worktree_hint(project_root: &Path) -> Option<String> {
    let worktrees = list_git_worktrees(project_root);
    if worktrees.is_empty() {
        return None;
    }
    let wt_list = worktrees
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Wrote to main project root. Git worktrees detected: [{}]. \
         If working in a worktree, call activate_project(\"{}\") first.",
        wt_list,
        worktrees[0].display()
    ))
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test worktree_hint -- --nocapture
```

Expected: 2 tests PASS.

**Step 5: Run full test suite to check no regressions**

```bash
cargo test
```

Expected: all tests pass.

**Step 6: Commit**

```bash
git add src/util/path_security.rs
git commit -m "feat(path_security): add worktree_hint helper

Detects git linked worktrees under the active project root. Returns an
advisory hint string for use in write-tool responses, so an agent knows
when it may have written to the main repo instead of a worktree.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 2: Inject hint into `edit_lines` and `create_file` (file.rs)

**Files:**
- Modify: `src/tools/file.rs`
  - `CreateFile::call` — lines 329–343
  - `EditLines::call` — lines 441–518

**Step 1: Update `CreateFile::call`**

The current `Ok(...)` at line 342 is:
```rust
        Ok(
            json!({ "status": "ok", "path": resolved.display().to_string(), "bytes": content.len() }),
        )
```

Replace the entire `call` body's last two lines (the `Ok(...)`) with:
```rust
        let mut resp = json!({
            "status": "ok",
            "path": resolved.display().to_string(),
            "bytes": content.len()
        });
        if let Some(hint) = crate::util::path_security::worktree_hint(&root) {
            resp["worktree_hint"] = json!(hint);
        }
        Ok(resp)
```

**Step 2: Update `EditLines::call`**

The current `Ok(...)` at lines 511–518 is:
```rust
        Ok(json!({
            "status": "ok",
            "path": resolved.display().to_string(),
            "lines_deleted": delete_count,
            "lines_inserted": lines_inserted,
            "new_total_lines": lines.len()
        }))
```

Replace with:
```rust
        let mut resp = json!({
            "status": "ok",
            "path": resolved.display().to_string(),
            "lines_deleted": delete_count,
            "lines_inserted": lines_inserted,
            "new_total_lines": lines.len()
        });
        if let Some(hint) = crate::util::path_security::worktree_hint(&root) {
            resp["worktree_hint"] = json!(hint);
        }
        Ok(resp)
```

**Step 3: Build to verify no compile errors**

```bash
cargo build 2>&1 | head -30
```

Expected: `Finished` with no errors.

**Step 4: Run tests**

```bash
cargo test
```

Expected: all pass.

**Step 5: Commit**

```bash
git add src/tools/file.rs
git commit -m "feat(tools/file): add worktree_hint to edit_lines and create_file responses

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 3: Inject hint into `replace_symbol` and `insert_code` (symbol.rs)

Both tools use `resolve_write_path(ctx, rel_path)` which returns `full_path`. We need `root` too — get it from `ctx.agent.require_project_root()`.

**Files:**
- Modify: `src/tools/symbol.rs`
  - `ReplaceSymbol::call` — lines 1070–1099
  - `InsertCode::call` — lines 1130–1161

**Step 1: Update `ReplaceSymbol::call`**

Current Ok at line 1099:
```rust
        Ok(json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) }))
```

Replace with (also add `path` field for transparency):
```rust
        let root = ctx.agent.require_project_root().await?;
        let mut resp = json!({
            "status": "ok",
            "path": full_path.display().to_string(),
            "replaced_lines": format!("{}-{}", start + 1, end)
        });
        if let Some(hint) = crate::util::path_security::worktree_hint(&root) {
            resp["worktree_hint"] = json!(hint);
        }
        Ok(resp)
```

**Step 2: Update `InsertCode::call`**

Current Ok at line 1161:
```rust
        Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1, "position": position }))
```

Replace with:
```rust
        let root = ctx.agent.require_project_root().await?;
        let mut resp = json!({
            "status": "ok",
            "path": full_path.display().to_string(),
            "inserted_at_line": insert_at + 1,
            "position": position
        });
        if let Some(hint) = crate::util::path_security::worktree_hint(&root) {
            resp["worktree_hint"] = json!(hint);
        }
        Ok(resp)
```

**Step 3: Build and test**

```bash
cargo build 2>&1 | head -30
cargo test
```

Expected: all pass.

**Step 4: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(tools/symbol): add path + worktree_hint to replace_symbol and insert_code

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 4: Inject hint into `rename_symbol` (symbol.rs)

`rename_symbol` builds a `mut result` JSON object at line 1445. We need to add the hint there.

**Files:**
- Modify: `src/tools/symbol.rs`, `RenameSymbol::call` — lines 1296–1460

**Step 1: Update the tail of `RenameSymbol::call`**

After the existing `if let Some(reason) = sweep_skip_reason { ... }` block (lines 1456–1458),
and before `Ok(result)` at line 1459, add:

```rust
        let rename_root2 = ctx.agent.require_project_root().await?;
        if let Some(hint) = crate::util::path_security::worktree_hint(&rename_root2) {
            result["worktree_hint"] = json!(hint);
        }
```

Note: `rename_root` (line 1320) is already declared in this function — use a different name
(`rename_root2`) to avoid a name conflict. Alternatively, reuse `rename_root` if it is still
in scope at this point — check the code and prefer reusing it if available.

**Step 2: Build and test**

```bash
cargo build 2>&1 | head -30
cargo test
```

Expected: all pass.

**Step 3: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(tools/symbol): add worktree_hint to rename_symbol response

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 5: Fix false "HARD-BLOCKED" claim in server_instructions.md

**Files:**
- Modify: `src/prompts/server_instructions.md`, lines 47–52

**Step 1: Update the Worktrees section**

Current text (lines 47–52):
```markdown
After `EnterWorktree`, ALWAYS call `activate_project("/absolute/worktree/path")` before
using any code-explorer tools. code-explorer tracks its own active project independently
of the shell's working directory — they are NOT automatically coupled.
MCP write tools (`edit_lines`, `replace_symbol`, `insert_code`, `create_file`) are
HARD-BLOCKED until `activate_project` is called.
```

Replace with:
```markdown
After `EnterWorktree`, ALWAYS call `activate_project("/absolute/worktree/path")` before
using any code-explorer tools. code-explorer tracks its own active project independently
of the shell's working directory — they are NOT automatically coupled.
If you forget, write tools will silently modify the main repo instead of the worktree —
they will include a `"worktree_hint"` field in their response to alert you. When you see
that field, call `activate_project` and redo the write.
```

**Step 2: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs(server_instructions): fix false HARD-BLOCKED claim for worktrees

Write tools now include an advisory worktree_hint field in responses
when git worktrees are detected. The previous claim was inaccurate.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 6: Final verification

**Step 1: Run full test suite with lint**

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

Expected: all clean.

**Step 2: Verify the git log looks right**

```bash
git log --oneline -8
```

Expected (most recent first):
```
<hash> docs(server_instructions): fix false HARD-BLOCKED claim
<hash> feat(tools/symbol): add worktree_hint to rename_symbol
<hash> feat(tools/symbol): add path + worktree_hint to replace_symbol and insert_code
<hash> feat(tools/file): add worktree_hint to edit_lines and create_file
<hash> feat(path_security): add worktree_hint helper
<hash> fix(server): treat LSP RequestCancelled (-32800) as recoverable error
```

**Step 3: Manual smoke test (optional but recommended)**

```bash
# Start MCP server (project = main repo, which has worktrees)
cargo run -- start --project . &
# Use MCP client to call edit_lines on a temp file
# Verify "worktree_hint" appears in response
```
