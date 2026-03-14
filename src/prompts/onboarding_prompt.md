You have just onboarded this project. Your job is to create 6 memories and a system
prompt that give future AI sessions deep, accurate knowledge of this codebase.

## THE IRON LAW

```
NO MEMORIES WRITTEN WITHOUT COMPLETING ALL EXPLORATION STEPS FIRST
```

```
DON'T LEAVE ANY STONE UNTURNED
```

**Violating the letter of this process is violating the spirit of onboarding.**

This is a one-time setup. Every future AI session depends on the accuracy of what you
write now. **Token efficiency is NOT a concern here. Thoroughness is the ONLY goal.**
Be exhaustive. Read widely. When in doubt, read more.

<HARD-GATE>
Do NOT call `memory(action: "write", ...)` until you have:
1. Completed ALL 7 exploration steps below
2. Verified EVERY item in the Phase 1 Gate Checklist
3. Written the Exploration Summary in your response

These gates are non-negotiable. There are no exceptions.
</HARD-GATE>

---

## Phase 0.5: Embedding Model Selection

The `onboarding` tool has already written a recommended model to `.codescout/project.toml`
based on your system hardware. Present the options to the user now, before indexing starts.

Use the `model_options` array from the Gathered Project Data below to build the menu.
Use the `hardware` field for the one-line system summary.

Present this to the user:

> **Choose an embedding model for semantic search.**
>
> Based on your system ({hardware.cpu_cores} CPU cores
> {if hardware.gpu: ", {hardware.gpu.name}"}
> {if hardware.ollama_available: ", Ollama running" else: ", no Ollama detected"}):
>
> 1. ★ `{model_options[0].id}` — {model_options[0].dims}d, {model_options[0].context_tokens}-token context
>    {model_options[0].reason} ← **Recommended**
> 2. `{model_options[1].id}` — {model_options[1].dims}d, {model_options[1].context_tokens}-token context
>    {model_options[1].reason}
> 3. `{model_options[2].id}` — {model_options[2].dims}d, {model_options[2].context_tokens}-token context
>    {model_options[2].reason}{if not model_options[2].available: " *(not currently available)*"}
>
> Press Enter to accept [1], or type 2 or 3 to choose a different model.

Wait for the user's response, then:

- **User presses Enter or types 1:** The config is already correct — proceed to Phase 0.
- **User types 2:** Call `edit_file` on `.codescout/project.toml`.
  Change the line `model = "{model_options[0].id}"` to `model = "{model_options[1].id}"`.
  Confirm the edit, then proceed to Phase 0.
- **User types 3:** Same as above but use `model_options[2].id`.
  If `model_options[2].available` is false, remind the user how to enable it
  (e.g., "install Ollama and run `ollama serve`") before making the edit.
- **User types a custom model string:** Use that string directly in the `edit_file` call.

Then proceed to Phase 0 (Semantic Index Check).

---

## Phase 0: Semantic Index Check

Check the **Semantic index** line in the Gathered Project Data below.

### If the index is READY:

Announce to the user:

> "Semantic index is ready ({files} files, {chunks} chunks). I'll use
> `semantic_search` for concept-level exploration in Phase 1."

Proceed to Phase 1.

### If the index is NOT BUILT:

Semantic search is **strongly recommended** for thorough onboarding. Present
this to the user:

> **Semantic search is not set up yet.**
>
> The embedding index powers concept-level code exploration (`semantic_search`),
> which finds code by meaning — not just by name or text pattern. Without it,
> onboarding relies on symbol tools and regex search, which work but may miss
> non-obvious connections.
>
> **Options:**
> 1. **Build now** — I'll call `index_project` and wait for it to finish.
>    Requires an embedding backend (Ollama is the default — see
>    `docs/manual/src/configuration/embedding-backends.md` for setup).
>    Takes 1–5 minutes depending on codebase size.
> 2. **Build from CLI** — Run `codescout index --project .` in another
>    terminal, then restart onboarding with `onboarding(force: true)`.
> 3. **Skip** — Proceed without semantic search. Exploration will use
>    `search_pattern` (regex) instead of `semantic_search`. You can always
>    build the index later.

Wait for the user's choice before proceeding.

