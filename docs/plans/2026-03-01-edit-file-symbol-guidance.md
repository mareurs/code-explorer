# edit_file Symbol Guidance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Block multi-line `edit_file` calls on source files and redirect agents to the right symbol-aware tool, while updating prompts with an anti-pattern table and private memory sync.

**Architecture:** `is_source_path` utility in `path_security.rs` (reuses existing `SOURCE_EXTENSIONS`). `infer_edit_hint` pure fn in `file.rs` for testable hint selection. Check fires in `EditFile::call()` before the write. Two prompt files updated in-place.

**Tech Stack:** Rust, `regex` crate (already imported), `serde_json`

---

### Task 1: Add `is_source_path` utility

**Files:**
- Modify: `src/util/path_security.rs` (after line 486, end of `check_source_file_access`)

**Step 1: Write the failing test**

Add inside the `#[cfg(test)]` module near the other `source_file_access_*` tests:

```rust
#[test]
fn is_source_path_recognizes_all_supported_extensions() {
    assert!(is_source_path("src/main.rs"));
    assert!(is_source_path("lib.py"));
    assert!(is_source_path("index.ts"));
    assert!(is_source_path("main.go"));
    assert!(is_source_path("App.java"));
    assert!(is_source_path("Main.kt"));
    assert!(is_source_path("server.js"));
    assert!(!is_source_path("README.md"));
    assert!(!is_source_path("Cargo.toml"));
    assert!(!is_source_path("config.json"));
}
```

**Step 2: Run test to verify it fails**

```
cargo test is_source_path_recognizes 2>&1 | grep -E "FAILED|error\[|not found"
```

Expected: compile error — `is_source_path` not defined.

**Step 3: Implement**

Add after `check_source_file_access` (after line 486):

```rust
/// Returns true if the path refers to a source code file (by extension).
/// Used to gate `edit_file` multi-line blocks.
pub fn is_source_path(path: &str) -> bool {
    Regex::new(SOURCE_EXTENSIONS)
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}
```

**Step 4: Run test to verify it passes**

```
cargo test is_source_path_recognizes 2>&1 | grep -E "PASSED|ok|FAILED"
```

Expected: `test ... ok`

**Step 5: Commit**

```
git add src/util/path_security.rs
git commit -m "feat(security): add is_source_path utility for edit_file guard"
```

---

### Task 2: Add `infer_edit_hint` pure function

**Files:**
- Modify: `src/tools/file.rs` (add before `pub struct EditFile;` at line 597)

**Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module (starts at line 736):

```rust
#[test]
fn infer_edit_hint_remove_when_new_string_empty() {
    let hint = infer_edit_hint("fn foo() {\n    bar();\n}", "");
    assert!(hint.contains("remove_symbol"), "got: {hint}");
}

#[test]
fn infer_edit_hint_replace_symbol_for_rust_fn() {
    let hint = infer_edit_hint("fn foo() {\n    old();\n}", "fn foo() {\n    new();\n}");
    assert!(hint.contains("replace_symbol"), "got: {hint}");
}

#[test]
fn infer_edit_hint_replace_symbol_for_python_def() {
    let hint = infer_edit_hint("def process(x):\n    return x", "def process(x):\n    return x * 2");
    assert!(hint.contains("replace_symbol"), "got: {hint}");
}

#[test]
fn infer_edit_hint_replace_symbol_for_class() {
    let hint = infer_edit_hint("class Foo {\n    x: i32\n}", "class Foo {\n    y: i32\n}");
    assert!(hint.contains("replace_symbol"), "got: {hint}");
}

#[test]
fn infer_edit_hint_insert_code_when_new_is_longer() {
    let hint = infer_edit_hint("placeholder", "fn extra() {\n    todo!();\n}\nplaceholder");
    assert!(hint.contains("insert_code"), "got: {hint}");
}

#[test]
fn infer_edit_hint_fallback_lists_all_tools() {
    let hint = infer_edit_hint("old line\nother line", "new line\nother line");
    assert!(
        hint.contains("replace_symbol") && hint.contains("insert_code") && hint.contains("remove_symbol"),
        "got: {hint}"
    );
}
```

**Step 2: Run tests to verify they fail**

```
cargo test infer_edit_hint 2>&1 | grep -E "FAILED|error\[|not found"
```

Expected: compile error — `infer_edit_hint` not defined.

**Step 3: Implement**

Add before `pub struct EditFile;` (line 597):

