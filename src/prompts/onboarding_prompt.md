You have just onboarded this project. Your job is to create 6 memories that give future AI sessions deep, accurate knowledge of this codebase — not a surface-level summary of its README.

## ⛔ Phase 1: Explore the Code — REQUIRED BEFORE WRITING ANY MEMORIES

The gathered data below (README, build config, CLAUDE.md) is a **starting point, not a substitute for exploration**. Memories written from documentation alone are shallow and frequently wrong. Every future session will rely on what you write now.

**Do NOT call `memory(action: "write", ...)` until you have completed the steps below and written the exploration summary.**

### Exploration Steps

Run these in order. They apply to any project.

1. **Map the structure** — Review the detected top-level structure or `list_dir(".")`. Identify: where is source code? tests? architecture docs?

2. **Find key abstractions** — `list_symbols` on the main source directory (e.g. `list_symbols("src/")`). Identify the 3–5 types/traits/classes that appear central.

3. **Read core implementations** — `find_symbol("CoreType", include_body=true)` for 2–3 of those abstractions. Understand how they actually work, not just what they're named.

4. **Trace one data flow** — Follow a representative path through the system (a request, command, event). Use `find_references`, `goto_definition`, or `semantic_search("how X flows")`.

5. **Read architecture docs** — If README or CLAUDE.md references any `docs/ARCHITECTURE.md`, design docs, or ADRs, `read_file` them now.

### Exploration Summary (write this before proceeding)

