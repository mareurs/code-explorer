# Editing

code-explorer provides two categories of editing tools:

- **Text-level editing** — `edit_lines` operates on raw text and line positions.
- **Symbol-level editing** — `replace_symbol`, `insert_code`, and `rename_symbol` operate on named code symbols located via the LSP. These are the preferred tools for editing source code.

All write operations are restricted to the active project root. Attempts to write outside the project root, or to paths on the security deny-list, are rejected with an error.

---

## When to use which editing tool

| Situation | Recommended tool |
|-----------|-----------------|
| Rewrite a function or method body | `replace_symbol` |
| Add a new function next to an existing one | `insert_code` |
| Rename a symbol everywhere it is used | `rename_symbol` |
| Edit lines inside a function when you know their numbers | `edit_lines` |
| Edit a config file, Markdown, or other non-code file | `edit_lines` |
| Create a new file | `create_file` |

For source code, prefer the symbol tools. They address code by name rather than by line number, which means your edit remains correct even if the file was modified since you last read it. Fall back to `edit_lines` when you need to change something inside a function body at a known position.

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

- Use this tool when you are generating a file from scratch. If the file already exists and you only need to change part of it, use `edit_lines` or a symbol tool instead — those tools are less likely to accidentally discard content you did not intend to touch.
- The path is resolved relative to the active project root, so `src/util/helpers.rs` and `./src/util/helpers.rs` both work. Absolute paths that fall inside the project root are also accepted.

---

## `edit_lines`

**Purpose:** Splice-edit a file by line position. Replace a range of lines, insert new lines, or delete lines — without having to send the old content as a match string.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File to edit, relative to project root |
| `start_line` | integer | yes | — | First line affected by the edit (1-based) |
| `delete_count` | integer | yes | — | Number of lines to remove; `0` means pure insertion |
| `new_text` | string | no | `""` | Text to insert at `start_line`; may contain newlines. Omit for pure deletion. |

The operation is a splice: `delete_count` lines beginning at `start_line` are removed and replaced by the lines in `new_text`. Setting `delete_count` to `0` inserts without removing anything. Omitting `new_text` (or setting it to `""`) deletes without inserting.

**Example — replace a single line:**

```json
{
  "path": "src/config.rs",
  "start_line": 42,
  "delete_count": 1,
  "new_text": "    pub const MAX_RETRIES: u32 = 5;"
}
```

**Example — insert two lines before line 10 (no deletion):**

```json
{
  "path": "src/lib.rs",
  "start_line": 10,
  "delete_count": 0,
  "new_text": "// Safety: caller ensures the pointer is valid.\n// See module docs for invariants."
}
```

**Example — delete lines 15 through 17:**

```json
{
  "path": "src/lib.rs",
  "start_line": 15,
  "delete_count": 3
}
```

**Example — append at end of file (set `start_line` to one past the last line):**

```json
{
  "path": "src/lib.rs",
  "start_line": 101,
  "delete_count": 0,
  "new_text": "\n// end of file marker"
}
```

**Output:**

```json
{
  "status": "ok",
  "path": "/home/user/project/src/config.rs",
  "lines_deleted": 1,
  "lines_inserted": 1,
  "new_total_lines": 120
}
```

The tool preserves the file's trailing newline. Requesting a `start_line` beyond the end of the file, or a `delete_count` that would reach past the end, returns an error.

**Tips:**

- Use `read_file` with `start_line`/`end_line` to confirm the exact line numbers before making the edit.
- `edit_lines` is well suited to changes inside a function body when you already know the line range from a prior `read_file` or `find_symbol` call.
- For adding a completely new top-level definition adjacent to an existing one, `insert_code` is more convenient because you address the location by symbol name rather than line number.

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
- `rename_symbol` requires a running LSP server for the file's language. If the LSP is unavailable, the tool returns an error — fall back to `search_pattern` combined with manual `edit_lines` in that case, but be aware it will not respect scoping.
- After a `replace_symbol` edit, the file's line numbers shift. If you plan to make additional `edit_lines` edits in the same file, re-read the file or call `list_symbols` again to get fresh line numbers.