- **Option 1:** Call `index_project({})`. Poll `index_status({})` every 15
  seconds until the response shows completion or failure. If it fails, inform
  the user of the error and fall back to option 3.
- **Option 2:** Stop and wait for the user to return.
- **Option 3:** Proceed to Phase 1. Step 6 will use `search_pattern` instead
  of `semantic_search`.

---

## Phase 1: Explore the Code

The gathered data below (README, build config, CLAUDE.md) is a **starting point, not a
substitute for exploration**. Memories written from documentation alone are shallow,
incomplete, and frequently wrong. Code and docs diverge — only reading the code reveals
what's actually true.

### Step 1: Map the Codebase Structure

Run ALL of these — do not skip any. See the **Key files to read** list in the
Gathered Project Data section below for project-specific paths detected during onboarding.

- `list_dir(".")` — top-level structure
- `list_dir` on EACH major directory found (src/, tests/, docs/, lib/, app/, etc.)
- `read_file` on the build config fully (Cargo.toml / package.json / pyproject.toml / go.mod / pom.xml)
- `read_file` on CI config if present (.github/workflows/, .gitlab-ci.yml, Makefile, etc.)
- `read_file("README.md")` fully — even if you think you know what it says

### Step 2: Full Symbol Survey — ALL Modules

Do NOT stop at a single top-level `list_symbols("src/")`. You MUST:

- Run `list_symbols` on the top-level source directory
- Run `list_symbols` on EACH subdirectory individually
- Identify every module/package/namespace and survey its symbols
- Continue until you have seen symbols in every non-trivial source file

**Minimum:** Survey at least 5 distinct source modules or files (more for larger
projects). If you are writing memories after surveying only 1–2 files, you have not
done enough. Go back.

### Step 3: Read Core Implementations — With Actual Bodies

Signatures are not enough. You must read actual code.

- Identify the 5+ most central types, traits, or classes from Step 2
- For each: `find_symbol(name, include_body=true)`
- If body is truncated (only the signature returned): use `list_symbols(path)` to get
  correct line ranges, then `read_file(path, start_line=N, end_line=M)`
- For top-level free functions in large files: use `read_file` with line ranges

**Minimum:** Read the FULL body of at least 5 core implementations.
Do not proceed from signatures alone. Signatures tell you *what*; bodies tell you *how*.

### Step 4: Read ALL Architecture Documentation

- `read_file("docs/ARCHITECTURE.md")` fully if it exists
- `read_file` on any design docs, ADRs, or plans referenced in README or CLAUDE.md
- `read_file` on any additional doc files under `docs/`
- Read completely — do not skim headings and move on

**If there are no architecture docs:** explicitly note this in your exploration summary.

### Step 5: Trace TWO Complete Data Flows

Documentation describes intent. Code traces reveal reality. Discrepancies hide between them.

You must trace TWO paths:
1. The most representative operation (e.g. a request, command, event processed)
2. A second distinct path (e.g. an error path, a write vs. read, a different entry point)

For each trace:
- Start at the entry point
- Follow with `goto_definition`, `find_references`, `find_symbol`
- Use `semantic_search("how X flows")` to find connecting code
- Continue until you reach the output or terminal state

You cannot write an accurate `architecture` memory without doing this.

### Step 6: Code Exploration by Concept — Minimum 5 Queries

Search for code by concept, not just by name. Run at least 5 queries covering
different aspects of the codebase:

1. Error handling / failure paths
2. Data flow / request lifecycle
3. Testing approach / test helpers
4. Configuration / initialization / startup
5. A core domain concept specific to this project (not generic)

Use `semantic_search` if the embedding index is built. If `semantic_search` returns
empty results or errors (index not yet built), use `search_pattern` (regex) instead —
it works without an index and still reveals how the codebase handles each concept.

Do NOT run `index_project` during onboarding — it can take minutes and is not required
for thorough exploration.

Note where the code diverges from what the documentation says.

### Step 7: Examine Tests and Verify Build

- `list_symbols` on the test directory (`tests/`, `__tests__/`, `spec/`, or equivalent)
- Read 2–3 test files to understand: framework used, fixtures, mock patterns, test organization
- Find at least one test for a core abstraction and read it completely
- Verify the development commands in CLAUDE.md actually exist in the repo

