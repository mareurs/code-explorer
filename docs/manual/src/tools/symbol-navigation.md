# Symbol Navigation

These eight tools give you IDE-grade code navigation backed by the Language
Server Protocol. They understand code structure — symbols, definitions,
references, types — not just bytes and lines.

**Supported languages:** Rust, Python, TypeScript, JavaScript, TSX, JSX, Go,
Java, Kotlin, C, C++, C#, Ruby.

All symbol navigation tools require an LSP server for the target language. If
the LSP server is not running or is still indexing, some tools fall back to
tree-sitter for basic results.

**Scope parameter:** `find_symbol`, `list_symbols`, and `find_references` accept an optional `scope` string to search library code as well as project code. See [Library Navigation](library-navigation.md) for the full scope reference.

> **See also:** [Tool Selection](../concepts/tool-selection.md) — when to reach
> for symbol tools vs semantic search vs text search. [Progressive Disclosure](../concepts/progressive-disclosure.md) — how `detail_level` controls output volume for these tools.

---

## `list_symbols`

**Purpose:** Return the symbol tree (functions, classes, methods, structs, etc.)
for a file, directory, or glob pattern. Use this to orient yourself before
reading or editing.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `relative_path` | string | no | project root | File, directory, or glob pattern (e.g. `src/**/*.rs`) |
| `depth` | integer | no | 1 | Depth of children to include (0 = top-level names only, 1 = direct children) |
| `detail_level` | string | no | exploring | `"full"` activates focused mode with symbol bodies |
| `offset` | integer | no | 0 | Skip this many files (focused mode pagination) |
| `limit` | integer | no | 50 | Max files per page |

**Example — outline a single file:**

```json
{
  "tool": "list_symbols",
  "arguments": {
    "relative_path": "src/auth/middleware.rs"
  }
}
```

**Output (exploring mode):**

```json
{
  "file": "src/auth/middleware.rs",
  "symbols": [
    { "name": "AuthMiddleware", "kind": "Struct", "start_line": 12, "end_line": 18 },
    { "name": "new",            "kind": "Function", "start_line": 21, "end_line": 28 },
    { "name": "handle",         "kind": "Function", "start_line": 31, "end_line": 74 },
    { "name": "verify_token",   "kind": "Function", "start_line": 77, "end_line": 102 }
  ]
}
```

**Output (focused mode — `detail_level: "full"`):**

Same structure, but each symbol includes a `"body"` field with the source lines
from `start_line` to `end_line`.

**Example — overview of a directory:**

```json
{
  "tool": "list_symbols",
  "arguments": {
    "relative_path": "src/handlers/"
  }
}
```

Returns one entry per file in the directory, each with its symbol list. At the
project root (`.`), walks the entire source tree recursively.

**Example — glob across multiple files:**

```json
{
  "tool": "list_symbols",
  "arguments": {
    "relative_path": "src/**/*.py",
    "depth": 0
  }
}
```

With `depth: 0`, returns only top-level symbol names, which is useful for a
very high-level map without the per-method detail.

**Tips:**

- Use this before reading or editing. Two tokens spent on the map saves dozens
  spent re-reading the wrong file.
- When the result overflows, the response includes an `overflow` object with
  a hint. Narrow with a more specific path or glob.
- For deep class hierarchies, increase `depth` to 2 or 3.
- Use `detail_level: "full"` only after you have identified the specific file
  you need — fetching bodies for an entire directory is expensive.

---

## `find_symbol`

**Purpose:** Find symbols by name pattern across the project or within a
specific file. Returns matching symbols with their location and, optionally,
their source body.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `pattern` | string | yes | — | Symbol name or substring (case-insensitive) |
| `relative_path` | string | no | — | Restrict to this file or glob pattern |
| `include_body` | boolean | no | false | Include source body in results |
| `depth` | integer | no | 0 | Depth of children to include |
| `detail_level` | string | no | exploring | `"full"` for bodies and pagination |
| `offset` | integer | no | 0 | Skip this many results |
| `limit` | integer | no | 50 | Max results per page |

**Example — find a function anywhere in the project:**

```json
{
  "tool": "find_symbol",
  "arguments": {
    "pattern": "authenticate_user"
  }
}
```

**Output (exploring mode):**

