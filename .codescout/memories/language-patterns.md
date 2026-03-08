# Language Patterns

Per-language anti-patterns and correct patterns for this project's languages.
Each section lists the top 5 mistakes LLMs make and the top 5 idiomatic patterns.

### Java

**Anti-patterns (Don't → Do):**
1. `@Autowired` field injection → constructor injection with `final` fields (Spring 4.3+ auto-infers)
2. `Optional.get()` without check → `orElseThrow(() -> new NotFoundException(id))`, Optional for return types only
3. `throws Exception` / bare catches → declare and catch specific exceptions, log with context
4. `Date`/`Calendar`/`SimpleDateFormat` → `java.time`: `LocalDate`, `ZonedDateTime`, `DateTimeFormatter`
5. Raw types `List items` → `List<String> items = new ArrayList<>()`

**Correct patterns:**
1. Records for data carriers (Java 16+): `public record UserDto(String name, String email) {}`
2. Sealed classes + pattern matching (Java 17+/21+) with switch expressions
3. Text blocks `"""` for multi-line strings (Java 15+)
4. Pattern matching instanceof (Java 16+): `if (obj instanceof String s) { s.length(); }`
5. Immutable collections: `List.of()`, `Map.of()`, `Set.of()`

---

### JavaScript

**Anti-patterns (Don't → Do):**
1. Missing Promise error handling → every `.then()` needs `.catch()`, every `async/await` needs try/catch
2. Stale closures in React hooks → ensure exhaustive dependency arrays in useEffect/useCallback/useMemo
3. Event listener / timer memory leaks → cleanup with `removeEventListener`, `clearInterval`, `AbortController`
4. `var` declarations → `const` by default, `let` only for reassignment
5. Loose equality `==` → always `===` and `!==`

**Correct patterns:**
1. Proper useEffect async: define async inside effect, call it, return cleanup with AbortController
2. `const` by default, destructuring at function boundaries
3. Named exports over default exports — aids tree-shaking and refactoring
4. Template literals over string concatenation
5. `jsconfig.json` with `checkJs: true` for type safety in JS projects

---

### Kotlin

**Anti-patterns (Don't → Do):**
1. `!!` (not-null assertion) overuse → `?.let`, `?:`, `?.` chaining, or redesign to eliminate nullability
2. `GlobalScope.launch`/`async` → lifecycle-bound scopes: `viewModelScope`, `lifecycleScope`, injected `CoroutineScope`
3. `runBlocking` in production code → only for `main()` and tests, use suspend functions
4. Mutable `var` in data classes → `val` + `List` (not `MutableList`), immutability by default
5. `enum` when sealed class is needed → `sealed class`/`sealed interface` for state with per-variant data

**Correct patterns:**
1. `val` over `var`, `List` over `MutableList` — expose read-only interfaces
2. Structured concurrency: `coroutineScope { launch { a() }; launch { b() } }`
3. Sealed class/interface for all state and result types
4. `Sequence` for large collections with chained operations
5. `require`/`check`/`error` for preconditions: `require(age >= 0) { "Age must be non-negative" }`

---

### Python

**Anti-patterns (Don't → Do):**
1. Mutable default arguments `def f(items=[])` → use `None` with `if items is None: items = []`
2. `typing.List`, `typing.Dict`, `typing.Optional` → built-in generics: `list[str]`, `str | None`
3. Bare/broad exception handling `except Exception: pass` → catch specific exceptions, log with context
4. `os.path.join()` → `pathlib.Path`: `Path(base) / "data" / "file.csv"`
5. `Any` type overuse → complete type annotations on all function signatures

**Correct patterns:**
1. Modern type hints (3.10+): `list[int]`, `dict[str, Any]`, `str | None`
2. `uv` for packages, `ruff` for linting/formatting, `pyright` for types, `pytest` for testing
3. `pyproject.toml` over `setup.py`/`requirements.txt`
4. `dataclasses` for internal data, Pydantic for validation, TypedDict for dict shapes
5. `is` comparison for singletons: `if x is None:` not `if x == None:`

---

### Rust

**Anti-patterns (Don't → Do):**
1. Gratuitous `.clone()` to silence borrow checker → borrow: `&str` over `&String`, `&[T]` over `&Vec<T>`
2. `.unwrap()` everywhere → `?` with `.context()` from anyhow, `.expect("invariant: ...")` only for proven invariants
3. `Rc<RefCell<T>>` / interior mutability overuse → restructure data flow and ownership
4. `String` params where `&str` suffices → `fn greet(name: &str)`, use `Cow<'_, str>` when ownership is conditional
5. Catch-all `_ => {}` in match → handle all variants explicitly, let compiler check exhaustiveness

**Correct patterns:**
1. `thiserror` for library errors, `anyhow` for application errors — propagate with `?`
2. Iterator chains over explicit loops — `.iter().map(f).collect()`, avoid unnecessary `.collect()`
3. `Vec::with_capacity()` when size is known
4. Derive common traits: `#[derive(Debug, Clone, PartialEq)]`, `#[derive(Default)]` when sensible
5. `if let`/`while let` for single-pattern matching instead of full match

---

### TypeScript

**Anti-patterns (Don't → Do):**
1. `any` type overuse → `unknown` when type is uncertain, Zod schemas for external data
2. Type assertion `as` abuse / `as unknown as T` → type guards, proper narrowing
3. Missing discriminated unions → model domain states with `'kind'`/`'type'` discriminant, `satisfies never` for exhaustiveness
4. Non-null assertion `!` abuse → handle null/undefined with narrowing, optional chaining, type guards
5. Enums → `as const` objects or string literal union types

**Correct patterns:**
1. Strict tsconfig: `strict: true`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`
2. Explicit return types on exported functions
3. Zod schema validation for external data — derive types with `z.infer<typeof Schema>`
4. Discriminated unions with exhaustiveness: `default: throw new Error(\`Unhandled: ${x satisfies never}\`)`
5. `interface` for object shapes, `type` for unions/intersections/mapped types
