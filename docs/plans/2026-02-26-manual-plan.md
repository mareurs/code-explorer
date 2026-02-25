# Manual Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a complete mdBook user manual for code-explorer, deployed to GitHub Pages via GitHub Actions.

**Architecture:** mdBook project in `docs/manual/`, built to `target/manual/`. GitHub Actions workflow deploys on push to master. CI sync check ensures all tools in code are documented.

**Tech Stack:** mdBook, GitHub Actions, GitHub Pages

**Design doc:** `docs/plans/2026-02-26-manual-design.md`

---

### Task 1: Scaffold mdBook project

**Files:**
- Create: `docs/manual/book.toml`
- Create: `docs/manual/src/SUMMARY.md`

**Step 1: Install mdBook locally**

Run: `cargo install mdbook`
Expected: mdbook binary available

**Step 2: Create `book.toml`**

```toml
[book]
title = "code-explorer Manual"
authors = ["Marius"]
language = "en"
src = "src"

[build]
build-dir = "../../target/manual"

[output.html]
git-repository-url = "https://github.com/mareurs/code-explorer"
edit-url-template = "https://github.com/mareurs/code-explorer/edit/master/docs/manual/{path}"
no-section-label = false
```

**Step 3: Create `SUMMARY.md`**

```markdown
# Summary

[Introduction](introduction.md)

# User Guide

- [Getting Started](getting-started/installation.md)
  - [Installation](getting-started/installation.md)
  - [Your First Project](getting-started/first-project.md)
  - [Routing Plugin](getting-started/routing-plugin.md)

- [Core Concepts](concepts/progressive-disclosure.md)
  - [Progressive Disclosure](concepts/progressive-disclosure.md)
  - [Tool Selection](concepts/tool-selection.md)
  - [Output Modes](concepts/output-modes.md)

- [Configuration](configuration/project-toml.md)
  - [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Semantic Search Guide](semantic-search-guide.md)

- [Language Support](language-support.md)

# Tool Reference

- [Tools Overview](tools/overview.md)
  - [Symbol Navigation](tools/symbol-navigation.md)
  - [File Operations](tools/file-operations.md)
  - [Editing](tools/editing.md)
  - [Semantic Search](tools/semantic-search.md)
  - [Git](tools/git.md)
  - [AST Analysis](tools/ast.md)
  - [Memory](tools/memory.md)
  - [Workflow & Config](tools/workflow-and-config.md)

# Development

- [Architecture](architecture.md)
- [Extending code-explorer](extending/adding-languages.md)
  - [Adding Languages](extending/adding-languages.md)
  - [Writing Tools](extending/writing-tools.md)
  - [The Tool Trait](extending/tool-trait.md)

- [Troubleshooting](troubleshooting.md)
```

**Step 4: Create stub files for every page**

Create each `.md` file listed in SUMMARY.md with a `# Title` heading and a TODO placeholder. This makes `mdbook build` pass immediately.

**Step 5: Build to verify**

Run: `mdbook build docs/manual`
Expected: Clean build, output in `target/manual/`

**Step 6: Commit**

```bash
git add docs/manual/
git commit -m "docs: scaffold mdBook manual with chapter stubs"
```

---

### Task 2: Write Introduction

**Files:**
- Modify: `docs/manual/src/introduction.md`

**Step 1: Write content**

Cover:
- The problem (LLMs waste context on blind navigation)
- The solution (IDE-grade tools via MCP, optimized for token efficiency)
- The four pillars (LSP, Semantic Search, Git, Memory)
- Who this manual is for (operators, users, contributors)
- How to read the manual (guide for each audience tier)

