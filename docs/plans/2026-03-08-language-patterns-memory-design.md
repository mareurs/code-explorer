# Language Patterns Memory — Design

**Date:** 2026-03-08
**Branch:** `experiments`
**Research:** `docs/research/claude-language-patterns.md`

## Problem

codescout has per-language *navigation* hints (`language_navigation_hints()`) but zero
per-language *coding pattern* guidance. LLMs produce predictable anti-patterns per
language (gratuitous `.clone()` in Rust, `any` in TypeScript, mutable default args in
Python, etc.) that are well-documented in community CLAUDE.md files and research.

## Solution

A new `language-patterns` memory topic, written during onboarding, containing the top 5
anti-patterns and top 5 correct patterns per detected project language. Content is
hardcoded in Rust (deterministic, no LLM variability). The system prompt instructs the
agent to read it before writing, editing, or reviewing code.

## Components

### 1. `language_patterns()` function

**Location:** `src/tools/workflow.rs` (near `language_navigation_hints()`)

```rust
fn language_patterns(lang: &str) -> Option<&'static str>
```

- Returns curated content per language
- 7 languages: Rust, Python, TypeScript, JavaScript, Go, Java, Kotlin
- ~15 lines per language: 5 anti-patterns ("Don't X → Do Y") + 5 correct patterns
- Content sourced from `docs/research/claude-language-patterns.md`
- Returns `None` for unsupported languages (C/C++, C#, Ruby — no research data)

### 2. `build_language_patterns_memory()` function

**Location:** `src/tools/workflow.rs`

```rust
fn build_language_patterns_memory(languages: &[String]) -> Option<String>
```

- Takes detected project languages
- Collects `language_patterns()` for each, skipping `None`
- Concatenates with a markdown header and per-language sections
- Returns `None` if no languages have patterns (nothing to write)

### 3. Onboarding writes the memory

In the onboarding tool's `call()` method, after language detection:
- Call `build_language_patterns_memory(&languages)`
- If `Some(content)`, write via `memory(action="write", topic="language-patterns", content=...)`
- This is a deterministic write — no LLM involvement in content generation

### 4. Server instructions rule

**File:** `src/prompts/server_instructions.md` — Rules section

Add one line:
```
10. **Read `language-patterns` memory before writing or editing code.** `memory(action="read", topic="language-patterns")` contains per-language anti-patterns and correct patterns. Read it before code changes or code review.
```

### 5. Per-project system prompt reinforcement

**File:** `.codescout/system-prompt.md` (generated during onboarding)

The `build_system_prompt_draft()` function adds a line like:
```
## Language Patterns
This project uses Rust, Python, TypeScript. Read `memory(action="read", topic="language-patterns")` before writing, editing, or reviewing code.
```

## Content per language (summary)

Each language gets ~15 lines total:

| Language | Anti-patterns (Don't → Do) | Correct patterns |
|----------|---------------------------|-----------------|
| Rust | `.clone()` → borrow, `.unwrap()` → `?`, `Rc<RefCell>` → restructure, `String` params → `&str`, catch-all `_` → exhaustive match | `thiserror`/`anyhow`, iterator chains, `Vec::with_capacity`, derive common traits, `if let` |
| Python | mutable defaults → `None`, `typing.List` → `list[str]`, bare except → specific, `os.path` → `pathlib`, `Any` → proper types | Modern type hints, `uv`/`ruff`/`pyright`, `pyproject.toml`, f-strings, dataclasses/Pydantic |
| TypeScript | `any` → `unknown`/Zod, `as` casting → type guards, missing discriminated unions → sealed types, `!` assertion → narrowing, enums → `as const` | Strict tsconfig, explicit return types, Zod for external data, branded types, `interface` for shapes |
| JavaScript | missing Promise error handling → `.catch()`/try-catch, stale closures → exhaustive deps, event listener leaks → cleanup, `var` → `const`/`let`, `==` → `===` | Proper useEffect async, named exports, early returns, template literals, `jsconfig.json` with `checkJs` |
| Go | `ioutil` → `io`/`os`, pre-modern loops → `slices`/`min`/`max`, Java-style interfaces → consumer-side, goroutine leaks → `ctx.Done()`, `%v` → `%w` | Table-driven tests, `errgroup`, functional options, `slog`, no name stuttering |
| Java | `@Autowired` fields → constructor injection, `Optional.get()` → `orElseThrow`, `throws Exception` → specific, `Date`/`Calendar` → `java.time`, raw types → generics | Records, sealed classes + pattern matching, switch expressions, text blocks, `List.of()` |
| Kotlin | `!!` → `?.let`/`?:`, `GlobalScope` → lifecycle scopes, `runBlocking` → suspend, mutable data classes → `val`/`List`, `enum` → sealed class | `val` over `var`, structured concurrency, `Result<T>`, `Sequence` for large collections, `require`/`check` |

## What we're NOT doing

- No new tool — it's just a memory topic
- No LLM-generated content — deterministic Rust strings
- No LSP instructions or community snippets in the memory
- No C/C++, C#, Ruby patterns (not in the research) — silently skipped
- No per-file or per-directory scoping — one memory for the whole project

## Data flow

```
onboarding() call
  → detect_languages() finds [rust, python, typescript]
  → build_language_patterns_memory(&languages)
    → language_patterns("rust") + language_patterns("python") + language_patterns("typescript")
    → concatenate into formatted markdown
  → memory(action="write", topic="language-patterns", content=...)
  → build_system_prompt_draft() adds "Language Patterns" section
  → server_instructions.md has generic rule (already present)
```

## Testing

- Unit test: `language_patterns()` returns `Some` for all 7 supported languages
- Unit test: `build_language_patterns_memory()` concatenates correctly, returns `None` for empty/unsupported
- Unit test: `build_language_patterns_memory()` deduplicates (e.g., TypeScript + JavaScript share some patterns but are separate entries)
- Integration: verify onboarding writes the memory topic (mock memory store)
