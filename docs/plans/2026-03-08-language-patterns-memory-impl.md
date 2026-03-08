# Language Patterns Memory — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a hardcoded `language-patterns` memory topic written during onboarding, containing top 5 anti-patterns and top 5 correct patterns per detected language, with system prompt instructions to read it before writing/reviewing code.

**Architecture:** A new `language_patterns(lang) -> Option<&'static str>` function in `src/tools/workflow.rs` returns curated content per language. A `build_language_patterns_memory(languages) -> Option<String>` function assembles the full memory. The onboarding tool's `call()` writes this memory. `server_instructions.md` and the system prompt draft both reference it.

**Tech Stack:** Rust, static strings, existing memory system (`MemoryStore::write`)

**Design doc:** `docs/plans/2026-03-08-language-patterns-memory-design.md`
**Research:** `docs/research/claude-language-patterns.md`

---

### Task 1: Add `language_patterns()` function

**Files:**
- Modify: `src/tools/workflow.rs` (insert after `language_navigation_hints` at line 170)

**Step 1: Write the failing test**

Add to the `tests` module in `src/tools/workflow.rs`:

```rust
#[test]
fn language_patterns_covers_all_supported_languages() {
    let supported = ["rust", "python", "typescript", "javascript", "go", "java", "kotlin"];
    for lang in &supported {
        assert!(
            language_patterns(lang).is_some(),
            "language_patterns() should return Some for {lang}"
        );
    }
}

#[test]
fn language_patterns_returns_none_for_unsupported() {
    assert!(language_patterns("haskell").is_none());
    assert!(language_patterns("ruby").is_none());
    assert!(language_patterns("c").is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test language_patterns_covers -- --nocapture`
Expected: FAIL — `language_patterns` function not found.

**Step 3: Write the implementation**

Insert after `language_navigation_hints()` (line 170) in `src/tools/workflow.rs`:

