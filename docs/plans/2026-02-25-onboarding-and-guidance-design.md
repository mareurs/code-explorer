# Onboarding & Claude Code Guidance Design

**Date:** 2026-02-25
**Status:** Approved

## Problem

code-explorer's MCP server has a single generic sentence as its `instructions` field, and the onboarding tool returns raw JSON with no guidance for Claude Code on how to use the tools effectively. Claude Code defaults to its built-in Read/Grep/Glob for source files, missing the semantic tools entirely.

## Research Sources

- **Serena onboarding**: "Prompt dispenser" pattern — tools return instruction prompts, LLM does the analysis. Onboarding creates structured memories. "Thinking tools" for self-reflection.
- **tool-infra plugin**: Hook-based steering — blocks Read/Grep/Glob on source files via PreToolUse hooks, injects tool guidance at SessionStart/SubagentStart, auto-fixes parameter mistakes.
- **Anthropic Skills Guide**: Progressive disclosure (frontmatter → body → linked files), trigger-rich descriptions, MCP+Skills paradigm ("MCP = what Claude can do, Skills = how Claude should do it").

## Goals

1. Claude Code gets **tool guidance from the moment the server connects** (server instructions)
2. First-time project discovery **creates rich memories** for future sessions (onboarding prompt)
3. **Opinionated steering** toward semantic tools (symbol navigation, AST, embeddings) over raw file reads
4. Claude Code only — we can be opinionated about the workflow

## Approach: Smart Server + Prompt Templates

Two layers, both context-aware:

### Layer 1: Server Instructions (every session)

Dynamic `ServerInfo.instructions` generated based on project state. Contains:

**Static preamble (always present):**
- Tool decision matrix: when to use each tool category
- Workflow patterns: "Understand Before Editing", "Find Usages Before Refactoring", "Discover Then Drill Down"
- Steering rules: prefer `get_symbols_overview` + `find_symbol` over `read_file` for source code
- Escape hatches: when raw file reads are appropriate (non-code files, targeted line ranges)

**Dynamic section (conditional):**
- Project name, path, detected languages
- Available memories (with instruction to read relevant ones, not all)
- Semantic index status (built/not built, suggestion to run `index_project`)

### Layer 2: Onboarding Prompt (first session)

The `onboarding` tool keeps its mechanical work (language detection, config creation) and adds a rich instruction prompt telling Claude what to explore and what memories to create.

**Exploration instructions:**
1. Project purpose — read README/docs
2. Tech stack — from build files + detected languages
3. Code architecture — `get_symbols_overview("src")` then deeper on key modules
4. Key abstractions — core types/traits/interfaces via `find_symbol`
5. Conventions — linting/formatting config, naming patterns, error handling style
6. Development commands — from build configs and CI
7. Architectural patterns — design patterns identified from code structure

**Memory categories to create:**
1. `project-overview` — purpose, tech stack, key dependencies
2. `architecture` — module structure, key abstractions, data flow, design patterns
3. `conventions` — code style, naming, error handling, testing patterns
4. `development-commands` — build, test, lint, format, run commands
5. `task-completion-checklist` — what to do when finishing a task

**`check_onboarding_performed` enhancement:**
When memories exist, returns memory list with instruction to read relevant ones on-demand, not all at once.

## Implementation Architecture

### File layout

```
src/
├── prompts/
│   ├── mod.rs                   # Module: prompt generation functions
│   ├── server_instructions.md   # Static template (include_str!)
│   └── onboarding_prompt.md     # Onboarding template with {placeholders}
```

### Changes

1. **`src/server.rs`** — `get_info()` generates dynamic instructions from template + project state
2. **`src/tools/workflow.rs`** — `Onboarding::call()` returns instruction prompt alongside JSON data; `CheckOnboardingPerformed::call()` returns memory list with guidance
3. **New `src/prompts/`** — markdown templates compiled into binary via `include_str!`

### What doesn't change
- Tool trait, tool registration, MCP bridge
- Memory system, config system, embedding pipeline
- All existing tool implementations
- Test structure

### No new dependencies
Just `include_str!` and string formatting.

## Key Design Decisions

1. **Prompts as .md files** — maintainable, readable, version-controlled. Compiled into binary via `include_str!` so no runtime file I/O needed.
2. **Dynamic server instructions** — adapts to project state (memories exist? index built?). Generated at startup and refreshed on project change.
3. **LLM-driven exploration** — the onboarding tool instructs, doesn't analyze. Claude uses the semantic tools to explore, producing richer memories than static analysis.
4. **Progressive disclosure** — server instructions (always loaded) → onboarding prompt (first session) → memories (read on-demand).
5. **Semantic index kept separate** — onboarding mentions it exists but doesn't mandate building it. Keeps onboarding fast.
