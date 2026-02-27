# E2E Test Fixture Projects — Design

**Date:** 2026-02-27
**Status:** Approved

## Problem

code-explorer's E2E tests run against external, mutable codebases (code-explorer itself for Rust, mirela/backend-kotlin for Kotlin). Assertions like `assert_contains "verifyPassword"` break when those projects refactor — not because code-explorer is broken, but because the test fixture moved. We need controlled, versioned test projects where we own every symbol and can write deterministic assertions.

## Solution

**Layered fixture architecture**: 5 language-specific projects that each implement a shared "core" feature matrix plus language-specific extensions. Assertions are data-driven from TOML manifests, not hardcoded in test functions.

### Languages

Rust, Python, TypeScript, Kotlin, Java — covers 3 LSP server families, JVM vs non-JVM, static vs dynamic typing, and different project structures (Cargo, pyproject, npm, Gradle).

### Architecture: Layered (Shared Core + Language Extensions)

- **Core layer**: Every project implements the same set of features (classes, methods, enums, generics, interfaces, modules, nested types, constants). A single `core-expectations.toml` defines cross-language assertions.
- **Extension layer**: Per-language files exercise unique features (Rust: blanket impls, lifetimes; Kotlin: sealed classes, companion objects; Python: decorators, protocols; etc.). Each has its own `<lang>-extensions.toml`.

## Core Feature Matrix

Every fixture project implements these features, mapped to language-appropriate idioms:

| Feature | What it tests | Rust | Python | TypeScript | Kotlin | Java |
|---|---|---|---|---|---|---|
| Struct/Class | `get_symbols_overview` | `struct Book` | `class Book` | `class Book` | `data class Book` | `record Book` |
| Methods | `find_symbol` name_path | `impl Book { fn title() }` | `def title(self)` | `title(): string` | `fun title()` | `String title()` |
| Interface/Trait | Abstract types | `trait Searchable` | `class Searchable(ABC)` | `interface Searchable` | `interface Searchable` | `interface Searchable` |
| Implementation | `find_referencing_symbols` | `impl Searchable for Book` | `class Book(Searchable)` | `implements` | `: Searchable` | `implements` |
| Enum | Variant discovery | `enum Genre` | `class Genre(Enum)` | `enum Genre` | `enum class Genre` | `enum Genre` |
| Generics | Parameterized types | `Catalog<T: Searchable>` | `Catalog[T]` | `Catalog<T>` | `Catalog<T : Searchable>` | `Catalog<T>` |
| Module/Package | Cross-file navigation | `mod models;` | `library/models/` | `import { Book }` | `package library.models` | `package library.models;` |
| Nested types | Deep name_path | `impl Catalog { struct Stats }` | inner class | nested class | companion object | static inner class |
| Free functions | Top-level symbols | `fn create_catalog()` | `def create_catalog()` | `export function` | `fun createCatalog()` | static util |
| Constants | Non-function symbols | `const MAX_RESULTS` | `MAX_RESULTS = 100` | `export const` | `const val` | `static final` |

## Language-Specific Extensions

### Rust
- Enum with struct/tuple variants (`SearchResult::Found { book, score }`)
- Trait with default methods
- Blanket impl (`impl<T: Display> Searchable for T`)
- Macro-generated code (`#[derive(Debug, Clone)]`)
- Associated types (`type Item = Book` in trait impl)
- Lifetime annotations
- `impl Trait` return types
- Module re-exports (`pub use`)

### Python
- `@property` decorator
- `@dataclass` with generated methods
- `__dunder__` methods
- Multiple inheritance with MRO
- Nested functions/closures
- Type aliases (`BookList = list[Book]`)
- `*args, **kwargs`
- Protocols (structural typing)

### TypeScript
- Union/intersection types
- Type guards (`x is Book`)
- Overloaded function signatures
- Mapped/conditional types
- Decorators
- Namespace merging (declaration merging)
- Index signatures
- `export default` vs named exports

### Kotlin
- Sealed class/interface hierarchy
- Companion object
- Extension functions (`fun Book.toJson()`)
- `suspend` functions
- Data class generated methods (copy, componentN)
- Delegated properties (`by lazy`)
- Object declarations (singletons)
- Inline/value classes
- Scope functions (apply, let, with)

