# Private Memories — Design

**Date:** 2026-03-01
**Status:** Approved

## Problem

All memories written via `write_memory` live in `.code-explorer/memories/` and are committed to git —
shared with every contributor. There is no place for developer-personal context: local machine config,
personal workflow preferences, WIP notes, and per-developer debugging history.

## Goal

Add a **private memory store** — gitignored, local to the developer — surfaced every session alongside
shared memories, with the LLM taught via system prompt rules when to use each store.

---

## Storage

**New directory:** `.code-explorer/private-memories/`

- Same layout as `.code-explorer/memories/` — one Markdown file per topic, path-like hierarchy
- `MemoryStore` is unchanged; a second instance is created pointing at `private-memories/`
- `ActiveProject` gets a second field: `private_memory: MemoryStore`
- When the private directory is **first created**, the code auto-appends
  `.code-explorer/private-memories/` to the project's `.gitignore` (creating `.gitignore` if absent)

### What belongs in private memories

| Private | Shared |
|---|---|
| Personal preferences and workflow rules | Architecture and module structure |
| Machine-specific config (ports, paths, GPU, env quirks) | Conventions and coding patterns |
| WIP notes and in-progress debugging context | Debugging techniques useful to all contributors |
| Personal debugging history specific to this setup | Design decisions with rationale |

**Default rule:** private first, promote to shared only if universally applicable.

---

## Tool API

Four existing tools each gain one optional parameter:

```
write_memory(topic, content, private?: bool = false)
read_memory(topic, private?: bool = false)
delete_memory(topic, private?: bool = false)
list_memories(include_private?: bool = false)
```

`list_memories(include_private=true)` returns two separate arrays:

```json
{
  "shared": ["architecture", "conventions", "gotchas"],
  "private": ["my-prefs", "wip/issue-42"]
}
```

---

## Session Surfacing (Approach B)

The `onboarding` tool's **already-onboarded fast path** (called every session when a project is
already set up) is extended to list private memories alongside shared ones.

### Response — currently

```json
{
  "onboarded": true,
  "memories": ["architecture", "conventions"],
  "message": "Available memories: architecture, conventions. Use read_memory(topic) to read relevant ones."
}
```

### Response — after this change

```json
{
  "onboarded": true,
  "memories": ["architecture", "conventions"],
  "private_memories": ["my-prefs", "wip/issue-42"],
  "message": "Available shared memories: architecture, conventions. Private memories: my-prefs, wip/issue-42. Read with read_memory(topic) or read_memory(topic, private=true)."
}
```

If `private_memories` is empty, the field is omitted and the message makes no mention of it (no noise).

---

## Onboarding: `system_prompt_draft` Injection

`build_system_prompt_draft()` in `src/tools/workflow.rs` gains a new section appended to the
generated `.code-explorer/system-prompt.md`:

```markdown
## Private Memory Rules

Private memories are gitignored — personal to this developer, not shared with the team.
They live in `.code-explorer/private-memories/`.

**Write to the private store** (`write_memory(topic, content, private=true)`) for:
- Personal preferences and workflow rules for this developer
- Machine-specific config (local ports, paths, GPU type, env quirks)
- WIP notes and in-progress debugging context
- Personal debugging history specific to this setup

**Write to the shared store** (`write_memory(topic, content)`) for:
- Architecture, conventions, design patterns — knowledge useful to ALL contributors
- When in doubt: private first, promote to shared only if universally applicable

**Each session:** `list_memories(include_private=true)` to see what's available.
```

`system-prompt.md` is committed — these are universal rules, not private content.

---

## `onboarding_prompt.md` Addition

The onboarding instructions (returned on first run to guide memory creation) get a brief addendum
after the 6 shared memories are created:

```
After creating the 6 shared memories, check if there are personal notes to capture.
Use write_memory(topic, content, private=true) for any local machine config or
developer-specific context. This is optional — skip if nothing personal applies yet.
```

---

## Implementation Touchpoints

| File | Change |
|---|---|
| `src/memory/mod.rs` | `MemoryStore::open_private(root)` method; auto-gitignore on first create |
| `src/agent.rs` | `ActiveProject` gets `private_memory: MemoryStore`; `Agent::new` + `Agent::activate` updated |
| `src/tools/memory.rs` | `private?: bool` param on all 4 tools; `list_memories` returns `{ shared, private }` |
| `src/tools/workflow.rs` | `build_system_prompt_draft()` adds Private Memory Rules section; onboarding status path lists private memories |
| `src/prompts/onboarding_prompt.md` | Brief addendum about optional private memory creation |

---

## Non-Goals

- No cross-project private memories (user-global store)
- No new tools (reuse existing 4 with `private` flag)
- No seeding of private memories during fresh onboarding (optional, LLM-driven)
- No migration of existing memories
