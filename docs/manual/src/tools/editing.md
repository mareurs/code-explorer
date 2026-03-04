# Editing

codescout provides two categories of editing tools:

- **Text-level editing** — `edit_file` finds and replaces an exact string in a file.
- **Symbol-level editing** — `replace_symbol`, `insert_code`, and `rename_symbol` operate on named code symbols located via the LSP. These are the preferred tools for editing source code.

All write operations are restricted to the active project root. Attempts to write outside the project root, or to paths on the security deny-list, are rejected with an error.

> **See also:** [Git Worktrees](../concepts/worktrees.md) — the worktree write
> guard that protects against silent edits to the wrong repository tree.

---

## When to use which editing tool

| Situation | Recommended tool |
|-----------|-----------------|
| Rewrite a function or method body | `replace_symbol` |
| Add a new function next to an existing one | `insert_code` |
| Rename a symbol everywhere it is used | `rename_symbol` |
| Change a string, constant, or small code fragment | `edit_file` |
| Edit a config file, Markdown, or other non-code file | `edit_file` |
| Create a new file | `create_file` |

For source code, prefer the symbol tools. They address code by name rather than by position, which means your edit remains correct even if the file was modified since you last read it. Fall back to `edit_file` when you need to change something small that is not naturally symbol-scoped.

---

## `create_file`

**Purpose:** Create a new file, or completely overwrite an existing one, with the supplied content. Parent directories are created automatically.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | Destination path, relative to project root |
| `content` | string | yes | — | Full content to write |

**Example:**

```json
{
  "path": "src/util/helpers.rs",
  "content": "pub fn clamp(v: i32, lo: i32, hi: i32) -> i32 {\n    v.max(lo).min(hi)\n}\n"
}
```

**Output:**

```json
{
  "status": "ok",
  "path": "/home/user/project/src/util/helpers.rs",
  "bytes": 58
}
```

**Tips:**

- Use this tool when you are generating a file from scratch. If the file already exists and you only need to change part of it, use `edit_file` or a symbol tool instead — those tools are less likely to accidentally discard content you did not intend to touch.
- The path is resolved relative to the active project root, so `src/util/helpers.rs` and `./src/util/helpers.rs` both work. Absolute paths that fall inside the project root are also accepted.

---

## `edit_file`

**Purpose:** Find-and-replace editing. Locates `old_string` in the file and replaces it with `new_string`. Works on any file type. Alternatively, prepend or append text without a match string.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File to edit, relative to project root |
| `old_string` | string | yes* | — | Exact text to find, including whitespace and indentation. *Not required when using `insert`. |
| `new_string` | string | yes | — | Replacement text. Set to `""` to delete the match. |
| `replace_all` | boolean | no | `false` | Replace every occurrence instead of requiring a unique match. |
| `insert` | string | no | — | `"prepend"` or `"append"` — add text at the start or end of the file without a match string. |

**Example — change a constant value:**

```json
{
  "path": "src/config.rs",
  "old_string": "    pub const MAX_RETRIES: u32 = 3;",
  "new_string": "    pub const MAX_RETRIES: u32 = 5;"
}
```

**Example — add an import at the top of a file:**

```json
{
  "path": "src/lib.rs",
  "insert": "prepend",
  "new_string": "use std::collections::HashMap;\n"
}
```

**Example — delete a block of lines:**

```json
{
  "path": "src/config.rs",
  "old_string": "    // TODO: remove this\n    legacy_init();\n",
  "new_string": ""
}
```

**Example — replace all occurrences of a deprecated API:**

```json
{
  "path": "src/util.rs",
  "old_string": "old_function()",
  "new_string": "new_function()",
  "replace_all": true
}
```

**Output:**

```json
"ok"
```

If `old_string` is not found, or appears multiple times without `replace_all: true`, the tool returns a recoverable error with the line numbers of all matches.

