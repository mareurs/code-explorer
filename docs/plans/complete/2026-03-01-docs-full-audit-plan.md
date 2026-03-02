# Documentation Full Audit — Structure, Gaps, and Cross-Links

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix every structural, content, and cross-linking gap in the mdBook manual and supporting docs, leaving readers able to navigate naturally from any entry point.

**Architecture:** Pure documentation — no code changes. All changes are in `docs/manual/src/` and `docs/`. Verification for each task is "open the relevant files and confirm the links point to real targets and the wording is accurate." No build step required, but `mdbook build docs/manual` can catch broken internal links.

**Tech Stack:** Markdown, mdBook SUMMARY.md, relative link paths.

---

## Audit Findings Summary

### A. SUMMARY.md — structural lie
Every concept page (Tool Selection, Output Modes, Dashboard, Output Buffers, Shell Integration, Worktrees, Routing Plugin, Superpowers) is nested under **Progressive Disclosure** in the TOC. That's semantically wrong — none of these are sub-topics of Progressive Disclosure. Readers browsing the TOC see "Dashboard" as a sub-point of "Progressive Disclosure" and get a false picture of the tool.

### B. Getting-started — no next-step navigation
The three getting-started pages have no "next →" links at the bottom. A reader who finishes Installation has no indication that "Your First Project" comes next; a reader who finishes "Your First Project" doesn't know the Routing Plugin exists.

### C. Content bugs in first-project.md
- "Sample output (Rust project):" block appears twice (one is empty, one has real content)
- "navigate deeper into subsystems." is duplicated on consecutive lines

### D. Tool count inconsistency
`installation.md` says **33 tools**; `tools/overview.md` and ROADMAP both say **31**.

### E. Missing concept pages
- **Library Navigation** — only a tool reference exists. No concept page explains auto-discovery, the scope parameter semantics, or when you'd want to navigate library source. Every other major feature (Semantic Search, Memory, Progressive Disclosure, Dashboard) has a concept page.
- **Memory** — `tools/memory.md` covers the tools exhaustively but there is no concept page explaining *what* persistent memory is and why you'd use it, at the level of "Semantic Search" or "Progressive Disclosure" concept pages.

### F. Missing cross-links — concept ↔ concept
Every concept page should point to related concepts. Currently none do.

| Page | Missing links |
|---|---|
| `concepts/progressive-disclosure.md` | → Output Modes, → Tool Selection |
| `concepts/tool-selection.md` | → Progressive Disclosure, → Semantic Search |
| `concepts/output-modes.md` | → Progressive Disclosure |
| `concepts/output-buffers.md` | → Shell Integration, → run_command tool ref |
| `concepts/shell-integration.md` | → Output Buffers, → run_command tool ref |
| `concepts/worktrees.md` | → Superpowers, → activate_project tool ref |
| `concepts/superpowers.md` | → Worktrees, → Routing Plugin |
| `concepts/routing-plugin.md` | → Superpowers |

### G. Missing cross-links — tool refs ↔ concept pages
| Tool page | Missing links |
|---|---|
| `tools/symbol-navigation.md` | → concepts/tool-selection.md, → concepts/progressive-disclosure.md |
| `tools/file-operations.md` | → concepts/output-buffers.md (read_file buffering) |
| `tools/editing.md` | → concepts/worktrees.md (worktree write guard) |
| `tools/library-navigation.md` | → concepts/library-navigation.md (once created) |
| `tools/git.md` | → concepts/progressive-disclosure.md (git_blame uses PD) |
| `tools/ast.md` | → concepts/tool-selection.md |

### H. Getting-started ↔ concepts cross-links missing
- `first-project.md` — no link to concepts at end (after the workflow section)
- `getting-started/routing-plugin.md` — no link to `concepts/routing-plugin.md` for deeper explanation

### I. Routing plugin command inconsistency
`getting-started/routing-plugin.md` shows a one-step install command; `concepts/routing-plugin.md` shows a two-step command (marketplace add, then install). These should match.

### J. ROADMAP staleness
- "Companion Claude Code plugin: code-explorer-routing" is listed under **What's Next** but the page itself says it's already live
- Contributor Skills table has `log-stat-analyzer` as "Blocked on Tool Usage Monitor" — but `get_usage_stats` is implemented and shown in ROADMAP's "What's Built" section

---

## Tasks

### Task 1: Fix SUMMARY.md concept nesting

**File:** `docs/manual/src/SUMMARY.md`

