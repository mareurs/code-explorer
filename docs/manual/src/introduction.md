# Introduction

This manual covers code-explorer: an MCP server that gives AI coding agents
IDE-grade code navigation, optimized for token efficiency.

---

## The Problem

When an AI coding agent tries to understand a codebase with conventional file
tools, it faces a mismatch between what the tools produce and what the task
actually requires.

Consider a routine task: "find all callers of `authenticate_user` and check
whether they handle the error case." With standard tools, the agent has a few
options:

- **grep** — returns every line containing the string, including comments,
  string literals, documentation, and test fixtures. Disambiguation is the
  agent's problem.
- **cat** — dumps the entire file when the agent needs one function. A 1,000-
  line module floods the context for a 30-line function.
- **find** — locates files by name, but has no awareness of what is inside them.

None of these tools understand code *structure*. They operate on bytes and
lines, not symbols, definitions, or references. The result is that agents burn
most of their context window on navigation overhead: reading full files to find
one function, re-reading the same module multiple times from different entry
points, asking questions they already answered two tool calls ago.

The downstream effects compound:

- **Shallow understanding.** When an agent can only see fragments at a time, it
  builds an incomplete picture and fills gaps with plausible-sounding guesses.
- **Hallucinated edits.** Functions that do not exist, arguments in the wrong
  order, return types copied from the wrong overload.
- **Constant course-correction.** The human has to re-read the agent's output,
  identify what it got wrong, and re-explain the structure it missed.

The tools are structurally blind. Every coding agent using only file primitives
runs into this wall, regardless of model capability.

---

## The Solution

code-explorer exposes the same information an IDE uses — symbol definitions,
references, call hierarchies, type information, git history — through a standard
MCP interface that any agent can call.

It is a Rust binary that runs alongside your coding agent. The agent sends MCP
tool calls; code-explorer delegates to the right backend (LSP server,
tree-sitter, git, embedding index) and returns structured, compact results.

Four pillars:

### LSP Navigation (7 tools, 9 languages)

The Language Server Protocol is how IDEs answer questions like "where is this
defined?" and "who calls this?". code-explorer runs LSP servers on your behalf
and exposes their answers as agent-friendly tools.

- `find_symbol` — locate any symbol by name across the project
- `get_symbols_overview` — the outline of a file or directory: classes,
  functions, structs, in tree form
- `find_referencing_symbols` — all callers/usages of a given symbol
- `replace_symbol_body` — replace a function body by name, not by line number
- `insert_before_symbol` / `insert_after_symbol` — add code relative to a
  named symbol
- `rename_symbol` — rename across the entire codebase via LSP

Supported languages: Rust, Python, TypeScript/JavaScript, Go, Java, Kotlin,
C/C++, C#, Ruby.

### Semantic Search (4 tools)

Sometimes you know the concept but not the name. Semantic search finds code by
meaning using embeddings, not keywords.

- `semantic_search` — "authentication middleware", "retry with exponential
  backoff", "how errors are serialized" — returns ranked code chunks. The
  optional `scope` parameter restricts search to project code, a specific
  library, or all sources.
- `index_project` — build or incrementally update the embedding index (smart
  change detection via git diff → mtime → SHA-256 fallback)
- `index_status` — check index coverage and staleness
- `check_drift` — after re-indexing, see which files changed meaningfully in
  *semantics* vs. trivially in bytes. Opt out with `drift_detection_enabled = false`
  in `[embeddings]`.

The embedding backend is configurable: OpenAI, Ollama, or any compatible
endpoint.

### Git Integration (3 tools)

- `git_blame` — who last changed each line and in which commit
- `git_log` — commit history for a file or the whole project
- `git_diff` — uncommitted changes, or diff against a specific commit

### Persistent Memory (4 tools)

Agents are stateless across sessions by default. code-explorer provides a
lightweight key-value store backed by markdown files in `.code-explorer/memories/`.

- `write_memory` / `read_memory` / `list_memories` / `delete_memory`