```rust
/// Returns curated anti-patterns and correct patterns for a language.
/// Content sourced from docs/research/claude-language-patterns.md.
fn language_patterns(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some(
            "### Rust\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. Gratuitous `.clone()` to silence borrow checker → borrow: `&str` over `&String`, `&[T]` over `&Vec<T>`\n\
             2. `.unwrap()` everywhere → `?` with `.context()` from anyhow, `.expect(\"invariant: ...\")` only for proven invariants\n\
             3. `Rc<RefCell<T>>` / interior mutability overuse → restructure data flow and ownership\n\
             4. `String` params where `&str` suffices → `fn greet(name: &str)`, use `Cow<'_, str>` when ownership is conditional\n\
             5. Catch-all `_ => {}` in match → handle all variants explicitly, let compiler check exhaustiveness\n\
             \n\
             **Correct patterns:**\n\
             1. `thiserror` for library errors, `anyhow` for application errors — propagate with `?`\n\
             2. Iterator chains over explicit loops — `.iter().map(f).collect()`, avoid unnecessary `.collect()`\n\
             3. `Vec::with_capacity()` when size is known\n\
             4. Derive common traits: `#[derive(Debug, Clone, PartialEq)]`, `#[derive(Default)]` when sensible\n\
             5. `if let`/`while let` for single-pattern matching instead of full match",
        ),
        "python" => Some(
            "### Python\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. Mutable default arguments `def f(items=[])` → use `None` with `if items is None: items = []`\n\
             2. `typing.List`, `typing.Dict`, `typing.Optional` → built-in generics: `list[str]`, `str | None`\n\
             3. Bare/broad exception handling `except Exception: pass` → catch specific exceptions, log with context\n\
             4. `os.path.join()` → `pathlib.Path`: `Path(base) / \"data\" / \"file.csv\"`\n\
             5. `Any` type overuse → complete type annotations on all function signatures\n\
             \n\
             **Correct patterns:**\n\
             1. Modern type hints (3.10+): `list[int]`, `dict[str, Any]`, `str | None`\n\
             2. `uv` for packages, `ruff` for linting/formatting, `pyright` for types, `pytest` for testing\n\
             3. `pyproject.toml` over `setup.py`/`requirements.txt`\n\
             4. `dataclasses` for internal data, Pydantic for validation, TypedDict for dict shapes\n\
             5. `is` comparison for singletons: `if x is None:` not `if x == None:`",
        ),
        "typescript" => Some(
            "### TypeScript\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. `any` type overuse → `unknown` when type is uncertain, Zod schemas for external data\n\
             2. Type assertion `as` abuse / `as unknown as T` → type guards, proper narrowing\n\
             3. Missing discriminated unions → model domain states with `'kind'`/`'type'` discriminant, `satisfies never` for exhaustiveness\n\
             4. Non-null assertion `!` abuse → handle null/undefined with narrowing, optional chaining, type guards\n\
             5. Enums → `as const` objects or string literal union types\n\
             \n\
             **Correct patterns:**\n\
             1. Strict tsconfig: `strict: true`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`\n\
             2. Explicit return types on exported functions\n\
             3. Zod schema validation for external data — derive types with `z.infer<typeof Schema>`\n\
             4. Discriminated unions with exhaustiveness: `default: throw new Error(\\`Unhandled: ${x satisfies never}\\`)`\n\
             5. `interface` for object shapes, `type` for unions/intersections/mapped types",
        ),
        "javascript" | "jsx" => Some(
            "### JavaScript\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. Missing Promise error handling → every `.then()` needs `.catch()`, every `async/await` needs try/catch\n\
             2. Stale closures in React hooks → ensure exhaustive dependency arrays in useEffect/useCallback/useMemo\n\
             3. Event listener / timer memory leaks → cleanup with `removeEventListener`, `clearInterval`, `AbortController`\n\
             4. `var` declarations → `const` by default, `let` only for reassignment\n\
             5. Loose equality `==` → always `===` and `!==`\n\
             \n\
             **Correct patterns:**\n\
             1. Proper useEffect async: define async inside effect, call it, return cleanup with AbortController\n\
             2. `const` by default, destructuring at function boundaries\n\
             3. Named exports over default exports — aids tree-shaking and refactoring\n\
             4. Template literals over string concatenation\n\
             5. `jsconfig.json` with `checkJs: true` for type safety in JS projects",
        ),
        "go" => Some(
            "### Go\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. `ioutil` package → `io.ReadAll`, `os.ReadFile`, `os.MkdirTemp` (deprecated since Go 1.16)\n\
             2. Pre-modern patterns → `slices.Contains()`, `min`/`max` builtins (1.21), `for range n` (1.22)\n\
             3. Java-style large interfaces at producer → accept interfaces at consumer, return structs, keep interfaces small (1-3 methods)\n\
             4. Error wrapping with `%v` → `fmt.Errorf(\"context: %w\", err)`, use `errors.Is`/`errors.As`\n\
             5. `context.Background()` deep in call chains → ctx as first param, pass through entire chain, never store in structs\n\
             \n\
             **Correct patterns:**\n\
             1. Table-driven tests with `t.Parallel()` and `t.Run()` subtests\n\
             2. `errgroup` for structured concurrency: `g, ctx := errgroup.WithContext(ctx)`\n\
             3. Functional options pattern: `WithPort(8080)`, `WithTimeout(30*time.Second)`\n\
             4. `slog` for structured logging (Go 1.21+), not `log.Println`\n\
             5. No name stuttering: `package kv; type Store` not `type KVStore`",
        ),
        "java" => Some(
            "### Java\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. `@Autowired` field injection → constructor injection with `final` fields (Spring 4.3+ auto-infers)\n\
             2. `Optional.get()` without check → `orElseThrow(() -> new NotFoundException(id))`, Optional for return types only\n\
             3. `throws Exception` / bare catches → declare and catch specific exceptions, log with context\n\
             4. `Date`/`Calendar`/`SimpleDateFormat` → `java.time`: `LocalDate`, `ZonedDateTime`, `DateTimeFormatter`\n\
             5. Raw types `List items` → `List<String> items = new ArrayList<>()`\n\
             \n\
             **Correct patterns:**\n\
             1. Records for data carriers (Java 16+): `public record UserDto(String name, String email) {}`\n\
             2. Sealed classes + pattern matching (Java 17+/21+) with switch expressions\n\
             3. Text blocks `\"\"\"` for multi-line strings (Java 15+)\n\
             4. Pattern matching instanceof (Java 16+): `if (obj instanceof String s) { s.length(); }`\n\
             5. Immutable collections: `List.of()`, `Map.of()`, `Set.of()`",
        ),
        "kotlin" => Some(
            "### Kotlin\n\
             \n\
             **Anti-patterns (Don't → Do):**\n\
             1. `!!` (not-null assertion) overuse → `?.let`, `?:`, `?.` chaining, or redesign to eliminate nullability\n\
             2. `GlobalScope.launch`/`async` → lifecycle-bound scopes: `viewModelScope`, `lifecycleScope`, injected `CoroutineScope`\n\
             3. `runBlocking` in production code → only for `main()` and tests, use suspend functions\n\
             4. Mutable `var` in data classes → `val` + `List` (not `MutableList`), immutability by default\n\
             5. `enum` when sealed class is needed → `sealed class`/`sealed interface` for state with per-variant data\n\
             \n\
             **Correct patterns:**\n\
             1. `val` over `var`, `List` over `MutableList` — expose read-only interfaces\n\
             2. Structured concurrency: `coroutineScope { launch { a() }; launch { b() } }`\n\
             3. Sealed class/interface for all state and result types\n\
             4. `Sequence` for large collections with chained operations\n\
             5. `require`/`check`/`error` for preconditions: `require(age >= 0) { \"Age must be non-negative\" }`",
        ),
        _ => None,
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test language_patterns -- --nocapture`
Expected: PASS — both tests green.

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: add language_patterns() with curated patterns for 7 languages"
```

