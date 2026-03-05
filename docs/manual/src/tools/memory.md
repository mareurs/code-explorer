# Memory

The `memory` tool gives the agent persistent, project-scoped storage. Notes written in one session are available in every future session, letting the agent build up knowledge about a codebase over time rather than rediscovering the same things repeatedly.

## `memory`

**Purpose:** Read, write, list, or delete persistent memory entries via a single unified tool.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `action` | string | **yes** | — | One of: `"read"`, `"write"`, `"list"`, `"delete"` |
| `topic` | string | required for `read`/`write`/`delete` | — | Path-like key, e.g. `"architecture"` or `"debugging/async-patterns"` |
| `content` | string | required for `write` | — | Markdown text to persist |
| `private` | boolean | no | `false` | If true, use the gitignored private store (personal notes not shared with teammates) |
| `include_private` | boolean | no | `false` | For `list`: also return private topics — returns `{ shared, private }` instead of `{ topics }` |

---

### `action: "write"`

Persist a piece of knowledge under a named topic.

**Example:**

```json
{
  "action": "write",
  "topic": "conventions/error-handling",
  "content": "All public functions return `anyhow::Result`. Errors are propagated with `?`. Only `main` and tool `call` methods convert to user-facing messages."
}
```

**Output:** `"ok"`

**Tips:** Write a memory whenever you learn something non-obvious — a naming convention, an architectural decision, a gotcha you had to debug. Topics with a slash create a sub-directory, which keeps related entries grouped. Calling `write` with an existing topic overwrites it.

---

### `action: "read"`

Retrieve a previously stored memory entry by its topic.

**Example:**

```json
{ "action": "read", "topic": "conventions/error-handling" }
```

**Output (found):**

```json
{
  "content": "All public functions return `anyhow::Result`. Errors are propagated with `?`. Only `main` and tool `call` methods convert to user-facing messages."
}
```

**Output (not found):** Returns a `RecoverableError` with a hint to call `list` first.

**Tips:** Read memories that are relevant to your current task. Use `list` first to see what topics exist, then read only the ones that apply.

---

### `action: "list"`

List all stored memory topics for the active project.

**Example:**

```json
{ "action": "list" }
```

**Output:**

```json
{
  "topics": [
    "architecture",
    "conventions/error-handling",
    "conventions/naming",
    "debugging/lsp-timeouts",
    "onboarding"
  ]
}
```

**With private topics:**

```json
{ "action": "list", "include_private": true }
```

**Output:**

```json
{
  "shared": ["architecture", "conventions/error-handling"],
  "private": ["personal/wip-notes"]
}
```

**Tips:** Call this at the start of a session to get an overview of what the agent already knows. Topics with slashes indicate sub-categories — scan the list for entries relevant to your current task.

---

### `action: "delete"`

Remove a memory entry that is no longer accurate or needed.

**Example:**

```json
{ "action": "delete", "topic": "debugging/lsp-timeouts" }
```

**Output:** `"ok"`

**Tips:** Delete memories when a refactor changes the architecture they describe, or when a bug they document has been fixed. Stale memories are worse than no memories. Deleting a topic that does not exist is a no-op.

---

## Private Store

Pass `private: true` to any action to target the gitignored private store at `.codescout/private-memories/`. Private memories are never surfaced in system instructions and are not shared with teammates:

```json
{ "action": "write", "topic": "wip-notes", "content": "...", "private": true }
```

---

## Using Memory Effectively

### Storage layout

Memories are stored as plain Markdown files in `.codescout/memories/` inside the project root. Each topic maps directly to a file path:

- `"architecture"` → `.codescout/memories/architecture.md`
- `"debugging/async-patterns"` → `.codescout/memories/debugging/async-patterns.md`

You can inspect or version-control these files like any other project file.

### Topic naming

Topics support path-like nesting with forward slashes:

| Category | Example topics |
|----------|---------------|
| Project conventions | `conventions/naming`, `conventions/error-handling`, `conventions/testing` |
| Architecture | `architecture`, `architecture/data-flow`, `architecture/module-boundaries` |
| Debugging notes | `debugging/async-patterns`, `debugging/known-issues` |
| Team preferences | `preferences/review-style`, `preferences/commit-format` |
| Onboarding summary | `onboarding` (written automatically by the `onboarding` tool) |

### What to store

Good candidates:

- **Project conventions** — naming rules, code style decisions not captured by linting
- **Architectural decisions** — why a module is structured a particular way, trade-offs consciously made
- **Debugging insights** — root causes of tricky bugs, non-obvious component interactions
- **Gotchas** — behaviours that surprised you and would surprise the next agent too

Avoid storing things already obvious from reading the code, or things that change so frequently the memory would immediately go stale.

### Recommended workflow

1. Start a new session → call `onboarding` (lists available memories if already done)
2. Call `memory(action: "list")` to see what topics exist
3. Call `memory(action: "read", topic: ...)` for topics relevant to your current task
4. As you work, call `memory(action: "write", ...)` when you learn something worth remembering
5. If you correct an earlier misunderstanding, overwrite the old entry with updated content

> **See also:** [Dashboard](../concepts/dashboard.md) — the Memories page lets
> you browse, create, and delete topics directly in a browser UI without writing
> tool calls.