Use this to record decisions, gotchas, and conventions so the agent picks them
up on the next session without re-discovery.

### Library Navigation (2 tools)

Navigate third-party dependency source code without leaving your agent workflow.
Libraries auto-register when LSP `goto_definition` returns paths outside the
project root.

- `list_libraries` — see all registered libraries and their status
- `index_library` — build an embedding index for a library so you can
  `semantic_search` within it using `scope: "lib:<name>"`

### The Rest

Beyond the five pillars: 7 file operation tools (directory listing, file
reading, pattern search, file creation, content replacement), 2 AST analysis
tools (function signatures, docstrings via tree-sitter), 3 workflow tools
(project onboarding, shell commands), and 2 config tools — **33 tools total**.

### Token Efficiency by Design

Every tool defaults to the most compact representation that is still useful.
Full bodies are available via `detail_level: "full"`. Paginated results use
`offset` and `limit`. Tools never dump unbounded output.

The design follows two modes:

- **Exploring** (default) — names and locations, capped at 200 items. Low
  token cost. Right for orientation.
- **Focused** — full detail, paginated. Use once you know what you are looking
  at.

When results overflow the cap, the tool tells you how to narrow the query rather
than silently truncating. You get guidance, not garbage.

---

## Who This Manual Is For

This manual is written for three audiences.

### Operators

You are setting up code-explorer for a team or configuring it to work with
Claude Code, Cursor, or another MCP-capable agent. You need to understand
installation, the MCP configuration format, embedding backend options, and
which LSP servers to install for your languages.

Start here: [Installation](getting-started/installation.md), then
[Project Configuration](configuration/project-toml.md).

### End-User Developers

You are a developer using Claude Code (or another agent) with code-explorer
already set up. You want to understand what the tools do and when to reach for
each one, so you can ask the agent better questions and interpret its reasoning.

Start here: [Progressive Disclosure](concepts/progressive-disclosure.md) and
[Tool Selection](concepts/tool-selection.md), then browse the
[Tool Reference](tools/overview.md) for the categories you use most.

### Contributors

You want to add a language, write a new tool, or swap in a different embedding
backend. You need to understand the internal architecture: the `Tool` trait,
the LSP client, the embedding pipeline, the output guard system.

Start here: [Architecture](architecture.md), then
[Adding Languages](extending/adding-languages.md) and
[Writing Tools](extending/writing-tools.md).

---

## How to Read This Manual

The manual is organized into three sections:

**User Guide** — everything you need to install, configure, and use
code-explorer. Reads linearly for first-time setup; use it as a reference once
you are familiar.

**Tool Reference** — one page per tool category. Each page covers what the
tools do, their parameters, output format, and when to prefer them over
alternatives. You do not need to read this cover to cover; look up the
category you need.

**Development** — architecture internals, extension guides, and troubleshooting.
Oriented toward contributors and operators debugging unexpected behavior.

---

## A Quick Example

Here is what a concrete agent interaction looks like with code-explorer versus
without it.

**Without code-explorer** — the agent uses `read_file` on `auth.rs` (850
lines), scans for `authenticate_user`, reads the function, then uses `grep` for
callers, gets 23 hits including test fixtures, reads three more files to
disambiguate, and still misses that the error type changed in a recent refactor.

**With code-explorer:**

```
get_symbols_overview("src/auth.rs")
  → authenticate_user [fn, line 142], SessionStore [struct, line 12], ...

find_referencing_symbols("authenticate_user", "src/auth.rs")
  → middleware/auth_guard.rs:88, handlers/login.rs:34, handlers/api.rs:201

git_log("src/auth.rs")
  → 3 days ago: "refactor: change AuthError to return structured payload"

find_symbol("AuthError", include_body=true)
  → enum AuthError { ... } with full definition
```

Four targeted calls. The agent sees the symbol tree, the exact call sites, the
relevant git history, and the type definition — without reading a single full
file. That is the difference code-explorer makes.
