# Design: System Prompt via `.codescout/system-prompt.md`

**Date:** 2026-02-28
**Status:** Approved

---

## Problem

code-explorer has a `system_prompt` field in `project.toml` that gets appended to
MCP server instructions as `## Custom Instructions`. But:

1. **It's never scaffolded** — users don't know it exists
2. **Inline TOML is awkward** for multi-line content
3. **No navigation guidance** — onboarding creates 6 reference memories but nothing
   about *how to navigate this specific codebase with code-explorer tools*
4. **Plugin can't read it easily** — parsing TOML from bash hooks is fragile

## Solution

A single file — `.codescout/system-prompt.md` — replaces the TOML field. It is:

- **Auto-generated** at onboarding (draft + user confirmation)
- **Project-specific** code exploration guidance (entry points, key abstractions,
  search tips, navigation strategy, project rules)
- **Always injected** into MCP server instructions as `## Custom Instructions`
- **Easy to read** from the plugin (just `cat` the file)

## Design Decisions

### One file, not two

Early exploration considered separate prompts: a "system prompt" (user rules) and a
"custom code-explorer prompt" (navigation hints). Rejected as unnecessary complexity
— both serve the same purpose: project-specific guidance for code exploration. One
file covers both.

### File-based, not TOML

The TOML `system_prompt` field is deprecated (with fallback). A markdown file is:
- Friendlier for multi-line content
- Directly readable by the routing plugin (no TOML parsing)
- Editable in any text editor or dashboard

### Not a memory

The system prompt is always injected (part of MCP server instructions). Memories are
read on demand. Different lifecycle, different injection point. Using a memory would
conflate the two concepts.

### Draft + confirm, not auto-generate

Onboarding presents a draft system prompt to the user for confirmation before saving.
This ensures quality and gives the user a chance to add project-specific rules the AI
couldn't discover.

## Storage

**Canonical:** `.codescout/system-prompt.md`

**Migration:** `build_server_instructions()` checks for the file first. If absent,
falls back to `project.toml`'s `system_prompt` field. If both exist, file wins.
The TOML field is deprecated in docs but not removed.

## Content Guidelines

The system prompt should be **15-30 lines** and complement — never repeat — the
static tool guidance in `server_instructions.md`.

### What goes in:

- **Entry points** — where to start exploring (`src/lib.rs` for API, `src/main.rs`
  for CLI)
- **Key abstractions** — 3-5 types/traits that are the codebase skeleton
- **Search tips** — queries that work well for THIS codebase, queries to avoid
- **Navigation strategy** — recommended exploration order for new tasks
- **Project rules** — conventions not covered by linters

### What does NOT go in:

- How to use code-explorer tools (static `server_instructions.md`)
- Full architecture details (`architecture` memory)
- Domain glossary, commands, conventions (respective memories)
- Anything longer than ~30 lines (keep it concise)

## Onboarding Flow

1. Gather context (existing, unchanged)
2. Create 6 memories (existing, unchanged)
3. **New:** Synthesize a system prompt draft from discovered context
4. **New:** Present draft to user for confirmation
5. **New:** After confirmation, write to `.codescout/system-prompt.md`
6. **New:** Deliver post-onboarding cheat sheet

### Onboarding Prompt Addition

New section in `onboarding_prompt.md`, after the 6 memory templates:

```markdown
### 7. System Prompt — `.codescout/system-prompt.md`

After creating the 6 memories, synthesize a concise system prompt (15-30 lines)
for this project. This prompt is injected into EVERY session automatically —
it must be short and high-value.

**What to include:**
- Entry points: where to start exploring (specific files + symbols)
- Key abstractions: 3-5 core types that are the skeleton of this codebase
- Search tips: queries that work well, queries to avoid
- Navigation strategy: recommended exploration order for new tasks
- Project rules: conventions the AI should always follow

**What NOT to include (these are already covered elsewhere):**
- How code-explorer tools work (static tool guidance handles this)
- Full architecture details (the `architecture` memory handles this)
- Command lists, glossary, conventions details (memories handle these)

**Format:** Present the draft to the user and ask for confirmation before
writing. After confirmation, write to `.codescout/system-prompt.md`.
```

### Post-Onboarding Guide

After all memories + system prompt are confirmed, deliver:

```
Your code-explorer setup is complete.

- **System prompt** (.codescout/system-prompt.md) — always-on project
  guidance. Edit it anytime to refine how the AI navigates your codebase.
- **Memories** — reference material read on demand. Update with
  write_memory(topic, content).
- **Starting a task** — read_memory("architecture") → list_symbols("src/")
  → semantic_search("your concept") → find_symbol("Name", include_body=true)
```

## Runtime Reading

In `build_server_instructions()` (or its caller):

```
1. Check for .codescout/system-prompt.md → read contents
2. If not found, check config.project.system_prompt (TOML fallback)
3. If content found, append as "## Custom Instructions"
```

The `ProjectStatus` struct already has `system_prompt: Option<String>`. The caller
in `agent.rs` reads the file and populates this field.

## Changes Required

| Component | Change |
|-----------|--------|
| `src/prompts/onboarding_prompt.md` | Add system prompt template + post-onboarding guide |
| `src/prompts/mod.rs` | Read `.codescout/system-prompt.md` in `build_server_instructions()` or accept it from caller |
| `src/agent.rs` | `project_status()` reads system-prompt.md, falls back to TOML field |
| `src/tools/workflow.rs` | Onboarding returns `system_prompt_draft` in JSON output |
| `src/config/project.rs` | Document `system_prompt` field as deprecated |
| Plugin `session-start.sh` | Read `.codescout/system-prompt.md` for subagent injection (plugin team) |

## What We're NOT Doing

- No second prompt type — one file covers rules + navigation
- No 7th memory — system prompt is a file, not a memory
- No new tools — `create_file` / `read_file` handle the file
- No dashboard changes yet — file-based editing is sufficient
- Not removing TOML field — graceful deprecation with fallback

## Open Questions for Routing Team

Responding to the routing team's proposal (`docs/plans/2026-02-28-prompt-injection-design.md`):

- **Q1 (subagent MCP sessions):** Each subagent gets a fresh MCP handshake and
  receives `server_instructions`. The system prompt already reaches them via this
  path. The plugin's subagent hook can additionally inject the file for redundancy.
- **Q2 (verbatim injection):** Yes — the system prompt is always injected via MCP
  server instructions. No conditional logic needed.
- **Q3 (injection order):** The MCP server appends custom instructions after static
  guidance. The plugin should follow the same convention for consistency.
- **Q4 (content overlap):** Addressed by clear content guidelines — system prompt
  covers navigation, memories cover reference material.
- **Q5 (size limits):** 30-line soft guideline enforced by onboarding prompt.
  No hard limit in code.
- **Q6 (dashboard):** File-based editing is sufficient. Dashboard can add a text
  area later if needed.
