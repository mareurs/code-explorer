# Git Tools

The `git_blame` tool gives you read access to the repository history of the active project. It uses the `git2` library and operates directly on the local repository — no `git` binary needs to be installed.

`git_blame` requires an active project that is inside a git repository. It respects the project's path security settings: passing paths outside the project root is rejected.

---

## `git_blame`

**Purpose:** Return line-level blame for a file: who last changed each line,
the commit SHA, and the commit timestamp.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path relative to the project root |
| `start_line` | integer | no | — | First line to include (1-indexed, inclusive) |
| `end_line` | integer | no | — | Last line to include (1-indexed, inclusive) |
| `detail_level` | string | no | compact | `"full"` returns all lines without the default 50-line cap |
| `offset` | integer | no | `0` | Skip this many lines (pagination) |
| `limit` | integer | no | `50` | Maximum lines per page |

**Example (blame a specific range):**

```json
{
  "path": "src/auth.rs",
  "start_line": 100,
  "end_line": 140
}
```

**Output:**

```json
{
  "lines": [
    {
      "line": 100,
      "content": "pub fn authenticate_user(token: &str) -> Result<Session> {",
      "sha": "a3f8c120",
      "author": "Alice",
      "timestamp": 1706745600
    },
    {
      "line": 101,
      "content": "    let claims = decode_jwt(token)?;",
      "sha": "a3f8c120",
      "author": "Alice",
      "timestamp": 1706745600
    }
  ],
  "total": 41
}
```

Each entry has:
- `line` — 1-indexed line number in the file
- `content` — the line text (from the last committed version, not the working copy)
- `sha` — short commit SHA (8 characters) of the last commit that touched this line
- `author` — commit author name
- `timestamp` — Unix timestamp of the commit

When the result exceeds the cap, an `overflow` object is added with a hint on
how to retrieve more lines.

**Tips:**

- Use `start_line`/`end_line` to scope blame to the function you care about.
  Getting blame for an entire large file is rarely useful.
- `timestamp` is a Unix timestamp. Divide by 86400 to get days since epoch, or
  compare two values to see which change is more recent.
- Blame operates on the last committed version of the file. Uncommitted changes
  to the working directory are not reflected. Use `run_command` with `git diff` to see uncommitted working directory changes.
- To understand the full context of a change, take the `sha` from a blame line
  and check it in your git client.

