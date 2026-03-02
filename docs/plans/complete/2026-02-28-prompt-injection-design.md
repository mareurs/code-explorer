# Proposal: Verbatim Prompt Injection + Onboarding 7th Memory

**From:** claude-plugins (routing plugin team)
**Date:** 2026-02-28
**Status:** Discussion

---

## Context

This proposal comes from the `code-explorer-routing` plugin side. We have visibility into what the
plugin injects into agents, and we noticed two gaps that we believe are best fixed in coordination
with the code-explorer server — specifically in `onboarding_prompt.md` and how `system_prompt` is
surfaced to the plugin layer. We've tried to be specific about reasoning, but you know the
internals far better, and we genuinely want to debate the approach before anything is built.

---

## Problem 1: `system_prompt` doesn't reach subagents

`project.toml` already has a `system_prompt` field. `build_server_instructions()` appends it as
`## Custom Instructions` in the MCP server instructions. This is good — it reaches the main agent.

But MCP `server_instructions` are only sent once, during the MCP handshake with the main agent.
Subagents spawned via Claude Code's `Agent` tool get a fresh context: they re-run the MCP
handshake and receive `server_instructions`, so they *do* get the system_prompt via that path.

Wait — actually, we should verify: does Claude Code re-send `server_instructions` to each
subagent's MCP session, or do subagents inherit the parent's MCP connection? If each subagent
opens a new MCP session, then `system_prompt` already reaches them. If they share the parent's
session, they don't.

**If subagents do NOT get server_instructions re-sent:** The plugin's `SubagentStart` hook is the
only reliable injection point. We would read `system_prompt` from `project.toml` in the hook and
inject it verbatim. This requires the hook to parse TOML, which is slightly awkward in bash —
see "Implementation Thoughts" below.

**If subagents DO get server_instructions re-sent:** There's no gap. The only remaining issue
is that the plugin can't currently verify this at hook time, so it can't report the state in
its session-start output (where it says "DRIFT WARNING:", "INDEX:", etc.). We might want a
light signal like `CE_SYSTEM_PROMPT_SET=true` available to the hook for display purposes.

**Our assumption:** subagents share the parent MCP connection and do NOT get a fresh
`server_instructions`. If we're wrong, this whole problem disappears and only the display gap
remains. Please correct us here — you'll know immediately from the MCP handshake code.

---

## Problem 2: No navigation-focused memory at onboarding

The 6 onboarding memories cover the project well: what it is, how it's structured, conventions,
commands, vocabulary, gotchas. What they don't cover is *how to navigate this specific codebase
with code-explorer tools*.

Examples of what's missing:
- "Entry point is `src/main.rs:main()`. Start there for binary flow, `src/lib.rs` for the API."
- "Semantic search works well for queries like 'error handling' or 'config loading'. Avoid generic
  terms like 'data' — too broad for this codebase."
- "The `Tool` trait in `src/tools/mod.rs` is the core abstraction. Everything hangs off it."
- "Don't start with `list_dir(src/)` — it's 30 files. Use `list_symbols(src/lib.rs)` instead."

This is different from `architecture` (which describes what modules exist) and `conventions` (which
describes how code is written). It's navigation knowledge: given that you want to do X, here's
the fastest path using code-explorer tools.

We'd like to propose a 7th memory: `custom-code-explorer-prompt`, created at onboarding after the
codebase is explored. Unlike the other 6, this one would be injected **verbatim into the agent
context** at every session start and subagent start — not just listed as a reference hint.

The distinction matters: you want the AI to *have read* this before touching any tool, not to
decide whether to read it.

---

## Problem 3: Post-onboarding guidance gap

After onboarding creates memories, the interaction ends with a summary and a "does anything need
correction?" check. But users — especially new ones — often don't know:

1. That `system_prompt` in `.code-explorer/project.toml` exists and lets them inject global
   project rules into every AI session (not just code-explorer sessions)
2. That the `custom-code-explorer-prompt` memory they just got can be updated anytime with
   `write_memory("custom-code-explorer-prompt", ...)` or via the dashboard
3. Which 4-5 code-explorer tools to reach for first when starting a new task

A brief post-onboarding guide — appended to the onboarding prompt after the "Confirm with user"
step — would help adoption significantly. It doesn't need to be exhaustive; a 20-line cheat sheet
is more useful than a reference manual at that moment.

---

## Proposed Changes

### Plugin side (we'll handle this in claude-plugins)

