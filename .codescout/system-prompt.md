# codescout — Code Explorer Guidance

## Entry Points
- `src/server.rs:run()` — tool registration + MCP server startup
- `src/tools/mod.rs:Tool` (L209) — the central trait every capability implements
- `src/agent.rs:Agent` — orchestrator holding active project + config
- `src/tools/` — one file per tool category (file, symbol, semantic, memory, workflow, github, config)

## Key Abstractions
- **`Tool` trait** (`src/tools/mod.rs:209`) — `name`, `description`, `input_schema`, `async call(Value, &ToolContext)`
- **`ToolContext`** (`src/tools/mod.rs:47`) — agent + lsp + output_buffer + progress; injected into every call
- **`Agent`** (`src/agent.rs:33`) — `Arc<RwLock<AgentInner>>`, holds `ActiveProject` (root, config, memory)
- **`RecoverableError`** (`src/tools/mod.rs:67`) — expected failures → `isError: false`; `anyhow::bail!` → `isError: true`
- **`OutputGuard`** (`src/tools/output.rs`) — enforces Exploring/Focused mode; do not bypass

## Search Tips
- `semantic_search("progressive disclosure output")` — finds OutputGuard + format patterns
- `semantic_search("LSP symbol navigation")` — finds symbol.rs tool implementations
- `semantic_search("embedding index incremental")` — finds embed/ change detection
- `semantic_search("error routing recoverable")` — finds RecoverableError + route_tool_error
- Avoid: "tool", "server", "error" alone (too broad); prefer compound terms

## Navigation Strategy
For any new task:
1. `memory(action="read", topic="architecture")` — orient
2. `list_symbols("src/tools/<relevant_file>.rs")` — see what's in the tool file
3. `semantic_search("your concept")` — find related code
4. `find_symbol("Name", include_body=true)` — read implementation
5. Check `docs/TODO-tool-misbehaviors.md` before starting

## Project Rules
- Read `docs/PROGRESSIVE_DISCOVERABILITY.md` before adding or modifying any tool
- Write tools must return `json!("ok")` — never echo input back
- When renaming tools: update all 3 prompt surfaces (`server_instructions.md`, `onboarding_prompt.md`, `build_system_prompt_draft()`)
- Use `RecoverableError` for bad input; `anyhow::bail!` only for genuine crashes
- `cargo fmt && cargo clippy -- -D warnings && cargo test` before completing any task
- Log unexpected tool behavior in `docs/TODO-tool-misbehaviors.md` immediately