---

### Task 2: Add `build_language_patterns_memory()` function

**Files:**
- Modify: `src/tools/workflow.rs` (insert after `language_patterns`)

**Step 1: Write the failing test**

Add to the `tests` module:

```rust
#[test]
fn build_language_patterns_memory_assembles_detected_languages() {
    let langs = vec!["rust".to_string(), "python".to_string()];
    let result = build_language_patterns_memory(&langs);
    assert!(result.is_some());
    let content = result.unwrap();
    assert!(content.contains("### Rust"));
    assert!(content.contains("### Python"));
    assert!(!content.contains("### Go"));
    assert!(content.starts_with("# Language Patterns"));
}

#[test]
fn build_language_patterns_memory_returns_none_for_unsupported_only() {
    let langs = vec!["haskell".to_string(), "ruby".to_string()];
    let result = build_language_patterns_memory(&langs);
    assert!(result.is_none());
}

#[test]
fn build_language_patterns_memory_returns_none_for_empty() {
    let result = build_language_patterns_memory(&[]);
    assert!(result.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test build_language_patterns_memory -- --nocapture`
Expected: FAIL — function not found.

**Step 3: Write the implementation**

Insert after `language_patterns()` in `src/tools/workflow.rs`:

```rust
/// Assembles a language-patterns memory from detected project languages.
/// Returns None if no detected languages have pattern data.
fn build_language_patterns_memory(languages: &[String]) -> Option<String> {
    let sections: Vec<&str> = languages
        .iter()
        .filter_map(|lang| language_patterns(lang))
        .collect();

    if sections.is_empty() {
        return None;
    }

    let mut content = String::from(
        "# Language Patterns\n\n\
         Per-language anti-patterns and correct patterns for this project's languages.\n\
         Each section lists the top 5 mistakes LLMs make and the top 5 idiomatic patterns.\n\n",
    );

    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            content.push_str("\n---\n\n");
        }
        content.push_str(section);
        content.push('\n');
    }

    Some(content)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test build_language_patterns_memory -- --nocapture`
