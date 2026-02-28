# Tool Selection

Choosing the right tool for a task depends on what you already know about the
code. code-explorer organizes its tools around three knowledge levels. Matching
your level of knowledge to the right tool category avoids wasted queries and
keeps context usage low.

## You Know the Name

You have a file path, function name, class name, or method name. Use the
structure-aware tools that navigate by name directly.

**`find_symbol(pattern)`** — locate a symbol by name substring across the
project or within a specific file. Fast because it goes through the language
server index.

```json
{
  "tool": "find_symbol",
  "arguments": { "pattern": "AuthService", "relative_path": "src/services/auth.rs" }
}
```

**`list_symbols(path)`** — list all symbols in a file, directory, or
glob pattern. Use this when you have a file and want to see what is in it
before deciding which symbol to read.

```json
{ "tool": "list_symbols", "arguments": { "path": "src/services/auth.rs" } }
```
```

**`list_functions(path)`** — list all function and method signatures in a file
using tree-sitter. No language server required; works immediately on any
supported file.

```json
{ "tool": "list_functions", "arguments": { "path": "src/services/auth.rs" } }
```

**`find_references(name_path, path)`** — find every location
that references a specific symbol. Use this when you know the symbol and want
to trace all its callers or usages.

```json
{
  "tool": "find_references",
  "arguments": { "name_path": "AuthService/verify_token", "path": "src/services/auth.rs" }
}
```
```

Once you have located the symbol you want, switch to Focused mode to read its
body:

```json
{
  "tool": "find_symbol",
  "arguments": {
    "pattern": "verify_token",
    "relative_path": "src/services/auth.rs",
    "include_body": true,
    "detail_level": "full"
  }
}
```

## You Know the Concept

You have domain knowledge but not a specific name. You know that "there is
error handling somewhere" or "authentication goes through a token check" but
you do not know the file or function name.

Start with semantic search, then drill down.

**`semantic_search(query)`** — finds code relevant to a natural language
description using embedding similarity. It crosses file boundaries and finds
conceptually related code even when the literal words do not match.

```json
{ "tool": "semantic_search", "arguments": { "query": "JWT token verification and expiry" } }
```

The response gives you scored chunks with file paths and line ranges. From
those results, you have names and locations — move to the name-based tools to
read the full symbols.

**Typical concept-first workflow:**

1. `semantic_search("how are database errors handled")` — get a list of
   relevant files and line ranges.
2. `list_symbols(found_file)` — see the symbol structure around those
   lines.
3. `find_symbol(name, include_body=true, detail_level="full")` — read the
   specific function body.

## You Know Nothing

You are exploring an unfamiliar codebase or an area you have not touched
before. Start with structure and orient yourself before looking at any code.

**Step 1 — see the directory structure:**

```json
{ "tool": "list_dir", "arguments": { "path": "src" } }
```

This gives you the top-level layout: which directories exist, rough file
counts. Do not use `recursive: true` yet — the compact view of the top level
is usually enough to identify where to look next.

**Step 2 — scan a promising file or directory:**

```json
{ "tool": "list_symbols", "arguments": { "path": "src/services/" } }
```

This shows all symbols across the directory in compact form. You get names,
kinds, and line numbers for every symbol in every file, without loading any
bodies.

**Step 3 — get a high-level picture with semantic search:**

```json
{ "tool": "semantic_search", "arguments": { "query": "request lifecycle and routing" } }
```

Semantic search fills in context that structure cannot: which files handle
which concerns, what patterns are used, where the main logic lives.

**Step 4 — drill into specifics:**

Once you have a target, use `find_symbol` in Focused mode to read actual code.

## Common Anti-Patterns

**Reading entire files with `read_file` instead of using `find_symbol`.**
`read_file` without explicit line ranges dumps the full file content into
context. If you know the function name, use `find_symbol(include_body=true)`
instead — you get the function body without the surrounding boilerplate.

**Using `search_pattern` (grep) when `semantic_search` would serve better.**
`search_pattern` matches literal text. It works well for finding exact
strings, imports, or call sites where you know the exact text. When you want
code that implements a concept ("retry logic", "cache invalidation"), semantic
search finds related code even when the words you think of do not appear in the
source.

**Switching to Focused mode before knowing what you want.**
Calling `list_symbols` with `detail_level: "full"` on a large directory
floods the context with every function body in every file. Use Exploring mode
to identify the target, then use Focused mode on that specific target.

**Using `find_references` without a specific symbol.**
This tool requires a fully-qualified symbol path (`TypeName/method_name` in the
file that defines it). It is not a search tool — it is a precision tool for
tracing usages of a known symbol.

## Quick Reference

| You know... | Start with |
|-------------|------------|
| File path | `list_symbols(file)` |
| Function/class name | `find_symbol(pattern)` |
| Who calls a function | `find_references(name_path, file)` |
| A concept or behaviour | `semantic_search(query)` |
| Nothing (unfamiliar area) | `list_dir` → `list_symbols` → `semantic_search` |
| Exact string or import | `search_pattern(regex)` |
