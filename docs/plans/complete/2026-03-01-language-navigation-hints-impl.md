# Language-Specific Navigation Hints — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Inject language-specific `find_symbol`/`list_symbols` navigation idioms into the system prompt draft generated during onboarding.

**Architecture:** A single `language_navigation_hints(lang) -> Option<&'static str>` function returns compact navigation patterns per language. `build_system_prompt_draft()` calls it for each detected language and appends a `## Language Navigation` section.

**Tech Stack:** Rust, no new dependencies.

**Design doc:** `docs/plans/2026-03-01-language-navigation-hints-design.md`

---

### Task 1: Add `language_navigation_hints` function with tests

**Files:**
- Modify: `src/tools/workflow.rs` — add function before `build_system_prompt_draft` (insert at line 137)
- Modify: `src/tools/workflow.rs` — add unit tests in `mod tests`

**Step 1: Write the failing tests**

Add these tests at the end of the `mod tests` block in `src/tools/workflow.rs` (before the closing `}`):

```rust
    #[test]
    fn language_hints_covers_main_languages() {
        for lang in &["rust", "python", "typescript", "javascript", "go", "java", "kotlin", "c", "cpp", "tsx", "jsx"] {
            assert!(
                language_navigation_hints(lang).is_some(),
                "expected hints for '{}'", lang
            );
        }
    }

    #[test]
    fn language_hints_returns_none_for_unknown() {
        assert!(language_navigation_hints("markdown").is_none());
        assert!(language_navigation_hints("bash").is_none());
        assert!(language_navigation_hints("unknown_lang").is_none());
    }
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p code-explorer language_hints -- --nocapture 2>&1 | head -20`
Expected: FAIL — `language_navigation_hints` not found

**Step 3: Write the implementation**

Insert this function at line 137 in `src/tools/workflow.rs` (just above `fn build_system_prompt_draft`):

```rust
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
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p code-explorer language_hints -- --nocapture`
Expected: 2 tests PASS

---

### Task 2: Integrate hints into `build_system_prompt_draft`

**Files:**
- Modify: `src/tools/workflow.rs:137-182` — the `build_system_prompt_draft` function (line numbers will shift by ~40 after Task 1)

**Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    #[test]
    fn system_prompt_draft_includes_language_hints() {
        let langs = vec!["rust".to_string(), "python".to_string()];
        let draft = build_system_prompt_draft(&langs, &[]);
        assert!(draft.contains("## Language Navigation"), "should have Language Navigation section");
        assert!(draft.contains("**rust:**"), "should have rust hints");
        assert!(draft.contains("**python:**"), "should have python hints");
        assert!(draft.contains("name_path"), "hints should mention name_path");
    }

    #[test]
    fn system_prompt_draft_omits_hints_for_unsupported_languages() {
        let langs = vec!["markdown".to_string()];
        let draft = build_system_prompt_draft(&langs, &[]);
        assert!(!draft.contains("## Language Navigation"), "should not have Language Navigation for markdown-only");
    }
```

**Step 2: Run tests to verify the first fails**

Run: `cargo test -p code-explorer system_prompt_draft -- --nocapture`
Expected: `system_prompt_draft_includes_language_hints` FAIL — no `## Language Navigation` in output

**Step 3: Modify `build_system_prompt_draft`**

In `build_system_prompt_draft`, insert this block **after** the "Search Tips" section (after the line `draft.push_str("- Use specific terms over generic ones...`)` and before the "Navigation Strategy" section:

```rust
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
```

**Step 4: Run all tests**

Run: `cargo test -p code-explorer -- --nocapture 2>&1 | tail -5`
Expected: All tests PASS (including the existing `onboarding_includes_system_prompt_draft_field`)

**Step 5: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- -D warnings`
Expected: Clean

**Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(onboarding): language-specific navigation hints in system prompt draft

Add language_navigation_hints() returning name_path patterns and kind
filters for rust, python, ts/js, go, java/kotlin, and c/cpp.
Integrated into build_system_prompt_draft() as a ## Language Navigation
section that appears when the project has supported languages."
```

---

### Task 3: Verify end-to-end with a manual smoke test

**Step 1: Run the server against this project**

Run: `cargo run -- start --project . 2>/dev/null &`

Call `onboarding(force=true)` and inspect the `system_prompt_draft` field in the response. It should contain:

```
## Language Navigation
**rust:**
- name_path: `StructName/method`, `impl Trait for Type/method`
...
```

**Step 2: Kill the server**

Run: `kill %1`

**Step 3: Verify full test suite**

Run: `cargo test`
Expected: All tests pass (533+ tests)
