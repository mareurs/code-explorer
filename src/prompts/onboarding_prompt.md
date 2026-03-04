You have just onboarded this project. Below you'll find pre-gathered context from key project files. Your job is to **synthesize this into 6 memories** using `memory(action: "write", topic: ..., content: ...)`.

## Rules

1. **Do NOT duplicate CLAUDE.md** — If CLAUDE.md content is provided below, it's loaded every session automatically. Memories should *supplement* it, not repeat it. If CLAUDE.md already covers dev commands, your `development-commands` memory should only add what's missing.
2. **Be specific** — Include file paths, exact command names, concrete patterns. "Uses clean architecture" is useless. "api/ → service/ → repository/ with interface+impl pattern" is useful.
3. **Be concise** — Each memory should be 15–40 lines. Longer means too much detail.
4. **Explore before writing** — The gathered data gives you a head start, but use code-explorer tools to verify and fill gaps: `list_symbols("src/")` for architecture, `find_symbol` for key abstractions, `list_symbols` for API surface.
5. **Confirm with the user** — After creating all 6 memories, summarize what you wrote and ask if anything needs correction.
6. **Private memories** — Use `memory(action: "write", topic: ..., content: ..., private: true)` for project-local notes that should not appear in system instructions (e.g. personal debugging notes, temporary state). Standard `memory(action: "write", ...)` creates shared memories visible to all agents.

## Memories to Create

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

**Anti-patterns:** Don't list every dependency. Don't include directory listings. Don't copy the README.

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
```

**Anti-patterns:** Don't list every file. Don't describe standard library types. DO include file paths for every abstraction.

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

**Anti-patterns:** Don't describe language-standard conventions everyone knows. Focus on project-specific conventions.

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

**Anti-patterns:** Don't duplicate commands already in CLAUDE.md (reference "see CLAUDE.md" instead). DO include non-obvious gotchas.

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

**Anti-patterns:** Don't define standard programming terms (API, REST, ORM). DO define terms used in project-specific ways.

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

**Anti-patterns:** Don't invent problems that don't exist. If nothing is obvious, write "No known gotchas discovered during onboarding. Update this memory as issues are found."

---

### 7. System Prompt — `.code-explorer/system-prompt.md`

After creating the 6 memories above, synthesize a concise system prompt (15-30 lines)
for this project. This prompt is injected into EVERY code-explorer session
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
- How code-explorer tools work (the static tool guidance handles this)
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
right? I'll save it to `.code-explorer/system-prompt.md`." After confirmation, write
the file using `create_file`. Inform the user they can edit it anytime.

---

## After Everything Is Created

After confirming all 6 memories and the system prompt with the user, deliver this:

---

**Your code-explorer setup is complete.**

- **System prompt** (`.code-explorer/system-prompt.md`) — always-on project guidance,
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

The data below was collected automatically. Use it as your starting point, then explore with code-explorer tools to fill gaps.

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