```rust
/// Infers which symbol-aware tool to suggest when `edit_file` is blocked.
/// `old_string` and `new_string` are the raw parameters from the LLM call.
fn infer_edit_hint(old_string: &str, new_string: &str) -> &'static str {
    // Deletion: new_string is empty
    if new_string.is_empty() {
        return "remove_symbol(name_path, path) — deletes the symbol and its doc comments/attributes";
    }

    // Structural replacement: old_string looks like a named definition
    let def_keywords = ["fn ", "def ", "func ", "fun ", "function ", "async fn ",
                        "async def ", "async function ", "class ", "struct ",
                        "impl ", "trait ", "interface ", "enum ", "type "];
    if def_keywords.iter().any(|kw| old_string.contains(kw)) {
        return "replace_symbol(name_path, path, new_body) — replaces the symbol body via LSP";
    }

    // Insertion: new content is substantially larger
    if new_string.len() > old_string.len() {
        return "insert_code(name_path, path, code, position) — inserts before or after a named symbol";
    }

    // Fallback
    "replace_symbol / insert_code / remove_symbol — use the tool that matches your intent:\n  \
     replace_symbol(name_path, path, new_body) — replace a symbol body\n  \
     insert_code(name_path, path, code, position) — insert before/after a symbol\n  \
     remove_symbol(name_path, path) — delete a symbol"
}
```

**Step 4: Run tests to verify they pass**

```
cargo test infer_edit_hint 2>&1 | grep -E "ok|FAILED"
```

Expected: all 6 tests `ok`.

**Step 5: Commit**

```
git add src/tools/file.rs
git commit -m "feat(tools/file): add infer_edit_hint for contextual symbol tool suggestions"
```

---

### Task 3: Wire the blocker into `EditFile::call()`

**Files:**
- Modify: `src/tools/file.rs` (inside `EditFile::call()`, around line 639–645)

**Step 1: Write the failing integration tests**

Add to the `#[cfg(test)]` module (after line 736). These need the `project_ctx()` helper already used by other tests in that module:

```rust
#[tokio::test]
async fn edit_file_blocks_multiline_on_rust_source() {
    let (dir, ctx) = project_ctx().await;
    let path = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "fn foo() {\n    old();\n}\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": "src/lib.rs",
                "old_string": "fn foo() {\n    old();\n}",
                "new_string": "fn foo() {\n    new();\n}"
            }),
            &ctx,
        )
        .await;

    let err = result.unwrap_err();
    let rec = err.downcast_ref::<crate::tools::RecoverableError>()
        .expect("should be RecoverableError");
    assert!(rec.message.contains("symbol-aware tool"), "got: {}", rec.message);
    assert!(rec.hint.contains("replace_symbol"), "got: {}", rec.hint);
}

#[tokio::test]
async fn edit_file_allows_singleline_on_rust_source() {
    let (dir, ctx) = project_ctx().await;
    let path = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "let x = 1;\n").unwrap();

    let result = EditFile
        .call(
            json!({"path": "src/lib.rs", "old_string": "x = 1", "new_string": "x = 2"}),
            &ctx,
        )
        .await;

    assert!(result.is_ok(), "single-line edits on source should pass: {:?}", result.err());
}

#[tokio::test]
async fn edit_file_allows_multiline_on_markdown() {
    let (dir, ctx) = project_ctx().await;
    let path = dir.path().join("README.md");
    std::fs::write(&path, "line one\nline two\n").unwrap();

    let result = EditFile
        .call(
            json!({"path": "README.md", "old_string": "line one\nline two", "new_string": "updated one\nupdated two"}),
            &ctx,
        )
        .await;

    assert!(result.is_ok(), "multi-line edits on non-source should pass: {:?}", result.err());
}

#[tokio::test]
async fn edit_file_blocks_multiline_python() {
    let (dir, ctx) = project_ctx().await;
    let path = dir.path().join("app.py");
    std::fs::write(&path, "def greet():\n    print('hello')\n").unwrap();

    let result = EditFile
        .call(
            json!({"path": "app.py", "old_string": "def greet():\n    print('hello')", "new_string": "def greet():\n    print('hi')"}),
            &ctx,
        )
        .await;

    let err = result.unwrap_err();
    let rec = err.downcast_ref::<crate::tools::RecoverableError>().expect("RecoverableError");
    assert!(rec.hint.contains("replace_symbol"), "got: {}", rec.hint);
}

#[tokio::test]
async fn edit_file_hint_suggests_remove_symbol_when_new_string_empty() {
    let (dir, ctx) = project_ctx().await;
    let path = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "fn foo() {\n    bar();\n}\n").unwrap();

    let result = EditFile
        .call(
            json!({"path": "src/lib.rs", "old_string": "fn foo() {\n    bar();\n}", "new_string": ""}),
            &ctx,
        )
        .await;

    let err = result.unwrap_err();
    let rec = err.downcast_ref::<crate::tools::RecoverableError>().expect("RecoverableError");
    assert!(rec.hint.contains("remove_symbol"), "got: {}", rec.hint);
}
```

