# File Operations

These tools cover reading, listing, searching, and locating files. They are read-only — no file is modified. For writing and editing, see [Editing](./editing.md).

All paths are relative to the active project root unless you supply an absolute path. Reads are subject to the project's security deny-list (e.g. SSH keys and credential files are blocked by default).

> **See also:** [Output Buffers](../concepts/output-buffers.md) — how large
> file reads are stored as `@file_id` refs rather than dumped into context.

---

## `read_file`

**Purpose:** Read the contents of a file, optionally restricted to a line range.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path relative to project root |
| `start_line` | integer | no | — | First line to return (1-indexed) |
| `end_line` | integer | no | — | Last line to return (1-indexed, inclusive) |

**Example — read an entire file:**

```json
{
  "path": "src/main.rs"
}
```

**Output:**

```json
{
  "content": "fn main() {\n    println!(\"Hello\");\n}\n",
  "total_lines": 3
}
```

**Example — read a specific range:**

```json
{
  "path": "src/main.rs",
  "start_line": 10,
  "end_line": 25
}
```

When you supply both `start_line` and `end_line`, the tool returns exactly those lines with no overflow cap applied. When neither is supplied and the file exceeds 200 lines, only the first 200 lines are returned and an `overflow` field tells you how to retrieve the rest:

```json
{
  "content": "... first 200 lines ...",
  "total_lines": 850,
  "overflow": {
    "shown": 200,
    "total": 850,
    "hint": "File has 850 lines. Use start_line/end_line to read specific ranges"
  }
}
```

**Tips:**

- Use `list_symbols` or `find_symbol` to locate the line range of a function before calling `read_file` — this lets you fetch exactly what you need without reading the whole file.
- For large files, prefer reading in chunks with explicit `start_line`/`end_line` over reading the whole file.
- If you want to search for a pattern rather than read, use `search_pattern` instead.

---

## `list_dir`

**Purpose:** List files and directories under a path. Pass `recursive=true` for a full tree.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | Directory path relative to project root |
| `recursive` | boolean | no | `false` | Descend into subdirectories |
| `detail_level` | string | no | compact | `"full"` to show all entries without the exploring-mode cap |
| `offset` | integer | no | `0` | Skip this many entries (focused-mode pagination) |
| `limit` | integer | no | `50` | Max entries per page in focused mode |

**Example — shallow listing:**

```json
{
  "path": "src"
}
```

**Output:**

```json
{
  "entries": [
    "/home/user/project/src/main.rs",
    "/home/user/project/src/lib.rs",
    "/home/user/project/src/tools/"
  ]
}
```

Directories are suffixed with `/`. In exploring mode the output is capped at 200 entries; if the directory has more, an `overflow` field appears with guidance.

**Example — full recursive tree:**

```json
{
  "path": "src",
  "recursive": true
}
```

Hidden files and paths matched by `.gitignore` are excluded automatically.

**Tips:**

- Start with a shallow listing to understand the top-level structure, then drill into subdirectories of interest.
- Use `recursive=true` only when you need the full tree. On large repositories a recursive walk can produce many entries; narrow it with a more specific `path` if you hit the overflow cap.
- To find files by name pattern use `find_file` instead — it supports glob patterns and is faster for targeted searches.

---

## `search_pattern`

**Purpose:** Search the codebase for a regex pattern. Returns matching lines with file path and line number.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `pattern` | string | yes | — | Regular expression to search for |
| `path` | string | no | project root | Directory to restrict the search to |
| `max_results` | integer | no | `50` | Maximum number of matching lines to return |

**Example:**

```json
{
  "pattern": "fn\\s+validate_\\w+",
  "path": "src",
  "max_results": 20
}
```

**Output:**

```json
{
  "matches": [
    {
      "file": "/home/user/project/src/util/path_security.rs",
      "line": 14,
      "content": "pub fn validate_read_path("
    },
    {
      "file": "/home/user/project/src/util/path_security.rs",
      "line": 38,
      "content": "pub fn validate_write_path("
    }
  ],
  "total": 2
}
```

The search walks the directory tree using the same `.gitignore`-aware walker as `list_dir`. Binary files that cannot be decoded as UTF-8 are silently skipped. The regex engine enforces size limits to prevent pathological patterns from hanging.

**Tips:**

- Use `path` to narrow the search when you already know which part of the codebase is relevant — this is significantly faster on large repos.
- Increase `max_results` if you expect many matches and need to see them all.
- When you know a symbol name, `find_symbol` is more precise than a regex search because it uses the LSP index. Use `search_pattern` when you are looking for text patterns, string literals, comments, or constructs that the LSP does not model as symbols.
- To find files by name (not content), use `find_file`.

---

## `find_file`

**Purpose:** Find files matching a glob pattern. Respects `.gitignore`.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `pattern` | string | yes | — | Glob pattern (e.g. `**/*.rs`, `src/**/mod.rs`) |
| `path` | string | no | project root | Directory to search within |
| `max_results` | integer | no | `100` | Maximum number of file paths to return |

**Example:**

```json
{
  "pattern": "**/*.toml",
  "path": "."
}
```

**Output:**

```json
{
  "files": [
    "/home/user/project/Cargo.toml",
    "/home/user/project/.code-explorer/project.toml"
  ],
  "total": 2
}
```

**Example — find all test files in a subdirectory:**

```json
{
  "pattern": "**/test_*.py",
  "path": "tests"
}
```

The glob is matched against the path relative to the search directory, so `**/*.rs` will match files at any depth. The walker respects `.gitignore`, so build artifacts, vendored dependencies, and other ignored paths are excluded.

**Tips:**

- Prefer `find_file` over `list_dir` when you are looking for files by name — the glob match is more expressive than scanning a directory tree manually.
- Use `search_pattern` when you need to find files by their contents rather than their names.
- The `**` wildcard matches across directory boundaries. Use it for language-wide searches like `**/*.rs` or to locate files with a specific name anywhere in the tree: `**/Makefile`.