The concept pages should not all be sub-items of Progressive Disclosure. Break them into logical clusters with `Progressive Disclosure` only owning `Output Modes` and `Tool Selection` as direct children.

**Before:**
```
- [Progressive Disclosure](concepts/progressive-disclosure.md)
  - [Tool Selection](concepts/tool-selection.md)
  - [Output Modes](concepts/output-modes.md)
  - [Dashboard](concepts/dashboard.md)
  - [Output Buffers](concepts/output-buffers.md)
  - [Shell Integration](concepts/shell-integration.md)
  - [Git Worktrees](concepts/worktrees.md)
  - [Routing Plugin](concepts/routing-plugin.md)
  - [Superpowers Workflow](concepts/superpowers.md)
```

**After:**
```
- [Progressive Disclosure](concepts/progressive-disclosure.md)
  - [Output Modes](concepts/output-modes.md)
  - [Tool Selection](concepts/tool-selection.md)

- [Shell Integration](concepts/shell-integration.md)
  - [Output Buffers](concepts/output-buffers.md)

- [Dashboard](concepts/dashboard.md)
- [Git Worktrees](concepts/worktrees.md)

- [Routing Plugin](concepts/routing-plugin.md)
  - [Superpowers Workflow](concepts/superpowers.md)
```

Also add `- [Library Navigation](concepts/library-navigation.md)` and `- [Memory](concepts/memory.md)` (new pages created in Tasks 5 and 6) under the Semantic Search section or as standalone peers.

**Verify:** Open SUMMARY.md and confirm nesting makes conceptual sense. Ensure all linked files exist.

---

### Task 2: Fix content bugs in first-project.md

**File:** `docs/manual/src/getting-started/first-project.md`

**Bug 1:** Duplicate "Sample output (Rust project):" section at lines ~110-115. There's an empty fenced code block followed immediately by a second "Sample output" heading + a real code block. Delete the first (empty) occurrence.

Current text to find and remove:
```
Sample output (Rust project):

```
```

Sample output (Rust project):
```

Remove the first occurrence plus its empty code block, keeping only the second with the real content.

**Bug 2:** Duplicate "navigate deeper into subsystems." — two consecutive identical lines at end of step 6 in the workflow. Delete the duplicate.

**Verify:** Re-read the file and confirm there is exactly one "Sample output" block and no duplicate lines.

---

### Task 3: Fix tool count in installation.md + add next-step link

**File:** `docs/manual/src/getting-started/installation.md`

**Fix 1:** Line that says "33 tools" — change to "31 tools" (consistent with tools/overview.md and ROADMAP).

**Fix 2:** Add a "Next Steps" section at the bottom of the file:

```markdown
## Next Steps

- [Your First Project](first-project.md) — open a project, run onboarding, and try the basic tools
- [Routing Plugin](routing-plugin.md) — install the plugin that steers Claude toward code-explorer tools automatically
```

**Verify:** Confirm "31 tools" appears, and both linked files exist.

---

### Task 4: Add next-step navigation to first-project.md and routing-plugin.md

**Files:**
- `docs/manual/src/getting-started/first-project.md`
- `docs/manual/src/getting-started/routing-plugin.md`

**first-project.md:** Add at the end, after the last paragraph about `.gitignore`:

```markdown
## Next Steps

- [Routing Plugin](routing-plugin.md) — install the plugin that ensures subagents also use code-explorer
- [Tool Selection](../concepts/tool-selection.md) — when to use symbol tools vs semantic search vs text search
- [Progressive Disclosure](../concepts/progressive-disclosure.md) — how tools manage output size automatically
```

**getting-started/routing-plugin.md:** Add at the end:

```markdown
## Further Reading

- [Routing Plugin (concepts)](../concepts/routing-plugin.md) — how the plugin works, why hard blocks beat soft warnings, the subagent coverage problem
```

**Verify:** Confirm links point to files that exist.

---

### Task 5: Create concepts/library-navigation.md

**File:** Create `docs/manual/src/concepts/library-navigation.md`

Content:

