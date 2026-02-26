# code-explorer

Rust MCP server giving LLMs IDE-grade code intelligence.

## The Problem

LLMs waste most of their context window on code navigation. `grep` returns walls of text. `cat` dumps entire files when you need one function. There's no way to ask "who calls this?" or "what changed here last?" — the tools are blind to code structure.

The result: shallow understanding, hallucinated edits, constant human course-correction.

## The Solution

code-explorer is an MCP server that gives your AI coding agent the same navigation tools a human developer uses in an IDE — but optimized for token efficiency.

**Four pillars:**

| Pillar | What it does | Tools |
|---|---|---|
| LSP Navigation | Go-to-definition, find references, rename — via real language servers | 7 tools, 9 languages |
| Semantic Search | Find code by concept, not just text match — via embeddings | 4 tools |
| Git Integration | Blame, history, diffs — context no other tool provides | 3 tools |
| Persistent Memory | Remember project knowledge across sessions | 4 tools |

Plus file operations (7 tools), AST analysis (2 tools), workflow (3 tools), config (2 tools), and library navigation (2 tools) — **33 tools total**.

**Recent additions:**
- **Library Search** — navigate third-party dependency source code via LSP-inferred discovery, symbol navigation, and semantic search. Libraries auto-register when `goto_definition` returns paths outside the project root.
- **Incremental Index Rebuilding** — smart change detection for the embedding index. Uses git diff → mtime → SHA-256 fallback chain to skip unchanged files, with staleness warnings when the index falls behind HEAD.
- **Semantic Drift Detection** — detects *how much* code changed in meaning after re-indexing, not just that bytes changed. Useful for filtering doc staleness and understanding the scope of a refactor. Opt out with `drift_detection_enabled = false` in `[embeddings]`.

## Platform Support

Tested on **Linux**. macOS and Windows may work but have not been verified. Contributions welcome.

## Installation

code-explorer has two components that work together:

1. **MCP Server** — provides the 33 tools (symbol navigation, semantic search, git, etc.)
2. **Routing Plugin** — ensures Claude always uses the right tool, across all sessions and subagents

**Both are recommended.** The MCP server gives Claude the capability; the plugin ensures
that capability is always used correctly. Without the plugin, Claude will occasionally
fall back to `grep`/`cat`/`read` out of habit — especially in subagents that start with
a blank slate.

### Step 1: Install the MCP server

```bash
cargo install code-explorer
```

Register it globally so it's available in every Claude Code session:

```bash
claude mcp add --global code-explorer -- code-explorer start --project .
```

Or per-project (add to your project's `.mcp.json`):

```bash
claude mcp add code-explorer -- code-explorer start --project /path/to/your/project
```

### Step 2: Install the routing plugin

```bash
claude /plugin install code-explorer-routing@sdd-misc-plugins
```

Or add to your user settings (`~/.claude/settings.json`) for all sessions:

```json
{
  "enabledPlugins": {
    "code-explorer-routing@sdd-misc-plugins": true
  }
}
```

The plugin is available from the [claude-plugins marketplace](https://github.com/mareurs/claude-plugins).

### Step 3: Verify

```bash
claude mcp list
# Should show: code-explorer with 33 tools
```

### How They Interact

```
┌─────────────────────────────────────────────────────┐
│                   Claude Code                        │
│                                                      │
│  ┌─────────────────────────────────────────────┐    │
│  │  code-explorer-routing plugin (hooks)        │    │
│  │                                              │    │
│  │  SessionStart  → inject tool selection guide │    │
│  │  SubagentStart → propagate to all subagents  │    │
│  │  PreToolUse    → redirect grep/cat/read to   │    │
│  │                  code-explorer equivalents    │    │
│  └──────────────────────┬──────────────────────┘    │
│                         │ routes to                   │
│  ┌──────────────────────▼──────────────────────┐    │
│  │  code-explorer MCP server (33 tools)         │    │
│  │                                              │    │
│  │  LSP · Semantic · Git · AST · Memory · ...   │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

**Without the plugin:** Claude has access to the tools but may not use them
optimally — it might read whole files instead of using `find_symbol`, or grep
instead of `semantic_search`.

**With the plugin:** Every session and subagent gets automatic guidance on which
tool to use for each situation. The `PreToolUse` hook actively intercepts
suboptimal tool calls and redirects them before they execute.

## Tools (33)

| Category | Count | Highlights |
|---|---|---|
| Symbol Navigation | 7 | `find_symbol`, `get_symbols_overview`, `find_referencing_symbols`, `rename_symbol` |
| File Operations | 7 | `read_file`, `list_dir`, `search_for_pattern`, `create_text_file` |
| Semantic Search | 4 | `semantic_search`, `index_project`, `index_status`, `check_drift` |
| Library Navigation | 2 | `list_libraries`, `index_library` |
| Git | 3 | `git_blame`, `git_log`, `git_diff` |
| AST Analysis | 2 | `list_functions`, `extract_docstrings` (offline, instant) |
| Memory | 4 | `write_memory`, `read_memory`, `list_memories`, `delete_memory` |
| Workflow & Config | 4 | `onboarding`, `execute_shell_command`, `activate_project` |

Every tool defaults to compact output (exploring mode) and supports `detail_level: "full"` with pagination for when you need the complete picture.

See the [full tool reference](docs/manual/src/tools/overview.md) for parameters, examples, and usage guidance.

## Supported Languages

| | Languages |
|---|---|
| **Full** (LSP + tree-sitter) | Rust, Python, TypeScript, TSX, Go, Java, Kotlin |
| **LSP only** | JavaScript, JSX, C, C++, C#, Ruby |
| **Detection only** | PHP, Swift, Scala, Elixir, Haskell, Lua, Bash, Markdown |

See [Language Support](docs/manual/src/language-support.md) for install commands and known quirks.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to get started. PRs from Claude Code are welcome!

## License

[MIT](LICENSE)
