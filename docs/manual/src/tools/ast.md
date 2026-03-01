# AST Analysis Tools

The two AST tools use [tree-sitter](https://tree-sitter.github.io/) to parse
source files and extract structural information. They work entirely offline: no
language server needs to be running, and no active project is required.

**Supported languages:** Rust, Python, TypeScript, Go.

> **See also:** [Tool Selection](../concepts/tool-selection.md) — when to use
> AST tools (`list_functions`, `list_docs`) vs LSP-backed symbol tools.

---

## AST Tools vs LSP Tools

Both AST tools and LSP symbol tools (`list_symbols`, `find_symbol`,
`find_references`) can tell you about the structure of a file. Choose
based on what you need:

| | AST tools | LSP tools |
|---|---|---|
| Language server required | No | Yes (server must start) |
| Startup latency | Instant | A few seconds first call |
| Languages | Rust, Python, TypeScript, Go | 9 languages |
| Output | Signatures only | Full symbol trees, types, references |
| References / rename | No | Yes |
| Works without active project | Yes | No |

Use AST tools when:
- You need a quick function list and do not want to wait for a language server
  to start.
- You are working in a CI or scripting context where LSP servers are not
  available.
- You want to extract documentation comments programmatically.

Use LSP tools when:
- You need full symbol trees with type information.
- You need to find all callers of a function (`find_references`).
- You need to perform a rename that propagates across the project.
- The file's language is not in the AST tool's supported set (Java, Kotlin,
  C/C++, C#, Ruby).

For most interactive coding tasks, start with `list_symbols` (LSP).
Fall back to `list_functions` if the language server is unavailable or slow to
start.

---

## `list_functions`

**Purpose:** List all function and method signatures in a file using
tree-sitter. Returns names, name paths, line ranges, and symbol kinds. Does not
require a language server.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path, absolute or relative to the project root |

**Example:**

```json
{
  "path": "src/tools/git.rs"
}
```

**Output:**

```json
{
  "file": "/home/user/myproject/src/tools/git.rs",
  "functions": [
    {
      "name": "name",
      "name_path": "impl Tool for GitBlame/name",
      "kind": "Method",
      "start_line": 12,
      "end_line": 14
    },
    {
      "name": "description",
      "name_path": "impl Tool for GitBlame/description",
      "kind": "Method",
      "start_line": 15,
      "end_line": 17
    },
    {
      "name": "call",
      "name_path": "impl Tool for GitBlame/call",
      "kind": "Method",
      "start_line": 32,
      "end_line": 74
    }
  ],
  "total": 3
}
```

Each entry has:
- `name` — the function or method name
- `name_path` — fully qualified path within the file (e.g.,
  `"MyStruct/my_method"` for a method, or just `"my_function"` for a top-level
  function)
- `kind` — `"Function"` or `"Method"`
- `start_line` / `end_line` — 1-indexed line range of the complete definition
  including the body

The tool recurses into nested scopes: methods inside `impl` blocks, functions
inside modules, and so on are all included.

**Tips:**

- `list_functions` is the fastest way to get a flat list of every callable in a
  file. It is useful as a first step before deciding which function to read in
  full with `list_symbols` and `find_symbol`.
- If the file's language is not supported (e.g., Java, Kotlin), the tool
  returns an error. Use `list_symbols` for those languages instead.
- Both absolute and relative paths are accepted. Relative paths are resolved
  against the project root if one is active, or against the current working
  directory otherwise.
- If the file does not exist, the tool returns an error immediately rather than
  silently returning an empty list.

---

## `list_docs`

**Purpose:** Extract all documentation comments from a file using tree-sitter.
Returns each doc comment alongside the name of the symbol it is attached to.
Supports Rust (`///`), Python (triple-quoted strings), TypeScript (JSDoc
`/** */`), and Go (`//` block comments preceding declarations).

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path, absolute or relative to the project root |

**Example:**

```json
{
  "path": "src/embed/index.rs"
}
```

**Output:**

```json
{
  "file": "/home/user/myproject/src/embed/index.rs",
  "docstrings": [
    {
      "symbol_name": "open_db",
      "content": "Open (or create) the SQLite embeddings database at the standard path\nwithin the project root. Creates the schema on first run.",
      "start_line": 14,
      "end_line": 16
    },
    {
      "symbol_name": "search",
      "content": "Search the index by cosine similarity. Returns up to `limit` results\nordered by descending score.",
      "start_line": 88,
      "end_line": 90
    }
  ],
  "total": 2
}
```

Each entry has:
- `symbol_name` — the name of the function, struct, or other declaration that
  the comment is attached to
- `content` — the full text of the doc comment, with comment markers stripped
  (the `///` prefix, `/**` delimiters, etc. are removed)
- `start_line` / `end_line` — 1-indexed line range of the comment itself in
  the source file

**Tips:**

- Use `list_docs` to quickly survey the documented API surface of a
  file without reading the full source. It is particularly useful for
  understanding a library or module you are unfamiliar with.
- In Python, triple-quoted strings that appear as the first statement of a
  function, class, or module are treated as docstrings. Module-level docstrings
  are associated with the module name.
- In TypeScript, only `/** */` JSDoc blocks are extracted. Regular `//`
  comments are not included.
- If a function has no doc comment, it does not appear in the output. An empty
  `docstrings` array means the file has no documentation comments.
- `symbol_name` is the bare name of the immediately following declaration. For
  more context — such as which struct a method belongs to — use
  `list_symbols` on the same file.
