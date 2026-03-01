# Progressive Disclosure

LLM context windows are finite. Every token spent on output you didn't need is
a token that could have held something useful. code-explorer is designed around
a single principle to address this: show the minimum that is actionable, and
reveal detail only when asked.

## The Problem

Without guardrails, code intelligence tools can produce enormous output:

| Tool | Worst case |
|------|------------|
| `list_symbols(dir)` | Walks the entire project, dumps every symbol in every file |
| `find_symbol(pattern)` | Project-wide search with thousands of matches |
| `find_references` | Popular symbols referenced in hundreds of files |
| `list_dir(recursive=true)` | Full directory tree of a large monorepo |
| `git_blame` (no line range) | Every line in a long file |

Filling the context window with irrelevant symbols, boilerplate bodies, and
off-target files makes it harder to reason about the code you actually care
about. It also wastes time: you pay the cost of processing all that output
before you can identify what you need.

## The Solution: Two Modes

code-explorer tools operate in one of two modes, controlled by the
`detail_level` parameter.

**Exploring mode** (the default) produces compact summaries: names, kinds,
file paths, and line numbers. Results are capped at 200 items. No function
bodies, no full diffs, no deep trees. The goal is to give you a map of the
territory so you can identify your target.

**Focused mode** (`detail_level: "full"`) produces complete detail: function
bodies, full symbol trees, entire diffs. Results are paginated via `offset` and
`limit` (default page size: 50). Use this only after you know what you are
looking for.

## How OutputGuard Enforces This

Every tool that can produce unbounded output delegates its output control to a
shared `OutputGuard` struct (in `src/tools/output.rs`). Tools do not implement
their own truncation logic; the guard enforces consistent behavior
project-wide.

`OutputGuard::from_input()` reads three optional fields from a tool's JSON
input:

- `detail_level`: `"full"` activates Focused mode; anything else (or absent)
  gives Exploring mode.
- `offset`: where to start in paginated output (default: 0).
- `limit`: page size in Focused mode (default: 50). If you pass an explicit
  `limit` in Exploring mode, the guard honours it as a cap.

`should_include_body()` returns `true` only in Focused mode. Tools use this to
decide whether to fetch and include function bodies from the language server.

`cap_items()` and `cap_files()` enforce the limits:

- In Exploring mode: keep the first `max_results` (or `max_files`) items,
  discard the rest, attach an `overflow` object describing what was omitted.
- In Focused mode: apply `offset`/`limit` pagination, attach an `overflow`
  object that includes `next_offset` when more pages remain.

When results exceed the cap, the overflow object tells you what to do next:

```json
{
  "overflow": {
    "shown": 47,
    "total": 312,
    "hint": "Narrow with a file path or glob pattern"
  }
}
```

In Focused mode, the overflow includes `next_offset` for sequential pagination:

```json
{
  "overflow": {
    "shown": 50,
    "total": 312,
    "hint": "Use offset/limit to page through results",
    "next_offset": 50
  }
}
```

## The Pattern: Explore, Identify, Focus

The intended workflow has three steps:

1. **Explore broadly.** Use tools in their default Exploring mode to get a
   compact map of the area you care about.
2. **Identify your target.** Read the compact output to find the file, symbol,
   or range that contains what you need.
3. **Focus narrowly.** Switch to Focused mode on exactly that target to get
   full detail.

### Example

You want to understand the authentication logic in a service layer.

**Step 1 — get the map:**

```json
{ "tool": "list_symbols", "arguments": { "path": "src/services/" } }
```

Response (compact, exploring mode):

```json
{
  "files": [
    {
      "file": "src/services/auth.rs",
      "symbols": [
        { "name": "AuthService", "kind": "Struct", "start_line": 12 },
        { "name": "handle_login", "kind": "Function", "start_line": 34 },
        { "name": "verify_token", "kind": "Function", "start_line": 61 },
        { "name": "refresh_session", "kind": "Function", "start_line": 89 }
      ]
    },
    {
      "file": "src/services/user.rs",
      "symbols": [
        { "name": "UserService", "kind": "Struct", "start_line": 8 },
        { "name": "find_by_email", "kind": "Function", "start_line": 22 }
      ]
    }
  ]
}
```

Four symbols in `auth.rs`, `verify_token` looks relevant. Total context
consumed: a few dozen tokens.

**Step 2 — identify the target:** `verify_token` in `src/services/auth.rs`.

**Step 3 — focus on it:**

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

Now you get the full function body — but only for the one function you
identified, not for every symbol in the file.

This workflow minimizes context usage at every step. The broad exploration is
cheap; the focused read is exact.

## Further Reading

- [Output Modes](output-modes.md) — the `detail_level`, `offset`, and `limit`
  parameters in full detail, with examples for every tool
- [Tool Selection](tool-selection.md) — matching your level of knowledge to the
  right tool, including the anti-patterns that cause context bloat
