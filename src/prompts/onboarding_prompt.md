You have just onboarded this project. Below you'll find pre-gathered context from key project files. Your job is to **synthesize this into 6 memories** using `write_memory(topic, content)`.

## Rules

1. **Do NOT duplicate CLAUDE.md** — If CLAUDE.md content is provided below, it's loaded every session automatically. Memories should *supplement* it, not repeat it. If CLAUDE.md already covers dev commands, your `development-commands` memory should only add what's missing.
2. **Be specific** — Include file paths, exact command names, concrete patterns. "Uses clean architecture" is useless. "api/ → service/ → repository/ with interface+impl pattern" is useful.
3. **Be concise** — Each memory should be 15–40 lines. Longer means too much detail.
4. **Explore before writing** — The gathered data gives you a head start, but use code-explorer tools to verify and fill gaps: `get_symbols_overview("src")` for architecture, `find_symbol` for key abstractions, `list_functions` for API surface.
5. **Confirm with the user** — After creating all 6 memories, summarize what you wrote and ask if anything needs correction.

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

## Gathered Project Data

The data below was collected automatically. Use it as your starting point, then explore with code-explorer tools to fill gaps.