```json
{
  "symbols": [
    {
      "name": "authenticate_user",
      "name_path": "AuthService/authenticate_user",
      "kind": "Function",
      "file": "src/auth/service.rs",
      "start_line": 44,
      "end_line": 89
    }
  ],
  "total": 1
}
```

**Example — find with body, restricted to one file:**

```json
{
  "tool": "find_symbol",
  "arguments": {
    "pattern": "authenticate_user",
    "relative_path": "src/auth/service.rs",
    "include_body": true,
    "detail_level": "full"
  }
}
```

**Output (focused mode with body):**

```json
{
  "symbols": [
    {
      "name": "authenticate_user",
      "name_path": "AuthService/authenticate_user",
      "kind": "Function",
      "file": "src/auth/service.rs",
      "start_line": 44,
      "end_line": 89,
      "body": "pub fn authenticate_user(&self, credentials: Credentials) -> Result<Session> {\n    let user = self.user_store.find_by_email(&credentials.email)?;\n    ..."
    }
  ],
  "total": 1
}
```

**Example — find all test functions in test files:**

```json
{
  "tool": "find_symbol",
  "arguments": {
    "pattern": "test_",
    "relative_path": "tests/**/*.rs"
  }
}
```

**Tips:**

- Pattern matching is case-insensitive substring matching. `"auth"` matches
  `AuthService`, `authenticate_user`, and `reauth_token`.
- Without `relative_path`, uses `workspace/symbol` (one LSP request per
  language), which is fast. With `relative_path`, uses per-file document
  symbols, which is slower but scoped.
- `name_path` in the result uses `/` as a separator for nested symbols, e.g.
  `AuthService/authenticate_user`. You need this value for
  `find_references`, `replace_symbol`, and related editing tools.
- Use `include_body: true` in the same call to avoid a separate read step when
  you already know the symbol name.

---

## `find_references`

**Purpose:** Find all locations in the codebase that reference (call, use,
import) a given symbol. This is the "find all usages" feature from your IDE.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol identifier, e.g. `"MyStruct/my_method"` |
| `relative_path` | string | yes | — | File that contains the symbol definition |
| `detail_level` | string | no | exploring | `"full"` for pagination |
| `offset` | integer | no | 0 | Skip this many results |
| `limit` | integer | no | 50 | Max results per page |

**Example — find all callers of a method:**

```json
{
  "tool": "find_references",
  "arguments": {
    "name_path": "AuthService/authenticate_user",
    "relative_path": "src/auth/service.rs"
  }
}
```

**Output (exploring mode):**

```json
{
  "references": [
    {
      "file": "src/handlers/login.rs",
      "line": 38,
      "column": 18,
      "context": "        let session = self.auth.authenticate_user(credentials)?;"
    },
    {
      "file": "src/handlers/api.rs",
      "line": 204,
      "column": 22,
      "context": "        auth_service.authenticate_user(req.credentials.clone())?;"
    },
    {
      "file": "tests/auth_integration.rs",
      "line": 91,
      "column": 12,
      "context": "    let result = service.authenticate_user(bad_creds);"
    }
  ],
  "total": 3
}
```

**Example — paginate a high-reference symbol:**

```json
{
  "tool": "find_references",
  "arguments": {
    "name_path": "Logger/log",
    "relative_path": "src/logging.rs",
    "detail_level": "full",
    "offset": 0,
    "limit": 25
  }
}
```

**Tips:**

- Both `name_path` and `relative_path` are required. The LSP needs to locate
  the symbol's definition position before it can find references.
- `name_path` must match the `name_path` value from `list_symbols` or
  `find_symbol` output, not just the bare name. For a top-level function, the
  name_path is just the function name (e.g. `"validate_token"`). For a method,
  it is `"StructName/method_name"`.
- Each reference includes a `context` line showing the source at that location,
  so you can often determine the call pattern without reading the file.
- For symbols with many references (e.g. utility functions, common types),
  use `detail_level: "full"` with `offset`/`limit` pagination.

---

## `replace_symbol`

**Purpose:** Replace the entire body of a named symbol with new source code.
The tool locates the symbol by name via LSP and replaces the lines from its
start to its end — no line numbers required.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol identifier, e.g. `"MyStruct/my_method"` |
| `relative_path` | string | yes | — | File containing the symbol |
| `new_body` | string | yes | — | Complete replacement source for the symbol |

**Example — rewrite a Rust function:**

