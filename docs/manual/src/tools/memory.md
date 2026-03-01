# Memory Tools

Memory tools give the agent persistent, project-scoped storage. Notes written in one session are available in every future session, letting the agent build up knowledge about a codebase over time rather than rediscovering the same things repeatedly.

## `write_memory`

**Purpose:** Persist a piece of knowledge about the active project under a named topic.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `topic` | string | yes | — | Path-like key, e.g. `"architecture"` or `"debugging/async-patterns"` |
| `content` | string | yes | — | Markdown text to store |

**Example:**

```json
{
  "topic": "conventions/error-handling",
  "content": "All public functions return `anyhow::Result`. Errors are propagated with `?`. Only `main` and tool `call` methods convert to user-facing messages."
}
```

**Output:**

```json
{ "status": "ok", "topic": "conventions/error-handling" }
```

**Tips:** Write a memory whenever you learn something non-obvious — a naming convention, an architectural decision, a gotcha you had to debug. Topics with a slash create a sub-directory, which keeps related entries grouped. Calling `write_memory` with an existing topic overwrites it.

---

## `read_memory`

**Purpose:** Retrieve a previously stored memory entry by its topic.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `topic` | string | yes | — | Exact topic string used when the entry was written |

**Example:**

```json
{ "topic": "conventions/error-handling" }
```

**Output (found):**

```json
{
  "topic": "conventions/error-handling",
  "content": "All public functions return `anyhow::Result`. Errors are propagated with `?`. Only `main` and tool `call` methods convert to user-facing messages."
}
```

**Output (not found):**

```json
{
  "topic": "conventions/error-handling",
  "content": null,
  "message": "not found"
}
```

**Tips:** Read memories that are relevant to your current task. Do not read all memories at once — use `list_memories` first to see what exists, then read only the ones that apply to what you are working on.

---

## `list_memories`

**Purpose:** List all stored memory topics for the active project.

**Parameters:** None.

**Example:**

```json
{}
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

**Tips:** Call this at the start of a session to get an overview of what the agent already knows about the project. The list is sorted alphabetically. Topics with slashes indicate sub-categories — scan the list for entries relevant to your current task, then use `read_memory` to fetch them individually.

---

## `delete_memory`

**Purpose:** Remove a memory entry that is no longer accurate or needed.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `topic` | string | yes | — | Exact topic of the entry to delete |

**Example:**

```json
{ "topic": "debugging/lsp-timeouts" }
```

**Output:**

```json
{ "status": "ok", "topic": "debugging/lsp-timeouts" }
```

**Tips:** Delete memories when a refactor changes the architecture they describe, or when a bug they document has been fixed. Stale memories are worse than no memories — they send future sessions down dead ends. Deleting a topic that does not exist is a no-op; it does not return an error.

---

## Using Memory Effectively

### Storage layout

Memories are stored as plain Markdown files in `.code-explorer/memories/` inside the project root. Each topic maps directly to a file path:

- `"architecture"` → `.code-explorer/memories/architecture.md`
- `"debugging/async-patterns"` → `.code-explorer/memories/debugging/async-patterns.md`

You can inspect or version-control these files like any other project file.

### Topic naming

Topics support path-like nesting with forward slashes. A flat structure works fine for small projects; for larger codebases, grouping by category keeps the list scannable:

| Category | Example topics |
|----------|---------------|
| Project conventions | `conventions/naming`, `conventions/error-handling`, `conventions/testing` |
| Architecture | `architecture`, `architecture/data-flow`, `architecture/module-boundaries` |
| Debugging notes | `debugging/async-patterns`, `debugging/known-issues` |
| Team preferences | `preferences/review-style`, `preferences/commit-format` |
| Onboarding summary | `onboarding` (written automatically) |

### What to store

Good candidates for memory entries:

- **Project conventions** — naming rules, code style decisions not captured by linting, patterns used throughout the codebase
- **Architectural decisions** — why a module is structured a particular way, trade-offs that were consciously made
- **Debugging insights** — root causes of tricky bugs, non-obvious interactions between components
- **Team preferences** — review expectations, commit message style, PR process
- **Gotchas** — behaviours that surprised you and would surprise the next agent too

Avoid storing things that are already obvious from reading the code, or that change so frequently that the memory would immediately go stale.

### Persistence across sessions

Memory persists indefinitely across sessions. The recommended workflow is:

1. At the start of a new session, call `onboarding`. If onboarding has been done, it will list available memories.
2. Scan the list and call `read_memory` for topics relevant to your current task.
3. As you work, call `write_memory` when you learn something worth remembering.
4. If you correct an earlier misunderstanding, overwrite the old entry with updated content.

The `onboarding` tool writes a summary entry under the topic `"onboarding"` automatically. You can write all other entries manually as you explore the codebase.

> **See also:** [Dashboard](../concepts/dashboard.md) — the Memories page lets
> you browse, create, and delete topics directly in a browser UI without writing
> tool calls.
