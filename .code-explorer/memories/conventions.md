# Conventions

## Naming
| Entity | Convention | Example |
|---|---|---|
| Tool structs | PascalCase, suffix `Tool` optional | `FindSymbolTool`, `RunCommandTool` |
| Tool name strings | `snake_case` | `"find_symbol"`, `"run_command"` |
| Error types | `RecoverableError` for expected; `anyhow` for crashes | — |
| LSP lang configs | one struct per language in `src/lsp/` | `RustLspConfig` |
| Memory topics | `kebab-case` | `"project-overview"`, `"dev-commands"` |

## Error Handling Pattern
```rust
// Input/recoverable: use RecoverableError
return Err(RecoverableError::with_hint("path not found", "check the path exists").into());

// Fatal: use anyhow::bail
anyhow::bail!("LSP process crashed: {}", e);
```

## Tool Implementation Pattern
```rust
// Each tool is a unit struct implementing Tool
pub struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "..." }
    fn input_schema(&self) -> Value { json!({ "type": "object", "properties": {...} }) }
    async fn call(&self, params: Value, ctx: &ToolContext) -> Result<Value> { ... }
}
```

## Output Rules
- **Exploring mode (default)**: compact, capped at 200 items, `by_file` overflow hint
- **Focused mode**: `detail_level: "full"` + offset/limit pagination
- **Write tools return**: `json!("ok")` only — never echo path, content, or size back
- See `docs/PROGRESSIVE_DISCOVERABILITY.md` for canonical patterns

## Testing
- Framework: Rust built-in (`#[test]`, `#[tokio::test]`)
- E2E tests gated behind feature flags: `cargo test --features e2e-rust` etc.
- Unit tests in-file in `mod tests {}` blocks
- Cache-invalidation: three-query sandwich (baseline → stale assert → fresh after invalidation)
- Reference example: `did_change_refreshes_stale_symbol_positions` in `src/lsp/client.rs`

## Three Prompt Surfaces (keep in sync)
When renaming/adding tools, update ALL three:
1. `src/prompts/server_instructions.md` — every MCP request
2. `src/prompts/onboarding_prompt.md` — one-time onboarding
3. `build_system_prompt_draft()` in `src/tools/workflow.rs` — generated per-project
