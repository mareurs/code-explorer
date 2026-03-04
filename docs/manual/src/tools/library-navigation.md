# Library Navigation

These two tools let you register, inspect, and semantically search third-party
library source code — directly from within your agent workflow, without leaving
the project.

All library access is **read-only**. Editing tools operate only on project code.

> **See also:** [Library Navigation](../concepts/library-navigation.md) — how
> auto-discovery works, the scope parameter, and when to navigate library source.

---

## Auto-discovery

The most common way libraries enter the registry is automatically: when an LSP
`goto_definition` request returns a path outside the project root (e.g. a Rust
crate in `~/.cargo/registry/`, a Python package in `.venv/`), codescout
walks the parent directories looking for a package manifest (`Cargo.toml`,
`package.json`, `pyproject.toml`, `go.mod`) and registers the library.

After auto-discovery, symbol navigation tools can follow references into library
code without any manual setup. Use the `scope` parameter to explicitly target
libraries in searches (see below).

---

## `scope` parameter

Once libraries are registered, pass the optional `scope` string to any symbol
or search tool:

| Value | What it searches |
|---|---|
| `"project"` (default) | Only project source code |
| `"lib:<name>"` | A specific registered library, e.g. `"lib:serde"` |
| `"libraries"` | All registered libraries |
| `"all"` | Project source + all libraries |

Tools that accept `scope`:
`find_symbol`, `list_symbols`, `find_references`, `semantic_search`

All results include a `"source"` field (`"project"` or `"lib:<name>"`) to
distinguish origin.

---

## `list_libraries`

**Purpose:** Show all registered libraries, their root paths, and whether a
semantic index has been built for each.

**Parameters:** None.

**Example:**

```json
{}
```

**Output:**

```json
{
  "libraries": [
    {
      "name": "serde",
      "root": "/home/user/.cargo/registry/src/index.crates.io-6f17d22bba15001f/serde-1.0.195/",
      "indexed": false
    },
    {
      "name": "tokio",
      "root": "/home/user/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tokio-1.35.1/",
      "indexed": true
    }
  ],
  "total": 2
}
```

**Tips:**

- Libraries with `"indexed": false` support symbol navigation (LSP + tree-sitter)
  but not `semantic_search`. Run `index_project` with the library's root path to add semantic search.
- The registry is stored in `.code-explorer/libraries.json`. You can inspect it
  directly if you need to edit or remove an entry.

---

## Indexing a Library for Semantic Search

> **Note:** The `index_library` tool was removed in the v1 tool restructure.
> Use `index_project` directly, passing the library's root path.

Once a library is registered (via `list_libraries` or auto-discovery), build its
semantic index by pointing `index_project` at its root:

```json
{
  "tool": "index_project",
  "arguments": { "path": "/home/user/.cargo/registry/src/.../serde-1.0.195/" }
}
```

After indexing, `semantic_search` with `scope: "lib:<name>"` searches within that library:

```json
{
  "tool": "semantic_search",
  "arguments": { "query": "channel with backpressure", "scope": "lib:tokio" }
}
```

**Tips:**

- Only index libraries you actively need to search semantically. LSP symbol
  navigation (`find_symbol`, `list_symbols`) works without indexing.
- Indexing a large library (e.g. `tokio`) may take a few minutes on the first
  run. The library path is shown in `list_libraries` output.