```json
{
  "tool": "replace_symbol",
  "arguments": {
    "name_path": "UserService/find_by_email",
    "relative_path": "src/services/user.rs",
    "new_body": "pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {\n    self.db\n        .query_opt(\"SELECT * FROM users WHERE email = $1\", &[&email])\n        .await\n        .map(|row| row.map(User::from_row))\n}"
  }
}
```

**Output:**

```json
{
  "status": "ok",
  "replaced_lines": "34-67"
}
```

**Example — update a Python method:**

```json
{
  "tool": "replace_symbol",
  "arguments": {
    "name_path": "TokenValidator/validate",
    "relative_path": "auth/validators.py",
    "new_body": "def validate(self, token: str) -> Claims:\n    try:\n        return jwt.decode(token, self.secret, algorithms=[\"HS256\"])\n    except jwt.ExpiredSignatureError:\n        raise TokenExpiredError(token)"
  }
}
```

**Tips:**

- The `new_body` replaces the entire span from the symbol's `start_line` to its
  `end_line`. Include everything: the function signature, decorators if they are
  within the span, and the closing brace.
- Read the current body first with `find_symbol` + `include_body: true` before
  rewriting, to confirm you understand the existing signature and indentation.
- Use `replace_symbol` for any change that touches a significant portion
  of the function. For small surgical changes (renaming a variable, changing
  one line), `edit_file` with a precise match string is less disruptive.
- This tool is robust to refactors above the target function. Line numbers
  change; symbol names generally do not.

---

## `insert_code`

**Purpose:** Insert code immediately before or after a named symbol. Addresses the insertion point by symbol name — no line numbers needed.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol name path (e.g. `'MyStruct/my_method'`) |
| `path` | string | yes | — | File path (relative or absolute) |
| `code` | string | yes | — | Code to insert (may contain newlines) |
| `position` | string | no | `"after"` | `"before"` or `"after"` the symbol |

**Example — add a helper function after an existing one:**

```json
{
  "tool": "insert_code",
  "arguments": {
    "name_path": "validate_token",
    "path": "src/auth/service.rs",
    "code": "\npub fn revoke_token(token: &str) -> Result<()> {\n    TOKEN_STORE.remove(token);\n    Ok(())\n}\n",
    "position": "after"
  }
}
```

**Example — add a test before a function:**

```json
{
  "tool": "insert_code",
  "arguments": {
    "name_path": "validate_token",
    "path": "src/auth/service.rs",
    "code": "#[test]\nfn test_validate_token_rejects_expired() {\n    // ...\n}\n",
    "position": "before"
  }
}
```

**Tips:**

- Use `insert_code` when you want to add a new function, method, or block adjacent to an existing one without knowing its exact line numbers.
- The symbol is located via LSP, so the insertion point is robust to edits above the target.
- `position: "before"` inserts immediately above the symbol's first line; `"after"` inserts immediately below the symbol's last line.
- The inserted code is not validated for syntax — make sure it compiles before committing.

---

## `rename_symbol`

**Purpose:** Rename a symbol across the entire codebase using the LSP rename
operation. Every reference in every file is updated atomically.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol identifier |
| `relative_path` | string | yes | — | File containing the symbol definition |
| `new_name` | string | yes | — | New name for the symbol |

**Example — rename a Rust struct:**

```json
{
  "tool": "rename_symbol",
  "arguments": {
    "name_path": "AuthMiddleware",
    "relative_path": "src/auth/middleware.rs",
    "new_name": "AuthenticationMiddleware"
  }
}
```

**Output:**

```json
{
  "status": "ok",
  "files_changed": 4,
  "total_edits": 11
}
```

**Example — rename a Python function:**

```json
{
  "tool": "rename_symbol",
  "arguments": {
    "name_path": "validate_token",
    "relative_path": "auth/validators.py",
    "new_name": "verify_access_token"
  }
}
```

**Example — rename a TypeScript interface:**

```json
{
  "tool": "rename_symbol",
  "arguments": {
    "name_path": "UserDto",
    "relative_path": "src/types/user.ts",
    "new_name": "UserResponse"
  }
}
```

**Tips:**

- `rename_symbol` is the safest way to rename. It uses the LSP workspace edit
  operation, which understands import paths, qualified names, and string-based
  references that IDEs handle. A text substitution with `search_pattern` + `edit_file` will
  miss these cases.
- The `relative_path` must point to the file that contains the definition, not
  a file that merely uses it.