Source material: `README.md` lines 1-24 (expand, don't copy verbatim).

**Step 2: Build to verify**

Run: `mdbook build docs/manual`
Expected: Clean build

**Step 3: Commit**

```bash
git add docs/manual/src/introduction.md
git commit -m "docs: write manual introduction"
```

---

### Task 3: Write Getting Started (3 pages)

**Files:**
- Modify: `docs/manual/src/getting-started/installation.md`
- Modify: `docs/manual/src/getting-started/first-project.md`
- Modify: `docs/manual/src/getting-started/routing-plugin.md`

**Step 1: Write `installation.md`**

Cover:
- Prerequisites (Rust toolchain, cargo)
- `cargo install code-explorer`
- Registering as MCP server: global (`claude mcp add --global`) and per-project (`.mcp.json`)
- Verification: `claude mcp list`
- Feature flags: `--features local-embed` for CPU-based embeddings

Source material: `README.md` lines 26-80.

**Step 2: Write `first-project.md`**

Cover:
- Start a Claude Code session in a project
- What happens automatically (LSP starts, config created)
- Running `onboarding` for first-time discovery
- Building the embedding index: `index_project`
- Trying basic tools: `list_dir`, `find_symbol`, `semantic_search`
- Show sample tool calls and their output

Source material: `src/prompts/server_instructions.md` (tool usage patterns), `src/prompts/onboarding_prompt.md`.

**Step 3: Write `routing-plugin.md`**

Cover:
- What the plugin does (hooks: SessionStart, SubagentStart, PreToolUse)
- Why it matters (subagents start blank, Claude falls back to grep/cat)
- Installation: `claude /plugin install code-explorer-routing@sdd-misc-plugins`
- The interaction diagram from README

Source material: `README.md` lines 82-110.

**Step 4: Build to verify**

Run: `mdbook build docs/manual`
Expected: Clean build

**Step 5: Commit**

```bash
git add docs/manual/src/getting-started/
git commit -m "docs: write getting started chapters"
```

---

### Task 4: Write Core Concepts (3 pages)

**Files:**
- Modify: `docs/manual/src/concepts/progressive-disclosure.md`
- Modify: `docs/manual/src/concepts/tool-selection.md`
- Modify: `docs/manual/src/concepts/output-modes.md`

**Step 1: Write `progressive-disclosure.md`**

Cover:
- The problem: unbounded output fills context windows
- The solution: compact by default, detail on demand
- The two modes: exploring (default) and focused
- How `OutputGuard` enforces this project-wide
- The pattern: explore broadly → identify target → focus narrowly

Source material: `docs/plans/2026-02-25-progressive-disclosure-design.md`, `src/tools/output.rs`.

**Step 2: Write `tool-selection.md`**

Cover:
- The "knowledge level" heuristic:
  - Know the name → LSP/AST tools
  - Know the concept → semantic search first
  - Know nothing → list_dir + get_symbols_overview
- Decision flowchart
- Common anti-patterns (reading whole files, grepping when you should use find_symbol)

Source material: `src/prompts/server_instructions.md` lines 1-25.

**Step 3: Write `output-modes.md`**

Cover:
- Exploring mode: compact, capped at 200 items
- Focused mode: full detail, paginated via offset/limit
- `detail_level: "full"` parameter
- Overflow messages and how to respond to them
- Examples of the same tool call in both modes

Source material: `src/prompts/server_instructions.md` lines 27-46.

**Step 4: Build to verify**

Run: `mdbook build docs/manual`

**Step 5: Commit**

```bash
git add docs/manual/src/concepts/
git commit -m "docs: write core concepts chapters"
```

---

### Task 5: Write Tool Reference — Symbol Navigation

**Files:**
- Modify: `docs/manual/src/tools/overview.md`
- Modify: `docs/manual/src/tools/symbol-navigation.md`

**Step 1: Write `overview.md`**

A quick map of all 31 tools organized by category with one-line descriptions. Link to each category page. Include the "which tool do I use?" decision tree.

**Step 2: Write `symbol-navigation.md`**

Document these 7 tools using the standard format (purpose, parameters, example, output in both modes, tips):
- `find_symbol`
- `get_symbols_overview`
- `find_referencing_symbols`
- `replace_symbol_body`
- `insert_before_symbol`
- `insert_after_symbol`
- `rename_symbol`

Source material: `src/tools/symbol.rs` — read each tool's `description()` and `input_schema()` methods for accurate parameter docs. Run the tools against code-explorer's own codebase to capture real output for examples.

**Step 3: Build to verify**

Run: `mdbook build docs/manual`

**Step 4: Commit**

```bash
git add docs/manual/src/tools/overview.md docs/manual/src/tools/symbol-navigation.md
git commit -m "docs: write tool reference — overview and symbol navigation"
```

---

### Task 6: Write Tool Reference — File Operations & Editing

**Files:**
- Modify: `docs/manual/src/tools/file-operations.md`
- Modify: `docs/manual/src/tools/editing.md`

**Step 1: Write `file-operations.md`**

Document (standard format):
- `read_file`
- `list_dir`
- `search_for_pattern`
- `find_file`

Source material: `src/tools/file.rs` — `description()` and `input_schema()`.

**Step 2: Write `editing.md`**

Document (standard format):
- `create_text_file`
- `replace_content`
- `edit_lines`
- `replace_symbol_body` (cross-reference to symbol-navigation.md)
- `insert_before_symbol` (cross-reference)
- `insert_after_symbol` (cross-reference)
- `rename_symbol` (cross-reference)

Explain when to use each: symbol tools for code, `edit_lines` for non-code or intra-symbol edits, `replace_content` for simple text substitution.

Source material: `src/tools/file.rs`, `src/tools/symbol.rs`.

**Step 3: Build to verify**

Run: `mdbook build docs/manual`

**Step 4: Commit**

```bash
git add docs/manual/src/tools/file-operations.md docs/manual/src/tools/editing.md
git commit -m "docs: write tool reference — file operations and editing"
```

---

### Task 7: Write Tool Reference — Semantic Search, Git, AST

**Files:**
- Modify: `docs/manual/src/tools/semantic-search.md`
- Modify: `docs/manual/src/tools/git.md`
- Modify: `docs/manual/src/tools/ast.md`

**Step 1: Write `semantic-search.md`**

Document (standard format):
- `semantic_search`
- `index_project`
- `index_status`

Source material: `src/tools/semantic.rs`.

**Step 2: Write `git.md`**

Document (standard format):
- `git_blame`
- `git_log`
- `git_diff`

Source material: `src/tools/git.rs`.

**Step 3: Write `ast.md`**

Document (standard format):
- `list_functions`
- `extract_docstrings`

Source material: `src/tools/ast.rs`.

**Step 4: Build to verify**

Run: `mdbook build docs/manual`

**Step 5: Commit**

```bash
git add docs/manual/src/tools/semantic-search.md docs/manual/src/tools/git.md docs/manual/src/tools/ast.md
git commit -m "docs: write tool reference — semantic search, git, AST"
```

---

### Task 8: Write Tool Reference — Memory, Workflow & Config

**Files:**
- Modify: `docs/manual/src/tools/memory.md`
- Modify: `docs/manual/src/tools/workflow-and-config.md`

**Step 1: Write `memory.md`**

Document (standard format):
- `write_memory`
- `read_memory`
- `list_memories`
- `delete_memory`

Include guidance on when to use memory (project conventions, debugging insights, architectural decisions).

Source material: `src/tools/memory.rs`, `src/memory/`.

**Step 2: Write `workflow-and-config.md`**

Document (standard format):
- `onboarding`
- `check_onboarding_performed`
- `execute_shell_command`
- `activate_project`
- `get_current_config`

Source material: `src/tools/workflow.rs`, `src/tools/config.rs`.

**Step 3: Build to verify**

Run: `mdbook build docs/manual`

**Step 4: Commit**

```bash
git add docs/manual/src/tools/memory.md docs/manual/src/tools/workflow-and-config.md
git commit -m "docs: write tool reference — memory, workflow, config"
```

---

### Task 9: Write Configuration chapter

**Files:**
- Modify: `docs/manual/src/configuration/project-toml.md`
- Modify: `docs/manual/src/configuration/embedding-backends.md`

**Step 1: Write `project-toml.md`**

Cover:
- File location: `.code-explorer/project.toml`
- All configuration fields with defaults
- `[embeddings]` section: model, chunk_size, chunk_overlap
- Ignored paths configuration
- Project metadata

Source material: `src/config/project.rs` — read the `ProjectConfig` struct and `load_or_default()`.

**Step 2: Write `embedding-backends.md`**

Cover:
- Remote (default): OpenAI-compatible API format
- Ollama setup: install, pull model, configure
- OpenAI setup: API key, model selection
- Local (feature-gated): `cargo install code-explorer --features local-embed`, fastembed-rs, no API needed
- Comparison table: speed, quality, cost, privacy

Source material: `src/embed/remote.rs`, `src/embed/mod.rs`, `Cargo.toml` feature flags.

**Step 3: Build to verify**

Run: `mdbook build docs/manual`

**Step 4: Commit**

```bash
git add docs/manual/src/configuration/
git commit -m "docs: write configuration chapters"
```

---

### Task 10: Write Semantic Search Guide

**Files:**
- Modify: `docs/manual/src/semantic-search-guide.md`

**Step 1: Write content**

End-to-end walkthrough:
1. Choose an embedding backend (Ollama recommended for local dev)
2. Install Ollama and pull `nomic-embed-text`
3. Configure `.code-explorer/project.toml`
4. Build the index: `index_project` — what happens, how long it takes
5. Check index health: `index_status`
6. Search strategies: concept queries, comparing results, drilling down
7. Incremental indexing: what gets re-indexed on changes
8. Troubleshooting: API connection, empty results, model mismatch

Source material: `src/embed/`, `src/tools/semantic.rs`, `README.md` lines 209-224.

**Step 2: Build to verify**

Run: `mdbook build docs/manual`

**Step 3: Commit**

```bash
git add docs/manual/src/semantic-search-guide.md
git commit -m "docs: write semantic search guide"
```

---

### Task 11: Write Language Support page

**Files:**
- Modify: `docs/manual/src/language-support.md`

**Step 1: Write content**

Table format for each of the 9 LSP-supported languages:

| Language | LSP Server | Install Command | Tree-sitter | Notes |
|----------|-----------|----------------|-------------|-------|

Languages: Rust, Python, TypeScript/JavaScript, Go, Java, Kotlin, C/C++, C#, Ruby.

For each, cover:
- Which LSP binary is expected and how to install it
- Tree-sitter support level (full AST analysis, or LSP-only)
- Known quirks or limitations

Source material: `src/lsp/servers/mod.rs` (LSP configs), `src/ast/mod.rs` (tree-sitter language detection).

**Step 2: Build to verify**

Run: `mdbook build docs/manual`

**Step 3: Commit**

```bash
git add docs/manual/src/language-support.md
git commit -m "docs: write language support page"
```

---

### Task 12: Write Extending chapters

**Files:**
- Modify: `docs/manual/src/extending/adding-languages.md`
- Modify: `docs/manual/src/extending/writing-tools.md`
- Modify: `docs/manual/src/extending/tool-trait.md`

**Step 1: Write `adding-languages.md`**

Concrete walkthrough: "Add Ruby support in 3 steps":
1. Add LSP server config in `src/lsp/servers/`
2. Add tree-sitter grammar in `src/ast/`
3. Add language detection in `src/ast/mod.rs`

Source material: existing language configs as templates.

**Step 2: Write `writing-tools.md`**

Walkthrough of creating a new tool:
1. Create struct implementing `Tool` trait
2. Define `input_schema()` with schemars
3. Implement `call()` with error handling
4. Register in `src/tools/mod.rs`

Source material: any simple tool (e.g., `list_functions` in `src/tools/ast.rs`).

**Step 3: Write `tool-trait.md`**

Reference docs for the `Tool` trait:
- `name()`, `description()`, `input_schema()`, `call()`
- `OutputGuard` integration
- Error handling patterns (`CallToolResult::error`)
- The `#[async_trait]` requirement

Source material: `src/tools/mod.rs`, `src/tools/output.rs`.

**Step 4: Build to verify**

Run: `mdbook build docs/manual`

**Step 5: Commit**

```bash
git add docs/manual/src/extending/
git commit -m "docs: write extending chapters"
```

---

### Task 13: Write Architecture and Troubleshooting

**Files:**
- Modify: `docs/manual/src/architecture.md`
- Modify: `docs/manual/src/troubleshooting.md`

**Step 1: Write `architecture.md`**

Lighter version of `docs/ARCHITECTURE.md`:
- Component diagram
- Data flow: MCP request → Tool dispatch → backend → response
- Key crates and their roles
- Link to full `docs/ARCHITECTURE.md` for internals

Source material: `docs/ARCHITECTURE.md`.

**Step 2: Write `troubleshooting.md`**

Common issues:
- LSP not starting (binary not found, wrong version)
- Semantic search returns no results (index not built, API down)
- Tool not found in Claude Code (MCP server not registered)
- Slow responses (large project, no index, LSP initialization)
- Configuration not applied (wrong file location, syntax errors)
- Embedding API errors (connection refused, auth failure)

**Step 3: Build to verify**

Run: `mdbook build docs/manual`

**Step 4: Commit**

```bash
git add docs/manual/src/architecture.md docs/manual/src/troubleshooting.md
git commit -m "docs: write architecture and troubleshooting chapters"
```

---

### Task 14: Add GitHub Actions workflow

**Files:**
- Create: `.github/workflows/manual.yml`

**Step 1: Write the workflow**

```yaml
name: Manual

on:
  push:
    branches: [master]
    paths: [docs/manual/**]
  pull_request:
    paths: [docs/manual/**]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: taiki-e/install-action@mdbook
      - run: mdbook build docs/manual

  deploy:
    if: github.ref == 'refs/heads/master'
    needs: build
    runs-on: ubuntu-latest
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deploy.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
      - uses: taiki-e/install-action@mdbook
      - run: mdbook build docs/manual
      - uses: actions/upload-pages-artifact@v3
        with:
          path: target/manual
      - id: deploy
        uses: actions/deploy-pages@v4
```

**Step 2: Commit**

```bash
git add .github/workflows/manual.yml
git commit -m "ci: add GitHub Actions workflow for manual build and deploy"
```

---

### Task 15: Add tool sync check to CI

**Files:**
- Modify: `.github/workflows/ci.yml`

**Step 1: Add `tool-docs-sync` job**

Add to the existing CI workflow:

```yaml
  tool-docs-sync:
    name: Tool Docs Sync
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Check all tools are documented
        run: |
          grep -A1 'fn name(&self)' src/tools/*.rs | grep '"' | sed 's/.*"\(.*\)"/\1/' | sort > /tmp/code-tools.txt
          grep -roh '## `[a-z_]*`' docs/manual/src/tools/ | sed 's/## `\(.*\)`/\1/' | sort > /tmp/doc-tools.txt
          diff /tmp/code-tools.txt /tmp/doc-tools.txt || { echo "::error::Tool docs out of sync! Update docs/manual/src/tools/ to match src/tools/"; exit 1; }
```

**Step 2: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add tool documentation sync check"
```

---

### Task 16: Slim down README

**Files:**
- Modify: `README.md`

**Step 1: Edit README**

- Keep: The Problem, The Solution, Installation (steps 1-3), Supported Languages, Contributing, License
- Replace the full tool reference tables with a summary list + link: "See the [full tool reference](https://mareurs.github.io/code-explorer/tools/overview.html)"
- Replace architecture diagram with link to manual
- Add prominent "Read the Manual" link near the top

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: slim README, link to manual for details"
```

---

### Task 17: Final build and verify

**Step 1: Full build**

Run: `mdbook build docs/manual`
Expected: Clean build, no warnings

**Step 2: Local preview**

Run: `mdbook serve docs/manual --open`
Expected: Manual opens in browser, all chapters render, navigation works, links resolve

**Step 3: Run sync check locally**

```bash
grep -A1 'fn name(&self)' src/tools/*.rs | grep '"' | sed 's/.*"\(.*\)"/\1/' | sort > /tmp/code-tools.txt
grep -roh '## `[a-z_]*`' docs/manual/src/tools/ | sed 's/## `\(.*\)`/\1/' | sort > /tmp/doc-tools.txt
diff /tmp/code-tools.txt /tmp/doc-tools.txt
```

Expected: No diff (all 31 tools documented)

**Step 4: Run existing CI checks**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
Expected: All pass (manual changes shouldn't affect code)