```markdown
# Library Navigation

Library navigation lets you explore third-party dependency source code using the
same symbol tools you use for your own project — `find_symbol`, `list_symbols`,
`goto_definition`, `semantic_search` — without switching contexts or manually
locating package directories.

## Auto-Discovery

Libraries are discovered automatically. When you call `goto_definition` on a
symbol and the LSP resolves it to a path *outside the project root* (typically
inside a language package cache), code-explorer registers that path as a library
and names it by the package name inferred from the manifest it finds there.

The next time you call `list_libraries`, the dependency appears in the list.
No manual registration is required for the common case.

## The Scope Parameter

Once a library is registered, pass `scope` to any navigation or search tool to
target it:

| Value | Searches |
|---|---|
| `"project"` (default) | Only your project's source code |
| `"lib:<name>"` | A specific registered library (e.g. `"lib:tokio"`) |
| `"libraries"` | All registered libraries combined |
| `"all"` | Your project + all registered libraries |

```json
{
  "tool": "semantic_search",
  "arguments": { "query": "retry with backoff", "scope": "lib:reqwest" }
}
```

Results include a `"source"` field so you can tell project code from library
code at a glance.

## Building a Library Index

Semantic search over library code requires an embedding index, just like project
code. Build one with `index_library`:

```json
{ "tool": "index_library", "arguments": { "name": "tokio" } }
```

This is a one-time cost per library. The index persists in
`.code-explorer/libraries/<name>/embeddings.db`.

## When to Use Library Navigation

- You're debugging an unfamiliar error from a dependency and want to read its
  source without leaving your session
- You want to understand how a library's internal types relate before writing
  integration code
- You're doing a security audit and want to trace a call chain into a dependency
- You want to find usage examples by searching the library's own tests with
  `semantic_search(scope: "lib:<name>")`

## Further Reading

- [Library Navigation Tools](../tools/library-navigation.md) — full reference for
  `list_libraries` and `index_library`
- [Symbol Navigation Tools](../tools/symbol-navigation.md) — the tools that accept
  the `scope` parameter
- [Semantic Search Tools](../tools/semantic-search.md) — semantic search within
  library scope
```

**Verify:** File exists and all relative links point to existing files.

---

### Task 6: Create concepts/memory.md

**File:** Create `docs/manual/src/concepts/memory.md`

Content:

```markdown
# Memory

Memory gives code-explorer persistent, project-scoped storage that outlives any
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
```

**Verify:** File exists and all relative links point to existing files.

---

### Task 7: Update SUMMARY.md to include new concept pages

**File:** `docs/manual/src/SUMMARY.md`

After completing Tasks 1, 5, and 6, update the User Guide section to include:
- `concepts/library-navigation.md` — add as standalone item near Library Navigation tool link
- `concepts/memory.md` — add near Memory tool link or as standalone

Full target SUMMARY.md User Guide section:

```
# User Guide

- [Installation](getting-started/installation.md)
  - [Your First Project](getting-started/first-project.md)
  - [Routing Plugin](getting-started/routing-plugin.md)

- [Progressive Disclosure](concepts/progressive-disclosure.md)
  - [Output Modes](concepts/output-modes.md)
  - [Tool Selection](concepts/tool-selection.md)

- [Shell Integration](concepts/shell-integration.md)
  - [Output Buffers](concepts/output-buffers.md)

- [Semantic Search](concepts/semantic-search.md)
  - [Setup Guide](semantic-search-guide.md)

- [Library Navigation](concepts/library-navigation.md)

- [Memory](concepts/memory.md)

- [Dashboard](concepts/dashboard.md)
- [Git Worktrees](concepts/worktrees.md)

- [Routing Plugin](concepts/routing-plugin.md)
  - [Superpowers Workflow](concepts/superpowers.md)

- [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Language Support](language-support.md)
```

**Verify:** All paths correspond to real files. Nesting is semantically correct.

---

### Task 8: Add Further Reading to progressive-disclosure.md, output-modes.md, tool-selection.md

**Files:**
- `docs/manual/src/concepts/progressive-disclosure.md`
- `docs/manual/src/concepts/output-modes.md`
- `docs/manual/src/concepts/tool-selection.md`

**progressive-disclosure.md** — add at the end:

```markdown
## Further Reading

- [Output Modes](output-modes.md) — the `detail_level`, `offset`, and `limit`
  parameters in full detail, with examples for every tool
- [Tool Selection](tool-selection.md) — matching your level of knowledge to the
  right tool, including the anti-patterns that cause context bloat
```

**output-modes.md** — add at the end:

```markdown
## Further Reading

- [Progressive Disclosure](progressive-disclosure.md) — the design principle
  behind the two-mode system and how `OutputGuard` enforces it
- [Symbol Navigation Tools](../tools/symbol-navigation.md) — the tools where
  `detail_level` has the most impact
```

**tool-selection.md** — add at the end:

```markdown
## Further Reading

- [Progressive Disclosure](progressive-disclosure.md) — how output volume is
  controlled once you've selected the right tool
- [Semantic Search](semantic-search.md) — deeper explanation of when and how
  semantic search finds code you can't name
```

**Verify:** All links point to real files. Read the added sections back and confirm they read naturally.

---

### Task 9: Add Further Reading to output-buffers.md and shell-integration.md

**Files:**
- `docs/manual/src/concepts/output-buffers.md`
- `docs/manual/src/concepts/shell-integration.md`

**output-buffers.md** — add at the end:

```markdown
## Further Reading

- [Shell Integration](shell-integration.md) — `run_command` in full detail:
  safety layer, dangerous command detection, and source file access blocking
- [Workflow & Config Tools](../tools/workflow-and-config.md) — full reference
  for `run_command` including the `cwd`, `acknowledge_risk`, and `timeout_secs`
  parameters
```

**shell-integration.md** — add at the end:

```markdown
## Further Reading

- [Output Buffers](output-buffers.md) — how large command output is stored and
  queried with `@cmd_id` refs rather than dumped into context
- [Workflow & Config Tools](../tools/workflow-and-config.md) — full `run_command`
  reference including all parameters
```

**Verify:** Links point to real files.

---

### Task 10: Add Further Reading to worktrees.md and superpowers.md

**Files:**
- `docs/manual/src/concepts/worktrees.md`
- `docs/manual/src/concepts/superpowers.md`

**worktrees.md** — add at the end:

```markdown
## Further Reading

- [Superpowers Workflow](superpowers.md) — how the Superpowers plugin integrates
  worktrees into a full TDD + parallel-agent development workflow
- [Workflow & Config Tools](../tools/workflow-and-config.md) — `activate_project`
  reference: the required call after entering a worktree
```

**superpowers.md** — add at the end:

```markdown
## Further Reading

- [Git Worktrees](worktrees.md) — the three-layer protection system (write guard,
  worktree hint, navigation exclusions) that prevents silent cross-worktree edits
- [Routing Plugin](routing-plugin.md) — how the plugin's `worktree-activate.sh`
  hook auto-calls `activate_project` when `EnterWorktree` fires
```

**Verify:** Links point to real files.

---

### Task 11: Add See-also to routing-plugin concept page

**File:** `docs/manual/src/concepts/routing-plugin.md`

The page already ends with an installation code block linking to the setup guide. Add a `## Further Reading` section after it:

```markdown
## Further Reading

- [Routing Plugin Setup Guide](../getting-started/routing-plugin.md) — installation
  steps, configuration options, and verification
- [Superpowers Workflow](superpowers.md) — the Superpowers plugin that pairs
  with the routing plugin for full lifecycle development
```

**Verify:** Links are correct.

---

### Task 12: Add cross-links from tool reference pages to concept pages

**Files:** 6 tool reference pages

**tools/symbol-navigation.md** — add "See also" note at the top (after the opening paragraph, before first `---`):

```markdown
> **See also:** [Tool Selection](../concepts/tool-selection.md) — when to reach
> for symbol tools vs semantic search vs text search. [Progressive Disclosure](../concepts/progressive-disclosure.md) — how `detail_level` controls output volume for these tools.
```

**tools/file-operations.md** — add "See also" note near the `read_file` section (after its description, before its parameter table):

```markdown
> **See also:** [Output Buffers](../concepts/output-buffers.md) — how large
> file reads are stored as `@file_id` refs rather than dumped into context.
```

**tools/editing.md** — add "See also" note at the top:

```markdown
> **See also:** [Git Worktrees](../concepts/worktrees.md) — the worktree write
> guard that protects against silent edits to the wrong repository tree.
```

**tools/library-navigation.md** — add "See also" note at the top:

```markdown
> **See also:** [Library Navigation](../concepts/library-navigation.md) — how
> auto-discovery works, the scope parameter, and when to navigate library source.
```

**tools/git.md** — add "See also" note at the top:

```markdown
> **See also:** [Progressive Disclosure](../concepts/progressive-disclosure.md) —
> `git_blame` in Exploring mode returns the first 200 lines; use Focused mode
> with `offset`/`limit` for longer files.
```

**tools/ast.md** — add "See also" note at the top:

```markdown
> **See also:** [Tool Selection](../concepts/tool-selection.md) — when to use
> AST tools (`list_functions`, `list_docs`) vs LSP-backed symbol tools.
```

**Verify:** All `../concepts/` paths resolve to files that exist.

---