- `name_path` must identify the definition unambiguously. If there are two
  symbols with the same name in a file (e.g. a field and a method), use the
  full `name_path` with the parent (e.g. `"MyStruct/value"`) to distinguish them.
- After renaming, verify with `find_symbol` on the new name to confirm all
  occurrences were updated.
- LSP rename support varies by server. Most servers handle functions, methods,
  classes, variables, and fields. Some do not rename string literals or
  macro-generated identifiers. Check the result's `files_changed` count against


---

## `remove_symbol`

**Purpose:** Delete a named symbol (function, struct, method, test, etc.) entirely from a file.
Uses LSP to identify the exact line range covered by the symbol — no manual line counting required.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol identifier, e.g. `"MyStruct/my_method"` or `"old_helper"` |
| `path` | string | yes | — | File containing the symbol |

**Example — delete a deprecated function:**

```json
{
  "tool": "remove_symbol",
  "arguments": {
    "name_path": "legacy_auth_check",
    "path": "src/auth/middleware.rs"
  }
}
```

**Output:** `"ok"`

**Tips:**

- Use `find_references` first to confirm nothing still calls the symbol before removing it.
- For methods on a struct or class, use the full path: `"MyStruct/my_method"`.
- The tool removes the exact LSP range — it will not leave behind stray blank lines from adjacent doc comments if they fall outside the symbol's range. Review the diff after removal.
- If you want to replace rather than delete, use `replace_symbol` instead.

---

## `goto_definition`

**Purpose:** Jump to the definition of a symbol at a given line. Resolves types via LSP — handles method calls, trait implementations, and cross-crate navigation. When the definition lives outside the project root, the library is auto-discovered and registered in `list_libraries`.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path (relative or absolute) |
| `line` | integer | yes | — | 1-indexed line number to jump from |
| `identifier` | string | no | — | Optional identifier on the line to target (disambiguates when multiple symbols are on the same line) |

**Example — jump to the definition of a symbol on line 42:**

```json
{
  "tool": "goto_definition",
  "arguments": {
    "path": "src/tools/symbol.rs",
    "line": 42
  }
}
```

**Output:**

```json
{
  "definitions": [
    {
      "file": "src/lsp/symbols.rs",
      "start_line": 12,
      "end_line": 28
    }
  ]
}
```

When the definition is in a library (outside the project root), the response includes a `library_registered` field:

```json
{
  "definitions": [
    {
      "file": "/home/user/.cargo/registry/src/serde-1.0.196/src/lib.rs",
      "start_line": 314,
      "end_line": 320
    }
  ],
  "library_registered": "serde"
}
```

**Tips:**

- Use `goto_definition` to quickly locate where a type, trait, or function is defined without searching by name.
- When the definition is in an external library and `library_registered` appears, run `index_project` with the library's root path to enable `semantic_search` across it.
- Supply `identifier` when the line has multiple symbols (e.g. a method call chain) to pin the lookup to the one you care about.
- The result is the definition site — if you want the symbol's type and documentation without navigating away, use `hover` instead.

---

## `hover`

**Purpose:** Get type information and documentation for a symbol at a given position. Returns the type signature, inferred types, and doc comments without navigating away from the current file. Complements `find_symbol` (name lookup) and `goto_definition` (navigation).

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path (relative or absolute) |
| `line` | integer | yes | — | 1-indexed line number |
| `identifier` | string | no | — | Optional identifier on the line to target (disambiguates when multiple symbols are on the same line) |

**Example — inspect the type of a variable:**

```json
{
  "tool": "hover",
  "arguments": {
    "path": "src/tools/symbol.rs",
    "line": 55
  }
}
```

**Output:**

```json
{
  "content": "pub fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>\n\nExecute the tool with the given JSON input and return a JSON result."
}
```

The `content` field contains the formatted hover text — typically a type signature followed by any doc comments the language server found.

**Tips:**

- Use `hover` when you want to understand a type without reading the full definition. It is faster than `goto_definition` + `read_file` for orientation tasks.
- `hover` is especially useful for complex inferred types (closures, iterator chains) where the type is not written explicitly in the source.
- If `content` is empty, the language server has no information for that position — try adjusting `line` by ±1 or supplying `identifier` to target a specific token.
- Supply `identifier` on lines with multiple symbols (a method call, a generic type parameter, a trait bound) to pin the hover to the right one.
  your expectations.