**Step 2: Run tests to verify they fail**

```
cargo test "edit_file_blocks_multiline\|edit_file_allows_singleline\|edit_file_allows_multiline_on_markdown\|edit_file_hint_suggests_remove" 2>&1 | grep -E "FAILED|ok|error"
```

Expected: the 3 "blocks" tests FAIL (edit succeeds when it shouldn't), the 2 "allows" tests may PASS or FAIL depending on test setup.

**Step 3: Wire the check**

In `EditFile::call()` — add after the `old_string.is_empty()` guard (after line ~645, before `let root = ...`):

```rust
// Block multi-line edits on source files — symbol tools are more precise.
if old_string.contains('\n')
    && crate::util::path_security::is_source_path(path)
{
    let hint = infer_edit_hint(old_string, new_string);
    return Err(super::RecoverableError::with_hint(
        "edit_file cannot replace multi-line source code — use a symbol-aware tool instead",
        hint,
    )
    .into());
}
```

**Step 4: Run all edit_file tests**

```
cargo test edit_file 2>&1 | grep -E "ok|FAILED|error"
```

Expected: all pass. If any existing test breaks, it's using a multi-line old_string on a source extension — update that test to use a non-source path or single-line string.

**Step 5: Run the full test suite**

```
cargo test 2>&1 | tail -5
```

Expected: all tests pass.

**Step 6: Commit**

```
git add src/tools/file.rs
git commit -m "feat(tools/file): block multi-line edit_file on source files with symbol tool hint"
```

---

### Task 4: Update `server_instructions.md`

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Add anti-pattern table**

Find the "Edit code" section (after line 63). After the `create_file` bullet and before the blank line leading to `### Refactor`, insert:

```markdown
**Prefer symbol tools over `edit_file` for source code:**
| ❌ `edit_file` for… | ✅ Use instead |
|---|---|
| Replacing a function/method/struct body | `replace_symbol(name_path, path, new_body)` |
| Inserting code before or after a symbol | `insert_code(name_path, path, code, position)` |
| Deleting a function, struct, or impl | `remove_symbol(name_path, path)` |
| Renaming a symbol across the codebase | `rename_symbol(name_path, path, new_name)` |

`edit_file` is for non-structural changes only: imports, string literals, comments, config values.
Multi-line edits on source files are blocked — the tool will tell you which symbol tool to use.
```

**Step 2: Add private memory params to the Memory section**

Find the Memory section (around line 125). After the `delete_memory` bullet, add:

```markdown
- `write_memory(topic, content, private=true)` — store in project-local private store (not surfaced in system instructions; use for sensitive or session-specific notes)
- `list_memories(include_private=true)` — returns both shared and private memories
```

**Step 3: Verify the file looks correct**

```
cargo run -- start --project . 2>&1 | head -5
```

(Just verifies the server still starts — the prompt file is loaded at runtime.)

**Step 4: Commit**

```
git add src/prompts/server_instructions.md
git commit -m "docs(prompts): add edit_file anti-pattern table and private memory params"
```

---

### Task 5: Update `onboarding_prompt.md`

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`

**Step 1: Add private memory note to the Rules block**

The Rules block is at the top of the file (lines 3–9). Add as Rule 6 (before the closing of the rules section):

```markdown
6. **Private memories** — Use `write_memory(topic, content, private=true)` for project-local notes that should not appear in system instructions (e.g. personal debugging notes, temporary state). Standard `write_memory` creates shared memories visible to all agents.
```

**Step 2: Commit**

```
git add src/prompts/onboarding_prompt.md
git commit -m "docs(prompts): add private memory guidance to onboarding rules"
```

---

### Task 6: Lint, format, final verification

**Step 1: Format**

```
cargo fmt
```

**Step 2: Lint**

```
cargo clippy -- -D warnings 2>&1 | grep -E "error|warning\[" | head -20
```

Fix any warnings before continuing.

**Step 3: Full test suite**

```
cargo test 2>&1 | tail -10
```

Expected: all tests pass, no regressions.

**Step 4: Final commit (if fmt/clippy touched files)**

```
git add -p
git commit -m "chore: fmt and clippy cleanup"
```

---

## Completion Checklist

- [ ] `is_source_path` in `path_security.rs` — tested for all supported extensions
- [ ] `infer_edit_hint` in `file.rs` — 6 unit tests covering all branches
- [ ] `EditFile::call()` — blocks multi-line source edits, routes to inferred hint
- [ ] `server_instructions.md` — anti-pattern table + private memory params added
- [ ] `onboarding_prompt.md` — Rule 6 for private memories added
- [ ] `cargo fmt` clean
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo test` — all tests pass