### Java
- Sealed interfaces (Java 17+)
- Records with generated accessors
- Pattern matching in switch
- Annotations
- Anonymous classes
- Static nested vs inner classes
- Generics with wildcards (`? extends`)
- Default methods in interfaces

## Directory Layout

```
tests/
├── fixtures/
│   ├── core-expectations.toml
│   ├── rust-extensions.toml
│   ├── python-extensions.toml
│   ├── typescript-extensions.toml
│   ├── kotlin-extensions.toml
│   ├── java-extensions.toml
│   │
│   ├── rust-library/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── models/
│   │       │   ├── mod.rs
│   │       │   ├── book.rs          # core: struct, methods, constants
│   │       │   └── genre.rs         # core: enum
│   │       ├── traits/
│   │       │   ├── mod.rs
│   │       │   └── searchable.rs    # core: trait + ext: default method, blanket impl
│   │       ├── services/
│   │       │   ├── mod.rs
│   │       │   └── catalog.rs       # core: generics, nested type, free functions
│   │       └── extensions/
│   │           ├── mod.rs
│   │           ├── results.rs       # ext: enum variants, associated types
│   │           └── advanced.rs      # ext: lifetimes, impl Trait, re-exports, macros
│   │
│   ├── python-library/
│   │   ├── pyproject.toml
│   │   └── library/
│   │       ├── __init__.py
│   │       ├── models/
│   │       │   ├── __init__.py
│   │       │   ├── book.py          # core + ext: @dataclass, @property
│   │       │   └── genre.py         # core: Enum
│   │       ├── interfaces/
│   │       │   ├── __init__.py
│   │       │   └── searchable.py    # core: ABC + ext: Protocol
│   │       ├── services/
│   │       │   ├── __init__.py
│   │       │   └── catalog.py       # core: generics, nested class, free functions
│   │       └── extensions/
│   │           ├── __init__.py
│   │           └── advanced.py      # ext: multiple inheritance, nested funcs, type aliases
│   │
│   ├── typescript-library/
│   │   ├── package.json
│   │   ├── tsconfig.json
│   │   └── src/
│   │       ├── index.ts
│   │       ├── models/
│   │       │   ├── book.ts
│   │       │   └── genre.ts
│   │       ├── interfaces/
│   │       │   ├── searchable.ts
│   │       │   └── types.ts         # ext: union, mapped, conditional, type guards
│   │       ├── services/
│   │       │   └── catalog.ts
│   │       └── extensions/
│   │           └── advanced.ts      # ext: overloads, decorators, namespace merging
│   │
│   ├── kotlin-library/
│   │   ├── build.gradle.kts
│   │   ├── settings.gradle.kts
│   │   └── src/main/kotlin/library/
│   │       ├── models/
│   │       │   ├── Book.kt          # core + ext: companion object
│   │       │   └── Genre.kt
│   │       ├── interfaces/
│   │       │   └── Searchable.kt
│   │       ├── services/
│   │       │   └── Catalog.kt       # core + ext: suspend, extension funs
│   │       └── extensions/
│   │           ├── Results.kt       # ext: sealed class, object declarations
│   │           └── Advanced.kt      # ext: delegated props, inline class
│   │
│   └── java-library/
│       ├── build.gradle
│       ├── settings.gradle
│       └── src/main/java/library/
│           ├── models/
│           │   ├── Book.java        # core: record
│           │   └── Genre.java
│           ├── interfaces/
│           │   ├── Searchable.java   # core + ext: default methods
│           │   └── package-info.java
│           ├── services/
│           │   └── Catalog.java
│           └── extensions/
│               ├── Results.java     # ext: sealed interface, pattern matching
│               └── Advanced.java    # ext: annotations, anonymous classes
│
├── e2e/
│   ├── mod.rs                       # test module root
│   ├── harness.rs                   # load TOML, start LSP, run assertions
│   ├── expectations.rs              # serde structs for TOML parsing
│   ├── test_core.rs                 # one #[tokio::test] per language
│   ├── test_rust_extensions.rs
│   ├── test_python_extensions.rs
│   ├── test_typescript_extensions.rs
│   ├── test_kotlin_extensions.rs
│   └── test_java_extensions.rs
```

## Expectations Format

