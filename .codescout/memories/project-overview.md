# codescout

## Purpose
Rust MCP server giving LLMs IDE-grade code intelligence: symbol-level navigation via LSP, semantic search via embeddings, persistent memory, shell integration, and GitHub integration. Built as a Claude Code companion — 28 tools total.

## Tech Stack
- **Language:** Rust (edition 2021, MSRV 1.75)
- **MCP SDK:** rmcp 0.1 (stdio + SSE transports)
- **Database:** SQLite (rusqlite bundled + sqlite-vec for cosine similarity)
- **LSP protocol:** lsp-types 0.97, JSON-RPC over stdio child process
- **AST:** tree-sitter (Rust, Python, Go, TypeScript, Java, Kotlin)
- **Async runtime:** Tokio full
- **Key deps:** git2 (git blame/log), anyhow/thiserror, clap, serde_json, reqwest (remote embeddings), fastembed (local embeddings, optional), axum (dashboard, optional)

## Features (28 tools)
| Category | Count |
|---|---|
| Symbol Navigation (LSP-backed) | 9 |
| File Operations | 6 |
| Semantic Search | 2 |
| Memory | 1 |
| Workflow (onboarding, run_command) | 2 |
| Config & Navigation | 3 |
| GitHub | 5 |

## Runtime Requirements
- Rust toolchain (cargo build)
- Optional: LSP servers installed per language (rust-analyzer, pyright, etc.) — `./scripts/install-lsp.sh`
- Optional: Ollama or OpenAI-compatible API for remote embeddings; or fastembed feature for local
- Dashboard: enabled by default via `dashboard` feature flag