**Multi-line edits on source files:** When `old_string` spans multiple lines in a `.rs`, `.py`, `.ts`, or similar file, the tool responds with a `pending_ack` handle and suggests a symbol tool instead. You can confirm the edit by re-running with `acknowledge_risk: true`, or by passing the returned handle as the `path`.

**Tips:**

- `old_string` must match exactly — include any leading whitespace and indentation.
- Use `search_pattern` first to verify the exact text if you are unsure what to match.
- If you get a multiple-matches error, expand `old_string` to include enough surrounding context to make it unique, or use `replace_all: true`.
- Use `insert: "prepend"` or `insert: "append"` to add content at the start or end of a file when there is no anchor string to match.
- For adding a completely new top-level definition adjacent to an existing one, `insert_code` is more convenient — it addresses the location by symbol name rather than requiring an exact text match.

---

## Symbol editing tools

The following three tools address code by symbol name and are backed by the Language Server Protocol. They work with any language that has an LSP server configured. See [Symbol Navigation](./symbol-navigation.md) for background on how symbols are identified.

All three require:

- `name_path` — the symbol's name path as returned by `find_symbol` or `list_symbols`, e.g. `"MyStruct/my_method"`.
- `path` — the file containing the symbol, relative to the project root.

### `replace_symbol`

**Purpose:** Replace the entire body of a named symbol — function, method, class, or any other LSP-visible construct — with new source code.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name_path` | string | yes | Symbol name path (e.g. `"parse_args"`, `"Config/from_file"`) |
| `relative_path` | string | yes | File containing the symbol |
| `new_body` | string | yes | Complete replacement source, including the signature and braces |

**Example:**

```json
{
  "name_path": "greet",
  "relative_path": "src/lib.rs",
  "new_body": "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {}!\", name)\n}"
}
```

**Output:**

```json
{
  "status": "ok",
  "replaced_lines": "10-14"
}
```

### `insert_code`

**Purpose:** Insert code immediately before or after a named symbol (e.g. add a new function above or below an existing one, or add an attribute).

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name_path` | string | yes | Symbol to insert relative to |
| `path` | string | yes | File containing the symbol |
| `code` | string | yes | Code to insert |
| `position` | string | no | `"before"` or `"after"` (default: `"after"`) |

**Example (insert after):**

```json
{
  "name_path": "serialize",
  "path": "src/codec.rs",
  "code": "\npub fn deserialize(bytes: &[u8]) -> Result<Self> {\n    todo!()\n}",
  "position": "after"
}
```

**Example (insert before):**

```json
{
  "name_path": "process_request",
  "path": "src/server.rs",
  "code": "#[tracing::instrument]\n",
  "position": "before"
}
```

**Output:**

```json
{
  "status": "ok",
  "inserted_at_line": 63
}
```

### `rename_symbol`

**Purpose:** Rename a symbol across the entire codebase using LSP. All references in all files are updated atomically.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name_path` | string | yes | Current symbol name path |
| `relative_path` | string | yes | File containing the symbol's definition |
| `new_name` | string | yes | New name for the symbol |

**Example:**

```json
{
  "name_path": "parse_config",
  "relative_path": "src/config.rs",
  "new_name": "load_config"
}
```

**Output:**

```json
{
  "status": "ok",
  "old_name": "parse_config",
  "new_name": "load_config",
  "files_changed": 3,
  "total_edits": 7
}
```

`files_changed` is the number of distinct files that were modified. `total_edits` is the total number of individual text substitutions applied across all files.

**Tips for symbol tools:**

- Use `find_symbol` or `list_symbols` first to obtain the correct `name_path`. Nested symbols use a slash-separated path: `"OuterStruct/inner_method"`.
- `rename_symbol` requires a running LSP server for the file's language. If the LSP is unavailable, the tool returns an error — fall back to `search_pattern` combined with `edit_file` in that case, but be aware it will not respect scoping.
- After a `replace_symbol` edit, the file content shifts. If you plan to make additional `edit_file` edits in the same file, use `search_pattern` to verify the exact text you want to match before editing.
