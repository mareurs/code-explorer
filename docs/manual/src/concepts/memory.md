# Memory

Memory gives codescout persistent, project-scoped storage that outlives any
single conversation. Notes written in one session are available in every future
session — the agent accumulates knowledge about a codebase over time rather than
rediscovering the same things repeatedly.

## The Problem It Solves

Without persistent memory, every new session starts from scratch. The agent has
to re-read CLAUDE.md, re-run onboarding, and re-discover facts it already knew:
which module handles authentication, where the main entry point is, what
convention the project uses for error types. This re-discovery burns time and
context window on every session.

With memory, the agent writes a note the first time it discovers something
non-obvious. Every subsequent session reads that note immediately and skips the
rediscovery entirely.

## Storage Layout

Memories are plain Markdown files in `.code-explorer/memories/`:

```
.code-explorer/memories/
  architecture.md
  conventions/
    error-handling.md
    naming.md
  debugging/
    lsp-timeouts.md
```

Topics with forward slashes map to subdirectories. You can version-control
memory files alongside code, or keep them local.

## Typical Workflow

At the start of a session:
1. Call `onboarding` — it lists existing memories and skips heavy discovery if
   memories are already written
2. Call `read_memory` for topics relevant to the current task

During a session:
3. Call `write_memory` when you discover something worth remembering — a
   naming convention, an architectural decision, a gotcha

At the end of a session:
4. Call `write_memory` to update entries if your understanding changed

## What Makes a Good Memory Entry

Good candidates:
- **Architectural decisions** — why a module is structured a certain way
- **Naming conventions** — patterns used throughout the codebase that aren't
  obvious from reading one file
- **Debugging insights** — root causes of tricky issues, non-obvious interactions
- **Entry points** — which file/function to start from for a given concern
- **Gotchas** — behaviours that surprised you and would surprise the next session

Avoid:
- Things obvious from reading the code
- Things that change so frequently the memory goes stale immediately
- Duplicating information already in CLAUDE.md

## Onboarding Integration

The `onboarding` tool automatically writes a summary entry under the topic
`"onboarding"`. This entry contains language detection results, detected entry
points, and a system prompt draft for the routing plugin. You do not need to
write it manually.

## Further Reading

- [Memory Tools](../tools/memory.md) — full reference for `write_memory`,
  `read_memory`, `list_memories`, and `delete_memory`
- [Dashboard](dashboard.md) — the Memories page lets you browse and edit topics
  in a browser UI
- [Workflow & Config Tools](../tools/workflow-and-config.md) — `onboarding`
  integrates with memory at session start
