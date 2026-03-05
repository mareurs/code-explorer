# codescout — Quick Start (No Clone Required)

codescout is an MCP server that gives Claude Code IDE-grade code intelligence: symbol
navigation, semantic search, git blame, and persistent memory. This guide gets you running
without cloning the repository.

---

## Prerequisites

### 1. Rust toolchain

```bash
# Install rustup if you don't have it
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify Rust 1.75+
rustc --version
```

### 2. Claude Code

Install from [code.claude.com](https://code.claude.com/) and verify it is on your PATH:

```bash
claude --version
```

---

## Install the Binary

The crate name `codescout` on crates.io is taken by an unrelated project. Install directly
from the GitHub repository:

```bash
cargo install --git https://github.com/mareurs/codescout
```

Verify the install:

```bash
codescout --version
```

---

## Register as an MCP Server

### Option A — Global (recommended for personal use)

Registers codescout for every Claude Code session on your machine:

```bash
claude mcp add --global codescout -- codescout start --project .
```

### Option B — Per-project via `.mcp.json`

Useful when you want to share the config with your team. Run this inside
the project root:

```bash
claude mcp add codescout -- codescout start --project /absolute/path/to/your/project
```

This writes a `.mcp.json` file. Commit it so every teammate gets codescout automatically
when they open the repo in Claude Code.

### Verify registration

```bash
claude mcp list
# should show: codescout  (23 tools)
```

---

## Install LSP Servers

LSP servers power symbol navigation (`find_symbol`, `list_symbols`, `goto_definition`, etc.).
Grab the install script from the repo without cloning:

```bash
# Download and run the installer
curl -fsSL https://raw.githubusercontent.com/mareurs/codescout/main/scripts/install-lsp.sh -o install-lsp.sh
chmod +x install-lsp.sh

./install-lsp.sh --check           # see what's installed
./install-lsp.sh --all             # install everything
./install-lsp.sh rust python go    # install specific languages only
```

Supported languages: Rust, Python, TypeScript/JavaScript, Go, Java, Kotlin, C/C++, C#, Ruby.

---

## Set Up Semantic Search (Optional)

Semantic search lets Claude find code by concept ("error handling", "authentication flow") rather
than by name. It requires an embedding backend.

### Option A — Ollama (fully local, recommended)

```bash
# Install Ollama from https://ollama.com then pull a model
ollama pull mxbai-embed-large
```

Then create `.codescout/project.toml` in your project root:

```toml
[embeddings]
model = "ollama:mxbai-embed-large"
```

### Option B — OpenAI

```toml
[embeddings]
model = "openai:text-embedding-3-small"
api_key = "sk-..."
```

### Build the index

Open a Claude Code session in your project and ask:

```
Run index_project to build the semantic search index.
```

Or call it directly:

```json
{ "name": "index_project", "arguments": {} }
```

Indexing takes 1–3 minutes for a ~100k line project. It is incremental — subsequent runs only
re-embed changed files.

---

## Install the Routing Plugin (Recommended)

The routing plugin automatically steers Claude toward codescout tools and away from `grep`,
`cat`, and `Read` — including inside subagents which otherwise fall back to built-ins.

```
/plugin marketplace add mareurs/sdd-misc-plugins
/plugin install code-explorer-routing@sdd-misc-plugins
```

Or add to `~/.claude/settings.json`:

```json
{
  "enabledPlugins": {
    "code-explorer-routing@sdd-misc-plugins": true
  }
}
```

---

## First Session

Open a terminal in your project and start Claude Code:

```bash
cd /your/project
claude
```

Then ask Claude:

```
Run onboarding to explore this project.
```

Onboarding discovers the project structure, detects languages, and writes memory entries so
future sessions start with context already loaded.

---

## Tool Quick Reference

| Goal | Tool |
|---|---|
| Explore directory structure | `list_dir` |
| List all symbols in a file or dir | `list_symbols` |
| Find a function by name | `find_symbol` with `include_body: true` |
| Find code by concept | `semantic_search` |
| Search by text / regex | `search_pattern` |
| Jump to a definition | `goto_definition` |
| Find all usages | `find_references` |
| Rename a symbol everywhere | `rename_symbol` |
| Replace a function body | `replace_symbol` |
| View git blame | `run_command("git blame <file>")` |
| Save a project note | `memory` with `action: "write"` |

---

## Troubleshooting

**`codescout` not found after install**

Make sure `~/.cargo/bin` is on your PATH:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

**Claude Code doesn't see the server**

```bash
claude mcp list   # confirm it's registered
which codescout   # confirm the binary is findable
```

**LSP tools return no results**

The LSP server for your language may not be installed. Run:
```bash
./install-lsp.sh --check
```

**`semantic_search` returns nothing**

The embedding index hasn't been built yet. Run `index_project` first, then verify with
`project_status`.