Expected: PASS — all 3 tests green.

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: add build_language_patterns_memory() assembler"
```

---

### Task 3: Write the memory during onboarding

**Files:**
- Modify: `src/tools/workflow.rs` — `impl Tool for Onboarding / call()` method (around line 440, after the existing `memory.write("onboarding", ...)` call)

**Step 1: Write the failing test**

Add to the `tests` module:

```rust
#[tokio::test]
async fn onboarding_writes_language_patterns_memory() {
    let (_dir, ctx) = project_ctx().await;
    // project_ctx creates main.rs (rust) and lib.py (python)
    let _result = Onboarding.call(json!({}), &ctx).await.unwrap();

    // Verify the language-patterns memory was written
    let memory_content = ctx
        .agent
        .with_project(|p| p.memory.read("language-patterns"))
        .await
        .unwrap();
    assert!(memory_content.contains("### Rust"), "should contain Rust patterns");
    assert!(memory_content.contains("### Python"), "should contain Python patterns");
    assert!(memory_content.contains("Anti-patterns"), "should contain anti-patterns section");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test onboarding_writes_language_patterns -- --nocapture`
Expected: FAIL — memory topic "language-patterns" not found.

**Step 3: Write the implementation**

In the `call()` method of `impl Tool for Onboarding`, find the block that writes the onboarding memory (around line 440):

```rust
p.memory.write("onboarding", &summary)?;
```

Add immediately after that line, still inside the same `with_project` closure:

```rust
// Write language-patterns memory (deterministic, from hardcoded content)
if let Some(patterns) = build_language_patterns_memory(&lang_list) {
    p.memory.write("language-patterns", &patterns)?;
}
```

**Important:** `lang_list` is already defined above this closure (line ~430). It must be cloned or captured. Check whether the closure already captures `lang_list` by reference — it does (it's used in the `summary` format string). So this addition should compile as-is.

**Step 4: Run tests to verify they pass**

Run: `cargo test onboarding_writes_language_patterns -- --nocapture`
Expected: PASS

Also run the existing onboarding tests to ensure no regressions:

Run: `cargo test onboarding_ -- --nocapture`
Expected: All onboarding tests PASS.

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: write language-patterns memory during onboarding"
```

---

### Task 4: Add server instructions rule

**Files:**
- Modify: `src/prompts/server_instructions.md` — Rules section (line 276, after rule 9)

**Step 1: Add the rule**

Append after rule 9 in the `## Rules` section:

```markdown
10. **Read `language-patterns` memory before writing or editing code.** `memory(action="read", topic="language-patterns")` contains per-language anti-patterns and correct patterns. Consult it before code changes or code review.
```

**Step 2: Verify the prompt compiles**

Run: `cargo build`
Expected: Success (the prompt is included via `include_str!` — a syntax error in the markdown won't break compilation, but we verify nothing else broke).

**Step 3: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "feat: add language-patterns rule to server instructions"
```

---

### Task 5: Add system prompt draft reinforcement

**Files:**
- Modify: `src/tools/workflow.rs` — `build_system_prompt_draft()` function (around line 220, after the "Language Navigation" section)

**Step 1: Write the failing test**

Add to the `tests` module:

```rust
#[test]
fn system_prompt_draft_includes_language_patterns_hint() {
    let langs = vec!["rust".to_string(), "python".to_string()];
    let entries = vec!["src/main.rs".to_string()];
    let draft = build_system_prompt_draft(&langs, &entries, None);
    assert!(
        draft.contains("language-patterns"),
        "draft should reference language-patterns memory"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test system_prompt_draft_includes_language_patterns -- --nocapture`
Expected: FAIL — assertion fails, draft doesn't contain "language-patterns".

**Step 3: Write the implementation**

In `build_system_prompt_draft()`, after the "Language Navigation" block (around line 220, after the `for (lang, hint) in &hints` loop), add:

```rust
// Language patterns reference — only if at least one language has patterns
let has_patterns = languages.iter().any(|l| language_patterns(l).is_some());
if has_patterns {
    let pattern_langs: Vec<&str> = languages
        .iter()
        .filter(|l| language_patterns(l).is_some())
        .map(|s| s.as_str())
        .collect();
    draft.push_str("## Language Patterns\n");
    draft.push_str(&format!(
        "This project uses {}. Read `memory(action=\"read\", topic=\"language-patterns\")` before writing, editing, or reviewing code.\n\n",
        pattern_langs.join(", ")
    ));
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test system_prompt_draft_includes_language_patterns -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: add language-patterns reference to system prompt draft"
```

---

### Task 6: Final verification

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

**Step 3: Run fmt**

Run: `cargo fmt`
Expected: No changes (or apply formatting).

**Step 4: Build release**

Run: `cargo build --release`
Expected: Success.

**Step 5: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: fmt + clippy cleanup for language-patterns feature"
```

---

### Summary of all changes

| File | Change |
|------|--------|
| `src/tools/workflow.rs` | Add `language_patterns()`, `build_language_patterns_memory()`, write memory in `Onboarding::call()`, add language-patterns hint in `build_system_prompt_draft()` |
| `src/prompts/server_instructions.md` | Add rule 10: read language-patterns before code changes |
| `src/tools/workflow.rs` (tests) | 6 new tests covering patterns, assembler, onboarding write, and prompt draft |
