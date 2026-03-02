# Language-Specific Navigation Hints in System Prompt Draft

**Date:** 2026-03-01
**Status:** Approved

## Problem

The `build_system_prompt_draft()` function generates a scaffold for `.code-explorer/system-prompt.md` that is language-agnostic. It says "This is a rust/python project" but gives no guidance on how `name_path`, `kind` filters, or `find_symbol` patterns work for that specific language.

Different languages structure code very differently:
- Rust: `impl Trait for Type/method`, free functions, mod trees
- Python: `ClassName/method`, top-level functions, decorators
- Go: `TypeName/MethodName` (receiver methods), interfaces
- Java/Kotlin: `ClassName/methodName`, annotations, inner classes

The LLM wastes tool calls guessing the right patterns when it could be told upfront.

## Solution

Add a `fn language_navigation_hints(lang: &str) -> Option<&'static str>` function in `src/tools/workflow.rs` that returns compact navigation idioms per language. Integrate it into `build_system_prompt_draft()` as a new `## Language Navigation` section.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Where to inject | `build_system_prompt_draft()` only | This becomes `.code-explorer/system-prompt.md`, loaded every session |
| Content scope | Navigation idioms only | name_path patterns, kind filters, concrete examples |
| Storage | Inline Rust match block | Simplest thing that works; compiled into binary, easy to test |

## Implementation

### New function

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

### Integration into build_system_prompt_draft

After the existing "Search Tips" section:

```rust
// Language-specific navigation hints
let hints: Vec<_> = languages.iter()
    .filter_map(|lang| {
        language_navigation_hints(lang).map(|h| (lang.as_str(), h))
    })
    .collect();
if !hints.is_empty() {
    draft.push_str("## Language Navigation\n");
    for (lang, hint) in &hints {
        draft.push_str(&format!("**{}:**\n{}\n\n", lang, hint));
    }
}
```

### Output example (Rust project)

```markdown
## Language Navigation
**rust:**
- name_path: `StructName/method`, `impl Trait for Type/method`
- find_symbol(kind="struct") for data types, kind="function" for free fns
- impl blocks: `find_symbol("impl MyStruct")` or list_symbols shows `impl Trait for Type`
- Example: `find_symbol("Server/handle_request")` finds a method on Server
```

## Languages Covered

| Language | Hint content |
|----------|-------------|
| rust | impl blocks, struct/function kinds, name_path with impl |
| python | class/method, decorator note, class kind filter |
| typescript/javascript/tsx/jsx | class/function, React component note |
| go | receiver methods, interface patterns |
| java/kotlin | class hierarchy, annotation note |
| c/cpp | struct vs class, header disambiguation |
| Others (ruby, php, etc.) | No hints — generic tool guidance suffices |

## Testing

1. **`system_prompt_draft_includes_language_hints`** — `build_system_prompt_draft(&["rust", "python"], &[])` → assert contains `## Language Navigation`, `**rust:**`, `**python:**`
2. **`system_prompt_draft_omits_hints_for_unknown_languages`** — `build_system_prompt_draft(&["markdown"], &[])` → assert does NOT contain `## Language Navigation`
3. **`language_navigation_hints_covers_main_languages`** — assert `language_navigation_hints("rust").is_some()` etc. for all 6 language groups

## Files Changed

- `src/tools/workflow.rs` — add `language_navigation_hints()`, modify `build_system_prompt_draft()`