After completing steps 1–5, write 4–6 sentences covering:
- What this system does (your own words, not the README's)
- The 3–5 most important types/modules and their roles
- How a typical operation flows through the system

**This summary is your gate to Phase 2. If you cannot write it from memory after exploring, you haven't explored enough.**

---

## Phase 2: Write the 6 Memories

Now write the memories. Your Phase 1 exploration should inform every memory — especially `architecture` and `conventions`, which cannot be written accurately from documentation alone.

### Rules

1. **Do NOT duplicate auto-loaded context** — CLAUDE.md, project README, and referenced docs are already available every session. Memories must *supplement* them, not repeat them. If something is already documented, write a pointer (`see CLAUDE.md § Key Patterns`) rather than copying it.
2. **References over copies — drift is real** — Code and docs change. A memory that copies a code snippet or lists tool names will go stale silently and actively mislead future sessions. Prefer: `"see docs/ARCHITECTURE.md for the layer diagram"` over pasting the diagram. Reserve inline content for things that are NOT documented elsewhere.
3. **Memories capture gaps, not summaries** — Ask: "Would a future AI session know this from CLAUDE.md, the README, or the referenced docs?" If yes, skip it or point to the source. Only write it if the answer is no.
4. **Be specific where you do write** — Include file paths, exact command names, concrete patterns. "Uses clean architecture" is useless. "`api/ → service/ → repository/` with interface+impl pattern in `src/`" is useful.
5. **Be concise** — Each memory should be 15–40 lines. Longer means too much detail or duplication.
6. **Confirm with the user** — After creating all 6 memories, summarize what you wrote and ask if anything needs correction.
7. **Private memories** — Use `memory(action: "write", topic: ..., content: ..., private: true)` for project-local notes that should not appear in system instructions (e.g. personal debugging notes, temporary state). Standard `memory(action: "write", ...)` creates shared memories visible to all agents.

### Memories to Create

### 1. `project-overview`

**What:** Project purpose, tech stack, key dependencies, runtime requirements.

**Template:**
```
# [Project Name]

## Purpose
[1-2 sentences: what does this project do and who is it for?]

## Tech Stack
- **Language:** [lang] [version if known]
- **Framework:** [framework] [version]
- **Database:** [if any]
- **Key deps:** [3-5 most important dependencies]

## Runtime Requirements
[What's needed to run: Node 20+, Java 21+, Docker, specific env vars, etc.]
```

**Anti-patterns:** Don't copy the README or CLAUDE.md. Don't list every dependency — just the ones not obvious from the build file. Don't include directory listings (CLAUDE.md already has the src/ tree). Focus on what's missing from those sources: runtime env requirements, non-obvious feature flags, external service dependencies.

---

### 2. `architecture`

**What:** Module structure, key abstractions with file locations, data flow, design patterns, entry points.

**Template:**
```
# Architecture

## Layer Structure
[Main modules/layers and their responsibilities]
[Include file paths: `src/services/` → business logic]

## Key Abstractions
[3-5 most important types/traits/interfaces]
[Name + file path for each]

## Data Flow
[How a typical request flows through the system]
[Entry point → layer 1 → layer 2 → output]

## Design Patterns
[Only patterns actually in use: DI, repository, event-driven, etc.]

## Invariants
[Hard rules — for each candidate ask: "what *concretely* breaks if this is ignored?"]
[If the failure mode is vague, it belongs in Strong Defaults, not here]
[Keep to ~5 entries max — if everything is an invariant, nothing is]

| Rule | Why it exists |
|---|---|
| [rule] | [specific failure if broken] |

## Strong Defaults
[Preferred behaviors that CAN be overridden with deliberate reason]

| Default | When it's okay to break it |
|---|---|
| [default behavior] | [specific condition that justifies breaking it] |
```

**Anti-patterns:** Don't repeat what CLAUDE.md's "Project Structure" or "Key Patterns" sections already say — they're loaded every session. Don't copy layer diagrams from `docs/ARCHITECTURE.md`; reference them instead (`see docs/ARCHITECTURE.md`). Focus on what's NOT in those docs: internal struct shapes, concrete data flow with actual function/method names, non-obvious wiring. Inline content here goes stale as code evolves — keep it minimal and specific.

**Invariants / Strong Defaults:** Don't lift every rule from CLAUDE.md into Invariants — only the ones an agent would realistically violate. If there's no specific observable failure mode, it belongs in Strong Defaults. Every Strong Default must include its override condition — a default with no escape hatch is just an invariant written poorly.

---

### 3. `conventions`

**What:** Code style, naming conventions, error handling, testing patterns.

**Template:**
```
# Conventions

## Naming
[Table: entity type → convention → example]

## Patterns
[Key patterns: error handling, DI, async, testing]
[Short code examples where helpful]

## Code Quality
[Linter, formatter, type checker — exact commands]

## Testing
[Framework, organization, how to write a new test]
```

**Anti-patterns:** Don't repeat CLAUDE.md's "Design Principles" section (progressive disclosure, no echo, two modes, RecoverableError) — it's already loaded. Don't copy the "Prompt Surface Consistency" or "Testing Patterns" sections either. Reference them: `"see CLAUDE.md § Design Principles"`. Write only conventions that are absent from CLAUDE.md: naming tables, code templates, file organization patterns discovered during exploration.

---

### 4. `development-commands`

**What:** Build, test, lint, format, run commands with gotchas. Includes pre-completion checklist.

**Template:**
```
# Development Commands

## Build & Run
[command] — [what it does] [gotchas if any]

## Test
[command] — [scope]

## Quality
[lint, format, type-check commands]

## Before Completing Work
1. [Step 1: specific command]
2. [Step 2: specific command]
...
```

**Anti-patterns:** Don't repeat commands from CLAUDE.md's "Development Commands" section — write `"see CLAUDE.md"` and only add what's missing: feature flags, optional tooling, environment setup not covered there. Don't copy the pre-completion checklist verbatim; CLAUDE.md's "Always run…" line already covers it.

---

### 5. `domain-glossary`

**What:** Project-specific terms, abbreviations, concepts that aren't obvious from code alone.

**Template:**
```
# Domain Glossary

**[Term]** — [1-sentence definition]. [File/module where it lives if relevant.]
**[Term]** — [1-sentence definition].
```

**What to include:** Domain model names with specific meaning, project-specific abbreviations, concepts requiring context.

**Anti-patterns:** Don't define terms that CLAUDE.md already explains (RecoverableError, OutputGuard, the two output modes, three-query sandwich, three prompt surfaces are all in CLAUDE.md). Don't copy definitions from docs — link to them. **Drift risk is high here:** glossary entries that describe specific types or APIs go stale as the code evolves. Prefer: `"OutputGuard — see src/tools/output.rs and CLAUDE.md § Design Principles"` over a full description. Only write inline definitions for concepts that exist nowhere else.

---

### 6. `gotchas`

**What:** Known issues, common mistakes, things that trip people up.

**Template:**
```
# Gotchas & Known Issues

## [Category]
- **Problem:** [what goes wrong]
  **Fix:** [what to do instead]
```

**What to include:** Config pitfalls, framework traps, build/test gotchas, flaky tests.

**Anti-patterns:** Don't invent problems that don't exist. Don't re-document issues already called out in CLAUDE.md (e.g. worktree `activate_project` requirement, tool misbehavior log — those are in CLAUDE.md already). Gotchas here should be things discovered during exploration that aren't in CLAUDE.md. If nothing new was found, write: "No additional gotchas discovered during onboarding. Update as issues are found." **Note:** gotchas about specific tool behavior or config values are high drift-risk — add a note about where to verify them (e.g. the config file or source line) so they can be checked rather than blindly trusted.

---

### 7. System Prompt — `.codescout/system-prompt.md`

After creating the 6 memories above, synthesize a concise system prompt (15-30 lines)
for this project. This prompt is injected into EVERY codescout session
automatically — it must be short and high-value. Do NOT repeat information from the
static tool guidance (how to use find_symbol, list_symbols, etc.) — that's already
provided to you separately.

**What to include:**
- Entry points: where to start exploring this codebase (specific files + symbols)
- Key abstractions: 3-5 core types/traits that form the skeleton of this project
- Search tips: semantic_search queries that work well for THIS codebase, and terms to avoid (too broad, too generic)
- Navigation strategy: recommended exploration order for a new task in this project
- Project rules: conventions the AI should always follow that aren't captured by linters

**What NOT to include (already covered elsewhere):**
- How codescout tools work (the static tool guidance handles this)
- Full architecture details (the `architecture` memory covers this)
- Command lists, glossary, detailed conventions (memories cover these)
- Anything over 30 lines (keep it concise — this is injected every session)

**Template:**
```
# [Project Name] — Code Explorer Guidance

## Entry Points
[Where to start. Specific files + symbols, not module descriptions.]

## Key Abstractions
[3-5 core types with file paths. What to understand first.]

## Search Tips
[Concrete query examples that work well. Terms to avoid.]

## Navigation Strategy
[Recommended exploration order for new tasks.]

## Project Rules
[Conventions the AI should always follow.]
```

**Process:** Present the draft to the user and ask: "Does this system prompt look
right? I'll save it to `.codescout/system-prompt.md`." After confirmation, write
the file using `create_file`. Inform the user they can edit it anytime.

---

## After Everything Is Created

After confirming all 6 memories and the system prompt with the user, deliver this:

---

**Your codescout setup is complete.**

- **System prompt** (`.codescout/system-prompt.md`) — always-on project guidance,
  injected into every session. Edit anytime to refine how AI navigates your codebase.
- **Memories** — reference material read on demand via `memory(action: "read", topic: ...)`. Update
  with `memory(action: "write", topic: ..., content: ...)`.
- **Semantic memories** — use `memory(action: "remember", content: "...")` to store knowledge
  that doesn't fit a named topic. Search later with `memory(action: "recall", query: "...")`.
  Useful for preferences, patterns discovered during work, and cross-cutting notes.
- **Quick start for new tasks:**
  1. `memory(action: "read", topic: "architecture")` — orient yourself
  2. `list_symbols("src/")` — see the module structure
  3. `semantic_search("your concept")` — find relevant code
  4. `find_symbol("Name", include_body=true)` — read the implementation

---

## Gathered Project Data

The data below was collected automatically. Use it as your starting point, then explore with codescout tools to fill gaps.

---

## Optional: Private Memories

After creating the 6 shared memories above, check if any personal context is worth
capturing now. Use `memory(action: "write", topic: ..., content: ..., private: true)` for anything specific
to your setup — local machine config, personal workflow preferences, or current WIP
context. This is optional; skip if nothing personal applies yet.

## Optional: Semantic Memories

For knowledge that doesn't fit a named topic — personal preferences, recurring patterns,
project-specific learnings — use semantic memories:

- `memory(action: "remember", content: "Always run integration tests with --release flag", bucket: "preferences")` — store a preference
- `memory(action: "remember", content: "The auth module uses a custom middleware chain")` — store a note (bucket auto-classified)
- `memory(action: "recall", query: "testing preferences")` — search by meaning later

Semantic memories with `bucket: "preferences"` are automatically included in future
onboarding prompts, so they persist across sessions without manual recall.