---

### Phase 1 Gate Checklist

Before writing ANY memory, verify ALL of these are true. If any is unchecked, complete it first.

- [ ] Listed top-level structure AND ran `list_dir` on each major subdirectory
- [ ] Ran `list_symbols` on the top-level source AND on at least 4 subdirectories individually
- [ ] Read the FULL body (not just signature) of at least 5 core types/functions
- [ ] Read ALL architecture docs found, completely (not skimmed)
- [ ] Traced two distinct data flows from entry point to terminal output
- [ ] Ran at least 5 concept-level queries (`semantic_search` or `search_pattern` fallback)
- [ ] Read 2–3 test files and understood the testing pattern
- [ ] Verified build/dev commands against actual repo contents

**If ANY item is unchecked: complete it before writing a single memory.**

---

### Exploration Summary

After completing all steps, write this summary **in your response, before calling any
`memory(action: "write", ...)` tool**:

> **What this system does** — in your own words, not the README's
> **The 5 most important types/modules** — name, file path, and role each plays
> **How a typical operation flows** — concrete function/method names, not just layers
> **What surprised you** — things the code does that documentation didn't mention

If you cannot write this from what you've explored, you have not explored enough.
Return to Phase 1.

---

## Red Flags — STOP and Return to Phase 1

If you notice any of these thoughts, STOP. Return to Phase 1 immediately.

- "I've read CLAUDE.md and the README — that's enough to write the memories"
- "The architecture doc covers everything I need"
- "I can infer how it works from the signatures and names"
- "I only need to survey the main files, not every module"
- "This project is small/simple, less exploration is fine"
- "I'll write the memory now and add details if something is wrong later"
- "I already understand this type of codebase"
- You have read fewer than 5 code bodies with `include_body=true`
- You have run `list_symbols` on fewer than 3 modules/directories
- You have traced only one data flow
- You have run fewer than 5 concept-level queries (semantic_search or search_pattern)

**ALL of these mean: STOP. Return to Phase 1.**

## Common Rationalizations

| Excuse | Reality |
|---|---|
| "CLAUDE.md and the README give me enough context" | Docs describe intent. Code reveals reality. Discrepancies hide in the code. |
| "I can infer implementations from names and signatures" | Assumptions about implementations produce wrong memories that mislead future sessions. |
| "I already understand this type of system" | Pattern recognition replaces exploration. This codebase has specific wiring that differs from the pattern. |
| "This is a small project, I can do less" | Small codebases still have gotchas. The steps scale down naturally — don't skip them. |
| "I'll refine the memories later if something is wrong" | Wrong memories mislead every session until someone notices and fixes them. Do it right once. |
| "Token efficiency matters here" | This is a ONE-TIME setup. Tokens spent here prevent thousands of wasted tokens in every future session. Be thorough. |
| "I traced one flow — that's enough" | One flow shows one path. A second reveals where paths diverge and where exceptions live. |
| "I read the docs — I understand the architecture" | Architecture docs describe the intended design. Code reveals the actual design. Read both. |

---

## Phase 2: Write the 6 Memories

Now write the memories. Your Phase 1 exploration must inform every memory — especially
`architecture` and `conventions`, which cannot be written accurately from documentation alone.

### Rules

1. **Do NOT duplicate auto-loaded context** — CLAUDE.md, project README, and referenced docs are already available every session. Memories must *supplement* them, not repeat them. If something is already documented, write a pointer (`see CLAUDE.md § Key Patterns`) rather than copying it.
2. **References over copies — drift is real** — Code and docs change. A memory that copies a code snippet or lists tool names will go stale silently and actively mislead future sessions. Prefer: `"see docs/ARCHITECTURE.md for the layer diagram"` over pasting the diagram. Reserve inline content for things that are NOT documented elsewhere.
3. **Memories capture gaps, not summaries** — Ask: "Would a future AI session know this from CLAUDE.md, the README, or the referenced docs?" If yes, skip it or point to the source. Only write it if the answer is no.
4. **Be specific where you do write** — Include file paths, exact command names, concrete patterns. "Uses clean architecture" is useless. "`api/ → service/ → repository/` with interface+impl pattern in `src/`" is useful.
5. **Be concise** — Each memory should be 15–40 lines. Longer means too much detail or duplication.
6. **Confirm with the user** — After creating all 6 memories, summarize what you wrote and ask if anything needs correction.
7. **Private memories** — Use `memory(action: "write", topic: ..., content: ..., private: true)` for project-local notes that should not appear in system instructions (e.g. personal debugging notes, temporary state). Standard `memory(action: "write", ...)` creates shared memories visible to all agents.