### Core expectations (cross-language)

```toml
[class_with_methods]
description = "A class/struct with multiple methods is discoverable"
tool = "get_symbols_overview"

  [class_with_methods.rust]
  path = "src/models/book.rs"
  contains_symbols = ["Book", "title", "isbn", "is_available"]

  [class_with_methods.kotlin]
  path = "src/main/kotlin/library/models/Book.kt"
  contains_symbols = ["Book", "title", "isbn", "isAvailable"]
  # ... etc for each language
```

### Language extensions

```toml
# kotlin-extensions.toml

[sealed_class_hierarchy]
description = "Sealed class with all subclasses discoverable"
tool = "find_symbol"
file = "src/main/kotlin/library/extensions/Results.kt"
symbol = "SearchResult"
contains_symbols = ["Found", "NotFound", "Error"]

[companion_object]
description = "Companion object methods discoverable via name_path"
tool = "find_symbol"
file = "src/main/kotlin/library/models/Book.kt"
symbol = "Companion"
contains_symbols = ["create", "fromJson"]
```

### Assertion types

Each expectation maps to exactly one tool call:

| `tool` value | Tool called | Key assertions |
|---|---|---|
| `get_symbols_overview` | `get_symbols_overview(path)` | `contains_symbols`: symbol names present in output |
| `find_symbol` | `find_symbol(symbol, file, include_body)` | `contains_symbols` (children), `body_contains` (source substrings) |
| `find_referencing_symbols` | `find_referencing_symbols(symbol, file)` | `expected_refs_contain`: files/symbols that reference it |
| `list_functions` | `list_functions(path)` | `contains_functions`: function names in output |
| `search_for_pattern` | `search_for_pattern(pattern)` | `expected_files`: files that match |

## Test Harness Design

### LSP Lifecycle

- **One LSP server per language, started once**, cached via `OnceLock<HashMap<String, FixtureContext>>`
- JVM languages (Kotlin, Java) have 5-minute init timeouts — startup cost is amortized across all test cases
- All tests are **read-only** — no fixture mutation — so they safely share one LSP session

### Test Execution

```rust
async fn run_expectations(language: &str, toml_path: &str) {
    let ctx = fixture_context(language).await;
    let expectations = load_expectations(toml_path, language);
    // Run all, collect failures, report at end
}
```

One `#[tokio::test]` per language per TOML file:

```rust
#[tokio::test]
#[cfg(feature = "e2e-rust")]
async fn core_rust() {
    run_expectations("rust", "tests/fixtures/core-expectations.toml").await;
}
```

### CI Feature Flags

```toml
# Cargo.toml
[features]
e2e = ["e2e-rust", "e2e-python", "e2e-typescript", "e2e-kotlin", "e2e-java"]
e2e-rust = []       # needs: rust-analyzer
e2e-python = []     # needs: pyright
e2e-typescript = [] # needs: typescript-language-server
e2e-kotlin = []     # needs: kotlin-lsp
e2e-java = []       # needs: jdtls
```

### Failure Reporting

```
  PASS  class_with_methods
  PASS  find_method_body
  FAIL  enum_variants: find_symbol("Genre") in src/models/genre.rs
        returned symbols ["Genre", "Fiction", "NonFiction"]
        but expected ["Science", "History"] to also be present

1 of 3 expectations failed for kotlin:
  - enum_variants: missing symbols: ["Science", "History"]
```

## What We Don't Build

- **No fixture code generation** — hand-written fixtures are more readable
- **No snapshot testing** — couples tests to exact LSP output format
- **No mutation testing** — tests code-explorer, not the LSP
- **No write-tool E2E in shared fixtures** — write tools get separate temp-copy tests

## Scalability

Adding a new language: create fixture project + add TOML entries + one test function.
Adding a new feature: add code to fixtures + add TOML section. No harness changes.
Adding a new assertion type: add `Assertion` enum variant + match arm.

### Projected scale

| Phase | Languages | Core features | Extensions/lang | Total test cases |
|---|---|---|---|---|
| v1 | 5 | ~10 | ~6-8 | ~80-90 |
| v2 (+Go, C++) | 7 | ~12 | ~6-8 | ~130-140 |
| v3 (full 9) | 9 | ~15 | ~6-8 | ~180-200 |
