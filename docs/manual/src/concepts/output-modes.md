# Output Modes

code-explorer tools produce different amounts of detail depending on which mode
they operate in. Understanding the two modes — and when to switch between them
— is the key to using the tools efficiently.

## Exploring Mode (Default)

Exploring mode is the default for every tool that supports it. You do not need
to pass any parameter to get it.

In Exploring mode, tools return compact summaries:

- **Symbols:** name, kind, file path, start and end line. No function bodies.
- **Files:** paths only, no content.
- **Diffs:** truncated at a reasonable size with a count of omitted files.
- **Blame:** first 200 lines, with an overflow message if the file is longer.

Results are capped at **200 items** (or 200 files for directory-spanning tools).
If more results exist, the response includes an `overflow` object explaining
what was omitted and how to narrow the query.

**Example — exploring mode response for `list_symbols`:**

```json
{
  "file": "src/services/auth.rs",
  "symbols": [
    { "name": "AuthService", "kind": "Struct", "start_line": 12, "end_line": 30 },
    { "name": "handle_login", "kind": "Function", "start_line": 34, "end_line": 60 },
    { "name": "verify_token", "kind": "Function", "start_line": 61, "end_line": 88 }
  ]
}
```

No bodies, no children — just the shape of the file.

## Focused Mode (`detail_level: "full"`)

Focused mode returns full detail and paginates results. Activate it by passing
`detail_level: "full"` to any tool that supports it.

In Focused mode, tools return:

- **Symbols:** full bodies, all children, complete detail.
- **Files:** full content of matching entries.
- **Diffs:** complete diff output, paginated by file if needed.
- **Blame:** paginated line-by-line blame for the full file.

Results are paginated via `offset` and `limit` (default page size: **50**).
The first page starts at `offset: 0`. Subsequent pages use the `next_offset`
value from the overflow object.

**Example — focused mode response for `find_symbol`:**

```json
{
  "symbols": [
    {
      "name": "verify_token",
      "kind": "Function",
      "file": "src/services/auth.rs",
      "start_line": 61,
      "end_line": 88,
      "body": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims> {\n    let key = DecodingKey::from_secret(secret);\n    let validation = Validation::new(Algorithm::HS256);\n    let data = decode::<Claims>(token, &key, &validation)\n        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;\n    if data.claims.exp < Utc::now().timestamp() as usize {\n        return Err(AuthError::TokenExpired);\n    }\n    Ok(data.claims)\n}"
    }
  ]
}
```

## Switching Between Modes

Pass `detail_level: "full"` to any tool that supports it:

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

```json
{
  "tool": "list_symbols",
  "arguments": {
    "path": "src/services/",
    "detail_level": "full",
    "limit": 10
  }
}
```
```

## Overflow Messages

When the number of results exceeds the cap, the response includes an `overflow`
object at the top level:

```json
{
  "results": [ "..." ],
  "overflow": {
    "shown": 47,
    "total": 312,
    "hint": "Narrow with a file path or glob pattern"
  }
}
```

The `hint` tells you how to reduce the result set. Common hints suggest
narrowing the path, providing a more specific pattern, or adding a glob filter.

In Focused mode, the overflow object also includes `next_offset` when more
pages exist:

```json
{
  "results": [ "..." ],
  "overflow": {
    "shown": 50,
    "total": 312,
    "hint": "Use offset and limit to page through results",
    "next_offset": 50
  }
}
```

When `next_offset` is absent (or `null`), you are on the last page.

**Paginating through results:**

```json
{ "tool": "find_symbol", "arguments": { "pattern": "Error", "detail_level": "full", "offset": 0,  "limit": 50 } }
{ "tool": "find_symbol", "arguments": { "pattern": "Error", "detail_level": "full", "offset": 50, "limit": 50 } }
{ "tool": "find_symbol", "arguments": { "pattern": "Error", "detail_level": "full", "offset": 100, "limit": 50 } }
```

## Tools That Support Both Modes

These tools respect `detail_level`, `offset`, and `limit`:

| Tool | Exploring output | Focused output |
|------|-----------------|----------------|
| `list_symbols` | Names, kinds, files, lines | Full symbol trees with bodies |
| `find_symbol` | Names, kinds, locations | + bodies (when `include_body=true`) |
| `find_references` | Reference locations | Paginated reference list |
| `list_dir` | File paths | Paginated entries |
| `git_blame` | First 200 lines | Paginated full blame |
| `semantic_search` | Top matches with scores | Paginated full chunks |

## Tools With Fixed Output

Some tools always cap their output at a fixed limit and do not support mode
switching:

| Tool | Behaviour |
|------|-----------|
| `search_pattern` | Always returns up to `max_results` matches (default: 50) |
| `find_file` | Always returns up to `max_results` paths (default: 100) |
| `list_functions` | Returns all functions in a single file; files are bounded by nature |
| `list_docs` | Returns all docstrings in a single file |

For these tools, use their own `limit` or `max_results` parameter to control
output size. They do not use the `detail_level` / `offset` / `limit` pattern.

## Practical Guidance

Use Exploring mode to find what you are looking for. Use Focused mode only once
you have a specific target. Switching early costs you context; switching at the
right time costs almost nothing.

If you see an overflow message in Exploring mode, prefer narrowing the query
(a more specific path, glob, or pattern) over switching to Focused mode. A
narrower Exploring query is usually more useful than a paginated Focused query
over 300 results.

If you see `next_offset` in Focused mode and you need all the results, page
through sequentially. Do not try to read all pages in a single pass unless you
need the full picture; often the first one or two pages contain the answer.
