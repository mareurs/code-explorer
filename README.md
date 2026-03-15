# codescout
MCP server giving AI coding agents IDE-grade code intelligence — symbol navigation,
semantic search, persistent memory — optimized for token efficiency.

Works with Claude Code, GitHub Copilot, Cursor, and any MCP-capable agent.

## What it does

- **Symbol navigation** — `find_symbol`, `list_symbols`, `find_references`, `goto_definition`, `replace_symbol`, backed by LSP across 9 languages
- **Semantic search** — find code by concept using embeddings, not grep
- **Library navigation** — explore dependency source code with scoped search, version tracking, and auto-discovery
- **Multi-project workspaces** — register related projects in `workspace.toml` for cross-project navigation with per-project memory and indexing
- **Token efficiency** — compact by default, details on demand, never dumps full files

## Why not just read files?

| Without codescout | With codescout |
|---|---|
| Agent reads full files to find one function | Navigates by symbol name — zero file reads |
| `grep` returns noise (comments, strings, docs) | `find_references` returns exact call sites |
| Context burns on navigation overhead | Token-efficient by design — compact by default |
| State lost between sessions | Persistent memory across sessions |
| Re-reads same modules from different entry points | Symbol index built once, queried instantly |

## Quick start

```bash
cargo install codescout
```

Then register as an MCP server. Example config:

```json
{
  "mcpServers": {
    "codescout": {
      "command": "codescout",
      "args": ["start"]
    }
  }
}
```

Config file locations:
- **Claude Code:** `~/.claude/settings.json`
- **Cursor:** `.cursor/mcp.json` (uses `"mcpServers"` key)
- **VS Code/Copilot:** `~/.config/Code/User/mcp.json` (uses `"servers"` key instead of `"mcpServers"`)

→ [Full installation guide](docs/manual/src/getting-started/installation.md)

## First run: onboarding

After registering, **run onboarding once per project** — ask your agent:

```
Run codescout onboarding
```

This starts LSP servers, detects your languages and entry points, and generates the system prompt injected into every future session. **Without this step, codescout's tool guidance won't load and LSP tools will error.**

## Agent integrations

| Agent | Guide |
|---|---|
| Claude Code | [docs/agents/claude-code.md](docs/agents/claude-code.md) |
| GitHub Copilot | [docs/agents/copilot.md](docs/agents/copilot.md) |
| Cursor | [docs/agents/cursor.md](docs/agents/cursor.md) |

## Multi-agent infrastructure

> codescout's design is informed by research on compound error in multi-agent systems — research and empirical evidence confirm failure rates of 41–87% in production pipelines. This finding drove the choice of single-session skill-based workflows over agent orchestration chains. [Read the analysis →](docs/research/multi-agent-context-loss.md)

## Tools (29)

`Symbol navigation (9)` · `File operations (6)` · `Semantic search (3)` · `Memory (1)` · `Library navigation (1)` · `Workflow (2)` · `Config (2)` · `GitHub (5)`

Supported languages: Rust, Python, TypeScript/JavaScript, Go, Java, Kotlin, C/C++, C#, Ruby.

→ [Tool reference](docs/manual/src/tools/overview.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to get started. PRs from Claude Code are welcome!

## Features

- Multi-project workspace support with per-project LSP, memory, and semantic indexing
- Library navigation with per-library embedding databases and version staleness hints
- LSP idle TTL — idle language servers are shut down automatically (Kotlin: 2h, others: 30min) and restarted transparently on next query
- Persistent memory across sessions with semantic recall
- Output buffers (`@cmd_*`, `@file_*`) for token-efficient large output handling
- Progressive disclosure — compact by default, full detail on demand

## License

[MIT](LICENSE)