### Task 13: Fix routing plugin install command inconsistency

**Files:**
- `docs/manual/src/getting-started/routing-plugin.md`
- `docs/manual/src/concepts/routing-plugin.md`

The getting-started page shows a one-step `claude /plugin install` command. The concept page shows a two-step flow (marketplace add, then install).

Read both pages and align them: use the two-step form in both, since `marketplace add` is needed first to make the plugin discoverable.

**getting-started/routing-plugin.md** — update Option 1 installation block to match the two-step form:

```bash
/plugin marketplace add mareurs/sdd-misc-plugins
/plugin install code-explorer-routing@sdd-misc-plugins
```

**Verify:** Both pages show the same installation commands.

---

### Task 14: Fix ROADMAP.md stale entries

**File:** `docs/ROADMAP.md`

**Fix 1:** Under "What's Next", remove this bullet since it's already live:
```
- Companion Claude Code plugin: `code-explorer-routing` (live at [mareurs/claude-plugins](https://github.com/mareurs/claude-plugins))
```
Move it to "What's Built" instead.

**Fix 2:** In the Contributor Skills table, `log-stat-analyzer` row:
- Change Status from `Blocked on Tool Usage Monitor` → `Ready` (since `get_usage_stats` is implemented as shown in What's Built)

**Fix 3:** Add link to the Dashboard concept page in the "What's Built" Dashboard bullet:
```markdown
- **Dashboard** — `code-explorer dashboard` web UI with tool stats and project health ([concept page](manual/src/concepts/dashboard.md))
```

**Verify:** Stale entries are gone, moved items are in the correct section.

---

### Task 15: Add next-step link from installation.md to first-project.md and routing-plugin.md

**File:** `docs/manual/src/getting-started/installation.md`

_(Already described in Task 3 — if Task 3 was executed, skip the next-step section portion here.)_

Also add a "See also" note near the Feature Flags section linking to the Embedding Backends config page:

```markdown
> **See also:** [Embedding Backends](../configuration/embedding-backends.md) —
> full backend comparison, recommended models, and per-backend configuration.
```

**Verify:** All links resolve correctly.

---

### Task 16: Add cross-links from introduction.md to getting-started

**File:** `docs/manual/src/introduction.md`

Read the end of the introduction page. Add a clear "Get Started" section just before the last section (or at the end):

```markdown
## Get Started

- [Installation](getting-started/installation.md) — build the binary, register
  the MCP server, install LSP servers
- [Your First Project](getting-started/first-project.md) — onboarding, indexing,
  and your first tool calls
- [Routing Plugin](getting-started/routing-plugin.md) — the plugin that ensures
  Claude always reaches for code-explorer tools
```

**Verify:** Links resolve, section reads naturally.

---

### Task 17: Commit

All changes are pure documentation. Batch all the above into a single well-described commit:

```bash
git add docs/
git commit -m "docs: full audit — structure, cross-links, content fixes, new concept pages

- Fix SUMMARY.md: denest concepts from Progressive Disclosure; add Library
  Navigation and Memory concept pages to TOC
- Create concepts/library-navigation.md and concepts/memory.md
- Fix first-project.md: remove duplicate sample output block and duplicate line
- Fix installation.md: correct tool count (33 → 31), add next-step links
- Add Further Reading to all concept pages (progressive-disclosure, output-modes,
  tool-selection, output-buffers, shell-integration, worktrees, superpowers,
  routing-plugin)
- Add See-also notes to tool reference pages (symbol-navigation, file-operations,
  editing, library-navigation, git, ast)
- Add getting-started next-step navigation links
- Fix routing plugin install command inconsistency
- Fix ROADMAP.md stale entries (routing plugin, log-stat-analyzer status)"
```

---

## Execution Order

Tasks with no dependencies can run in parallel. Suggested order:

1. **Task 2** (content bug fix — isolated, no deps)
2. **Task 3** (tool count fix — isolated)
3. **Tasks 5 + 6** in parallel (new concept pages — independent of each other)
4. **Tasks 1 + 7** together (SUMMARY.md — depends on Tasks 5+6 being created)
5. **Tasks 8 + 9 + 10 + 11** in parallel (concept page Further Reading — independent)
6. **Task 12** (tool ref cross-links — depends on concept pages existing)
7. **Tasks 4 + 13 + 14 + 15 + 16** in parallel (getting-started + ROADMAP — independent)
8. **Task 17** (commit — depends on everything)

Total estimated time: ~45–60 minutes of focused editing.
