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
crate in `~/.cargo/registry/`, a Python package in `.venv/`), code-explorer
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
`find_symbol`, `list_symbols`, `find_references`,
`list_functions`, `semantic_search`

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
  but not `semantic_search`. Run `index_library` to add semantic search.
- The registry is stored in `.code-explorer/libraries.json`. You can inspect it
  directly if you need to edit or remove an entry.

---

## `index_library`

**Purpose:** Build or incrementally update the semantic search index for a
registered library. After indexing, `semantic_search` with
`scope: "lib:<name>"` searches within that library.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name` | string | yes | — | Library name as shown in `list_libraries` |
| `force` | boolean | no | `false` | Force full reindex, ignoring cached file hashes |

**Example:**

```json
{
  "name": "serde"
}
```

**Output:**

```json
{
  "status": "ok",
  "library": "serde",
  "files_indexed": 42,
  "total_chunks": 380
}
```

**Tips:**

- Only index libraries you actively need to search semantically. LSP symbol
  navigation (`find_symbol`, `list_symbols`) works without indexing.
- Indexing a large library (e.g. `tokio`) may take a few minutes on the first
  run. Subsequent incremental updates are fast.
- Use `semantic_search` with `scope: "lib:<name>"` after indexing:

```json
{
  "query": "channel with backpressure",
  "scope": "lib:tokio"
}
```
