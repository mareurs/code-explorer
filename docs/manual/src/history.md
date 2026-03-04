# From code-explorer to codescout

> **TL;DR**
> - The project was renamed. The binary is now `codescout`, not `code-explorer`.
> - Update your MCP config: change the server key from `code-explorer` to `codescout`.
> - Update any scripts or aliases that call the old binary name.
> - 9 tools were renamed and 3 consolidated — see the [CHANGELOG](../../../CHANGELOG.md) for the full mapping.

---

## The name

The original name was `code-explorer`. It made sense at the time — the tool helped an AI navigate
a codebase the way a developer would explore it in an IDE.

Two things changed.

First, the practical one: `code-explorer` was already taken on [crates.io](https://crates.io). A
Rust binary needs a crate name, and that one wasn't available.

Second, the honest one: the name had stopped fitting. By the time the rename happened, the tool
had grown persistent memory that survives across sessions, semantic search over embeddings, a shell
integration with output buffering, a project dashboard, and LSP-backed navigation across 9
languages. It wasn't just exploring files anymore. It was orienting an AI inside a codebase —
tracking context, surfacing what matters, remembering what was learned.

*Scout* felt closer to that. A scout doesn't just wander. It goes ahead, maps the terrain, and
comes back with something useful.

## What it grew into

The project started as file navigation. You could list symbols, search for patterns, read a
function body without dumping the whole file into context.

Then it got LSP: real go-to-definition, hover types, find-all-references — the same signals a
developer gets from their IDE, available to the AI.

Then semantic search: find code by concept, not just by text match. Then persistent memory: notes
the AI can read back next session, carrying context forward. Then shell integration with output buffers, so large command output
doesn't blow the context window. Then a dashboard for project health.

Each addition was driven by a recurring friction — the AI doing something clumsy that a better
tool could prevent. The scope kept expanding because the problem kept expanding.

## Migrating from code-explorer

If you were running `code-explorer` before, here's everything that changed at the API surface:

| What | Before | After |
|---|---|---|
| Binary name | `code-explorer` | `codescout` |
| MCP server key (`.mcp.json`) | `"code-explorer"` | `"codescout"` |
| Claude Code settings key | `"code-explorer"` | `"codescout"` |
| Cargo crate | `code-explorer` | `codescout` |

Update your `.mcp.json` (or Claude Code's `~/.claude/settings.json`) to use `"codescout"` as the
server key. The tool list and behavior are unchanged — it's a rename, not a rewrite.

## What else changed

Alongside the rename, the tool API was tidied up:

- **9 tools renamed** for consistency — plural `list_*` for enumeration, `find_*` for search,
  `search_*` for text/semantic. Full mapping in the [CHANGELOG](../../../CHANGELOG.md).
- **3 tools consolidated** — `insert_before_symbol` and `insert_after_symbol` merged into
  `insert_code(position: "before"|"after")`. `is_onboarded` folded into `onboarding(force: true)`.