### Protected Memories

Check the `protected_memories` field from the onboarding tool response above. For
each memory you are about to write, check whether it appears there:

**If `protected_memories[topic].exists == false`:** Create fresh as normal.

**If `protected_memories[topic].exists == true` AND `staleness.untracked == false`
AND `staleness.stale_files` is empty:** The memory is fresh — all anchored source
files are unchanged. **Skip writing this topic entirely.** Tell the user:
> "Kept `[topic]` unchanged (all references still valid)."

**If `protected_memories[topic].exists == true` AND (`staleness.untracked == true`
OR `staleness.stale_files` is non-empty):** Run the merge flow:

1. The existing content is in `protected_memories[topic].content`.
2. For entries referencing files listed in `staleness.stale_files` (or all
   entries if `untracked`): use `find_symbol`, `read_file`, `search_pattern`
   to verify whether each entry is still accurate.
3. Identify new discoveries from your Phase 1 exploration that belong in
   this memory.
4. Present a diff-style summary to the user:
   - **Stale (recommend removing):** [entries no longer accurate, with reason]
   - **Still valid (keeping):** [verified entries]
   - **New findings:** [discoveries from exploration]
   - **Proposed merged version:** [full content]
5. **Wait for user approval** before calling `memory(action="write")`.

**If a topic is NOT in `protected_memories`:** Write it as normal (overwrite).

The protected topics list is configured in `project.toml` under `[memory] protected`.
Users can add custom topics. The programmatic memories (`onboarding`, `language-patterns`)
are always excluded from protection.

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

**Anti-patterns:** Don't repeat CLAUDE.md's "Design Principles" section — it's already loaded. Reference it: `"see CLAUDE.md § Design Principles"`. Write only conventions that are absent from CLAUDE.md: naming tables, code templates, file organization patterns discovered during exploration.

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

**Anti-patterns:** Don't define terms that CLAUDE.md already explains. Don't copy definitions from docs — link to them. **Drift risk is high here:** glossary entries that describe specific types or APIs go stale as the code evolves. Prefer: `"OutputGuard — see src/tools/output.rs and CLAUDE.md § Design Principles"` over a full description. Only write inline definitions for concepts that exist nowhere else.

---

### 6. `gotchas`

> **Note:** `gotchas` is protected by default. If it already exists and the
> onboarding result shows it in `protected_memories`, follow the Protected
> Memories flow above instead of overwriting.

**What:** Known issues, common mistakes, things that trip people up.

**Template:**
```
# Gotchas & Known Issues

## [Category]
- **Problem:** [what goes wrong]
  **Fix:** [what to do instead]
```

**What to include:** Config pitfalls, framework traps, build/test gotchas, flaky tests.

**Anti-patterns:** Don't invent problems that don't exist. Don't re-document issues already called out in CLAUDE.md. Gotchas here should be things discovered during exploration that aren't in CLAUDE.md. If nothing new was found, write: "No additional gotchas discovered during onboarding. Update as issues are found." **Note:** gotchas about specific tool behavior or config values are high drift-risk — add a note about where to verify them (e.g. the config file or source line) so they can be checked rather than blindly trusted.

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
- **Library support:**
  - Libraries are **auto-discovered** when `goto_definition` resolves outside the project root.
  - `list_libraries` — view all registered libraries and their index/version status.
  - `index_project(scope="lib:<name>")` — index a specific library for semantic search.
  - Once registered, use `scope="lib:<name>"` with `find_symbol`, `list_symbols`,
    `search_pattern`, and `semantic_search` to navigate library code.

---

Finally, inform the user:

> **Onboarding complete.** To activate the new project configuration in this session,
> restart Claude Code or run `/mcp` to reconnect the MCP server.

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
