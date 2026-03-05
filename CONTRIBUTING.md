# Contributing to codescout

We welcome contributions! Whether it's a bug fix, new language support, or documentation improvement — we're happy to review it.

## Getting Started

```bash
git clone https://github.com/mareurs/codescout.git
cd codescout
cargo build
cargo test
```

## Before Submitting a PR

Run the same checks CI will run:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## What to Contribute

**Good first contributions:**
- Add a tree-sitter grammar for a new language (see `src/ast/`)
- Add an LSP server config for a new language (see `src/lsp/servers/`)
- Fix a bug
- Improve documentation

**Please open an issue first for:**
- Large architectural changes
- New tool categories
- Changes to the progressive disclosure design

## Using Claude Code?

PRs generated with Claude Code are welcome. Just mention it in the PR description. If you're using codescout itself as an MCP server while contributing to codescout — that's the dream. Let us know how it went.

## Project Structure

See [CLAUDE.md](CLAUDE.md) for the full developer guide, including project structure, design principles, and key patterns. That file is also what Claude Code reads when working on this project.
