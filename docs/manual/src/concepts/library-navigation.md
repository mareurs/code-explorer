# Library Navigation

Library navigation lets you explore third-party dependency source code using the
same symbol tools you use for your own project — `find_symbol`, `list_symbols`,
`goto_definition`, `semantic_search` — without switching contexts or manually
locating package directories.

## Auto-Discovery

Libraries are discovered automatically. When you call `goto_definition` on a
symbol and the LSP resolves it to a path *outside the project root* (typically
inside a language package cache), codescout registers that path as a library
and names it by the package name inferred from the manifest it finds there.

The next time you call `list_libraries`, the dependency appears in the list.
No manual registration is required for the common case.

## The Scope Parameter

Once a library is registered, pass `scope` to any navigation or search tool to
target it:

| Value | Searches |
|---|---|
| `"project"` (default) | Only your project's source code |
| `"lib:<name>"` | A specific registered library (e.g. `"lib:tokio"`) |
| `"libraries"` | All registered libraries combined |
| `"all"` | Your project + all registered libraries |

```json
{
  "tool": "semantic_search",
  "arguments": { "query": "retry with backoff", "scope": "lib:reqwest" }
}
```

Results include a `"source"` field so you can tell project code from library
code at a glance.

## Building a Library Index

Semantic search over library code requires an embedding index, just like project
code. Build one with `index_project` pointed at the library's root path:

```json
{ "tool": "index_project", "arguments": { "path": "/path/to/tokio-1.35.1/" } }
```

This is a one-time cost per library. The index persists in
`.code-explorer/libraries/<name>/embeddings.db`.

## When to Use Library Navigation

- You're debugging an unfamiliar error from a dependency and want to read its
  source without leaving your session
- You want to understand how a library's internal types relate before writing
  integration code
- You're doing a security audit and want to trace a call chain into a dependency
- You want to find usage examples by searching the library's own tests with
  `semantic_search(scope: "lib:<name>")`

## Further Reading

- [Library Navigation Tools](../tools/library-navigation.md) — full reference for
  `list_libraries` and library indexing
- [Symbol Navigation Tools](../tools/symbol-navigation.md) — the tools that accept
  the `scope` parameter
- [Semantic Search Tools](../tools/semantic-search.md) — semantic search within
  library scope