**`session-start.sh` and `subagent-guidance.sh`:**
- Read `system_prompt` from `project.toml` (if set), inject verbatim before guidance.txt
- Read `.code-explorer/memories/custom-code-explorer-prompt.md` (if exists), inject verbatim
- Injection order (our current thinking — debatable):
  1. system_prompt (user's global project rules — frames everything)
  2. custom-code-explorer-prompt (navigation hints — project-specific)
  3. guidance.txt (tool routing rules — generic)
  4. Memory hints, drift warnings, etc.

**`detect-tools.sh`:**
- Add `CE_SYSTEM_PROMPT` and `CE_CUSTOM_PROMPT` variables for downstream hooks

### Server side (what we'd like your input on)

**`src/prompts/onboarding_prompt.md`:**

Two additions:

**Addition 1 — 7th memory template** (inserted before "Gathered Project Data"):

```markdown
### 7. `custom-code-explorer-prompt`

**What:** Navigation knowledge for this codebase — how to use code-explorer tools effectively
here, not just what the code contains.

**Template:**
```
# Custom Code Explorer Prompt

## Entry Points
[Where to start exploration. Specific files/symbols, not module descriptions.]
[e.g., "src/main.rs:main() for CLI flow, src/lib.rs for MCP tool API"]

## Semantic Search Tips
[Query patterns that work well for THIS codebase. Be concrete.]
[e.g., "Search 'tool registration' not 'tool' — too broad"]
[e.g., "Avoid querying for struct names directly — use find_symbol instead"]

## Key Abstractions
[3-5 types/traits that are the skeleton of this codebase. Name + file path.]
[These are what to understand first before anything else.]

## Navigation Strategy
[Recommended exploration order for a new task in this codebase.]
[e.g., "1. read_memory('architecture') → 2. list_symbols('src/tools/') → 3. find_symbol"]
```

**Anti-patterns:** Don't overlap with `architecture` (what exists) or `conventions` (how it's
written). Focus on *how to navigate* with code-explorer specifically.
```

**Addition 2 — Post-onboarding guidance section** (appended at the very end, after the
"Confirm with user" step):

```markdown
## After Memories Are Created

After confirming memories with the user, deliver this brief guide:

---

**Your code-explorer setup is complete. A few things worth knowing:**

**System prompt** — You can add project-wide rules for all AI sessions by editing
`.code-explorer/project.toml` and setting:
```toml
[project]
system_prompt = "Always use conventional commits. Never edit generated files in build/."
```
This injects into every session automatically. Use it for rules that apply regardless of task.

**Navigation memory** — The `custom-code-explorer-prompt` memory you just created guides AI
navigation of this codebase. Update it anytime as you learn what works:
- Via dashboard: open the Memories section, edit `custom-code-explorer-prompt`
- Via tool: `write_memory("custom-code-explorer-prompt", "...")`

**Starting a new task** — suggested opening move:
1. `read_memory("architecture")` — orient yourself
2. `list_symbols("src/")` — see the module structure
3. `semantic_search("your concept")` — find relevant code
4. `find_symbol("TargetName", include_body=true)` — read it

---
```

---

## Open Questions (We'd Appreciate Debate)

**Q1: Does `system_prompt` reach subagents via server_instructions re-send?**
If yes, Problem 1 is not a problem. We need a definitive answer on this before building anything
on the plugin side for system_prompt injection.

**Q2: Should `custom-code-explorer-prompt` be injected verbatim or just listed as a reference?**
Our argument for verbatim: it's short, high-value, and the AI should have it *before* any tool
call — not discover it mid-task. Counter-argument: it adds context tokens every session even for
tasks where it's not useful (e.g., writing a commit message). Does code-explorer have a mechanism
for conditional injection (only inject if a code-exploration tool is the first tool called)?

**Q3: Injection order?**
We proposed: system_prompt → custom-code-explorer-prompt → guidance.txt. The reasoning: user's
rules should frame the session before generic tool routing. But there's an argument for guidance.txt
first (establish tool rules, then project context). What does the server currently do in
`build_server_instructions()`? It appends system_prompt *after* the standard instructions. Should
the plugin follow the same convention for consistency?

**Q4: Content overlap between `custom-code-explorer-prompt` and `architecture`?**
The 7th memory template tries to scope to "navigation" vs "structure", but in practice an AI
following the onboarding prompt might duplicate entry points between `architecture` and
`custom-code-explorer-prompt`. Is there a way to make the onboarding prompt enforce this
boundary more firmly? Or should `architecture` be updated to *not* include entry points,
deferring them to the 7th memory?

**Q5: Size limits?**
If `custom-code-explorer-prompt` gets verbose (a user adds a lot of project-specific guidance),
injecting it verbatim into every subagent becomes expensive. Should there be a soft size warning
in the memory tool when writing to this specific topic? Or a hard limit? We don't know if
this is a real concern in practice.

**Q6: Dashboard editability of `system_prompt`?**
Currently `system_prompt` is in `project.toml`. The dashboard has a full memory CRUD API. Should
`system_prompt` be surfaced in the dashboard as a special editable field in a "Config" section,
or should it move to `memories/system_prompt.md` to use the existing memory UI? We lean toward
keeping it in `project.toml` (it's a config value, not a knowledge artifact), but the dashboard
doesn't currently expose it for editing. A simple text area in the Config tab for `system_prompt`
would make the "edit in dashboard" workflow possible without changing the data model.

---

## Implementation Thoughts (Take or Leave)

**Reading `system_prompt` from TOML in bash** is the awkward part for the plugin. Options:
- `python3 -c "import tomllib; ..."` — works on Python 3.11+, may not be universally available
- `grep`/`sed` heuristic — fragile for multiline strings
- Expose a `code-explorer config get project.system_prompt` CLI subcommand — clean, no deps,
  and generally useful for other integrations. This is our preferred option if you're open to it.

**The 7th memory** could alternatively be generated by the `onboarding` tool itself (not the
prompt) — after calling `write_memory` for the 6 standard memories, the tool could call a
`generate_navigation_hints()` function that introspects the project structure. That would be
more consistent output but less AI-generated nuance. Probably not worth the complexity.

**Post-onboarding guidance** is purely a prompt change — low risk, easy to iterate. If the
copy isn't right, update the prompt. We'd happily iterate on the wording if you want to keep
the guidance voice consistent with the rest of the manual.
