# Installation

> **Platform support:** code-explorer has been tested on **Linux**. macOS and Windows may work but have not been verified. Contributions welcome.

## Prerequisites

You need a working Rust toolchain. If you do not have one, install it via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Verify you have Rust 1.75 or later:

```bash
rustc --version
```

You also need [Claude Code](https://code.claude.com/) installed and available as `claude` on your PATH.

## Installing the Binary

Install code-explorer from crates.io:

```bash
cargo install code-explorer
```

This builds with the `remote-embed` feature enabled by default, which adds HTTP client support for
talking to an external embedding API (OpenAI-compatible, Ollama, etc.). If you want local CPU-based
embeddings without any external service, see [Feature Flags](#feature-flags) below.

## Registering as an MCP Server

Claude Code discovers MCP servers through its configuration. You can register code-explorer either
globally (applies to every Claude Code session on your machine) or per-project (applies only when
working in a specific directory tree).

### Global Registration

Run this once to make code-explorer available in every Claude Code session:

```bash
claude mcp add --global code-explorer -- code-explorer start --project .
```

With global registration, code-explorer starts automatically whenever Claude Code opens. The
`--project .` argument tells it to activate the project you opened Claude Code in.

### Per-Project Registration via `.mcp.json`

For tighter control — useful when different projects need different embedding backends or when you
are sharing configuration with a team — create or edit `.mcp.json` in the project root:

```bash
claude mcp add code-explorer -- code-explorer start --project /path/to/your/project
```

This writes an entry to `.mcp.json`. Commit the file so that everyone working on the project gets
the same code-explorer setup automatically.

A resulting `.mcp.json` entry looks like this:

```json
{
  "mcpServers": {
    "code-explorer": {
      "command": "code-explorer",
      "args": ["start", "--project", "/path/to/your/project"]
    }
  }
}
```

Replace `/path/to/your/project` with the absolute path to your repository root.

## Verification

After registering, confirm Claude Code sees the server and all its tools:

```bash
claude mcp list
```

You should see `code-explorer` listed with 31 tools. If it does not appear, make sure the
`code-explorer` binary is on your PATH:

```bash
which code-explorer
code-explorer --version
```

## Feature Flags

code-explorer has three embedding modes, controlled at compile time via Cargo features:

| Feature | What it does | When to use it |
|---|---|---|
| `remote-embed` (default) | HTTP client for OpenAI-compatible embedding APIs | You have Ollama, OpenAI, or a compatible server running |
| `local-embed` | CPU embeddings via fastembed-rs and ONNX Runtime | You want embeddings with no external service |
| both | Both backends compiled in; backend selected at runtime via config | Maximum flexibility |

### Installing with Local Embeddings

```bash
cargo install code-explorer --features local-embed
```

The first time you build a semantic search index, the selected model (typically
`nomic-embed-text`, ~130MB) downloads automatically to `~/.cache/huggingface/hub/`. Subsequent
uses are fully offline.

### Installing with Both Backends

```bash
cargo install code-explorer --features remote-embed,local-embed
```

Switch between backends per-project by setting `embed_backend` in `.code-explorer/project.toml`.
See [Embedding Backends](../configuration/embedding-backends.md) for details.

### Minimal Install (No Embeddings)

If you only want LSP-backed symbol navigation and git tools and do not need semantic search, you
can build without any embedding feature:

```bash
cargo install code-explorer --no-default-features
```

Semantic search tools (`semantic_search`, `index_project`) will return a clear error if called
without an embedding backend compiled in.
