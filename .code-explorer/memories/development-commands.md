# Development Commands

See CLAUDE.md for primary commands. This supplements with gotchas.

## Build & Run
```bash
cargo build                          # debug build
cargo run -- start --project .       # run MCP server on stdio
cargo run -- index --project .       # build embedding index
cargo run -- dashboard --project .   # web UI (default port 8099)
```

## Features
```bash
cargo build --features local-embed   # local ONNX embeddings (fastembed)
cargo test --features e2e-rust       # E2E tests (requires rust-analyzer installed)
cargo test --features e2e            # all E2E (requires all LSP servers)
```

## LSP Server Install
```bash
./scripts/install-lsp.sh --check    # what's installed
./scripts/install-lsp.sh --all      # install all
./scripts/install-lsp.sh rust       # specific language
```

## Before Completing Work
1. `cargo fmt` — format
2. `cargo clippy -- -D warnings` — zero warnings required
3. `cargo test` — all tests must pass (baseline ~932)
4. Check `docs/TODO-tool-misbehaviors.md` — log any unexpected tool behavior encountered

## Gotchas
- `panic = "abort"` in release profile — panics kill the process immediately (intentional: prevents zombie MCP state)
- E2E tests need real LSP servers; they're excluded from default `cargo test`
- Dashboard is behind the `dashboard` feature (enabled by default)
- Remote embeddings need `OPENAI_API_KEY` or Ollama running; `local-embed` feature avoids this
