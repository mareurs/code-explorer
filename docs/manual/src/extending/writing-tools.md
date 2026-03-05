# Writing Tools

This walkthrough creates a new tool from scratch. We will build a hypothetical
`word_count` tool that counts words in a file, then register it with the MCP
server. The same pattern applies to every tool in the codebase.

---

## Step 1: Create the tool struct

Each tool is a unit struct. Create a new file or add to an existing tool module
(e.g. `src/tools/file.rs` for file-related tools):

```rust
pub struct WordCount;
```

That is it. Tools carry no state — all runtime state lives in `ToolContext`,
which is passed to every `call()`.

---

## Step 2: Implement the Tool trait

The `Tool` trait lives in `src/tools/mod.rs`:

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as exposed over MCP (e.g. "find_symbol")
    fn name(&self) -> &str;

    /// Short description shown to the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input (already parsed from JSON)
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;
}
```

Here is the complete implementation for `WordCount`:

```rust
use async_trait::async_trait;
use anyhow::Result;
use serde_json::{json, Value};
use crate::tools::{Tool, ToolContext};
use crate::tools::output::OutputGuard;

pub struct WordCount;

#[async_trait]
impl Tool for WordCount {
    fn name(&self) -> &str {
        "word_count"
    }

    fn description(&self) -> &str {
        "Count words in a file. Returns total word count and line count."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (absolute or relative to project root)"
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        // 1. Read and validate parameters
        let path_str = input["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' parameter"))?;

        // 2. Resolve and security-validate the path
        let project_root = ctx.agent.project_root().await;
        let security = ctx.agent.security_config().await;
        let path = crate::util::path_security::validate_read_path(
            path_str,
            project_root.as_deref(),
            &security,
        )?;

        // 3. Do the actual work
        if !path.exists() {
            anyhow::bail!("File not found: {}", path.display());
        }
        let content = std::fs::read_to_string(&path)?;
        let word_count = content.split_whitespace().count();
        let line_count = content.lines().count();

        // 4. Return results as JSON
        Ok(json!({
            "file": path.display().to_string(),
            "words": word_count,
            "lines": line_count,
        }))
    }
}
```

### Key patterns in `call()`

**Parameter extraction.** The `input` value is already parsed JSON. Use
`input["field"].as_str()`, `.as_u64()`, `.as_bool()`, etc. Always handle
missing required fields with a clear error message.

**Path resolution.** For any tool that reads files, always validate through
`validate_read_path()` (or `validate_write_path()` for write tools). This
resolves relative paths against the project root and blocks access to
sensitive system directories.

```rust
let project_root = ctx.agent.project_root().await;
let security = ctx.agent.security_config().await;
let path = crate::util::path_security::validate_read_path(
    path_str,
    project_root.as_deref(),
    &security,
)?;
```

**Using `ctx.agent`.** The agent provides project state:
- `ctx.agent.project_root().await` — active project root (`Option<PathBuf>`)
- `ctx.agent.require_project_root().await?` — same but returns an error if no project is active
- `ctx.agent.security_config().await` — path security configuration
- `ctx.agent.with_project(|proj| { ... }).await?` — access config, memory store

**Using `ctx.lsp`.** The LSP manager provides language server access:
- `ctx.lsp.get_or_start(language, workspace_root).await?` — get or launch an LSP client
- `ctx.lsp.get(language).await` — get an existing client without starting one

**Error handling.** Use `anyhow::bail!()` for validation errors and `?` for
propagating internal errors. The server catches all errors and surfaces them
to the LLM as text content (see [The Tool Trait](tool-trait.md) for details).

**Return value.** Always return `Ok(json!({...}))`. The server serializes this
to pretty-printed JSON and wraps it in an MCP `CallToolResult`.

---

## Step 3: Register the tool

Add the tool to the tool vector in `src/server.rs` in the `from_parts()`
method:

```rust
pub async fn from_parts(agent: Agent, lsp: Arc<LspManager>) -> Self {
    // ...
    let tools: Vec<Arc<dyn Tool>> = vec![
        // File tools
        Arc::new(ReadFile),
        Arc::new(ListDir),
        Arc::new(SearchForPattern),
        Arc::new(CreateTextFile),
        Arc::new(FindFile),
        Arc::new(ReplaceContent),
        Arc::new(EditLines),
        // ... other categories ...
        // Add your tool:
        Arc::new(WordCount),
    ];
    // ...
}
```

Tools are dispatched dynamically by name. The `list_tools` handler iterates
this vector to build the MCP tool list, and `call_tool` looks up tools by
matching `tool.name()` against the request. No other registration is needed.

Remember to add the `use` import at the top of `server.rs`:

```rust
use crate::tools::file::WordCount;  // or wherever you placed it
```

---

## Step 4: Using OutputGuard

If your tool returns a list of items that could be large, integrate the
progressive disclosure system. `OutputGuard` enforces two modes:

- **Exploring** (default): caps output at 200 items, no pagination
- **Focused** (`detail_level: "full"`): paginated with `offset`/`limit`

Add the standard parameters to your schema:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {
                "type": "string",
                "description": "File path or directory"
            },
            "detail_level": {
                "type": "string",
                "description": "Output detail: omit for compact (default), 'full' for complete"
            },
            "offset": {
                "type": "integer",
                "description": "Skip this many results (focused mode pagination)"
            },
            "limit": {
                "type": "integer",
                "description": "Max results per page (focused mode, default 50)"
            }
        }
    })
}
```

Then use `OutputGuard` in your `call()`:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
    let guard = OutputGuard::from_input(&input);

    // ... collect all results ...
    let all_items: Vec<SomeType> = do_work();

    // Cap the output according to the active mode
    let (items, overflow) = guard.cap_items(all_items, "Narrow with a path filter");

    let mut result = json!({
        "items": items,
        "total": items.len(),
    });

    // Attach overflow metadata so the LLM knows there is more
    if let Some(info) = overflow {
        result["overflow"] = OutputGuard::overflow_json(&info);
    }

    Ok(result)
}
```

The overflow JSON tells the LLM how many results exist and how to get the next
page:

```json
{
  "shown": 200,
  "total": 1423,
  "hint": "Narrow with a path filter",
  "next_offset": 200
}
```

Use `cap_items()` for result lists and `cap_files()` for file lists. The
semantics are the same; the distinction exists so you can configure different
caps for items vs files if needed.

Use `guard.should_include_body()` to decide whether to include full source
bodies in symbol results:

```rust
if guard.should_include_body() {
    // Include the "body" field with source text
}
```

See [The Tool Trait](tool-trait.md) for the full `OutputGuard` API reference.

---

## Step 5: Testing

Tools are tested by constructing a `ToolContext` with a test agent and calling
`tool.call()` directly. The pattern from `src/server.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::sync::Arc;

    async fn make_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let lsp = Arc::new(LspManager::new());
        let ctx = ToolContext { agent, lsp };
        (dir, ctx)
    }

    #[tokio::test]
    async fn word_count_basic() {
        let (dir, ctx) = make_ctx().await;

        // Create a test file
        let test_file = dir.path().join("hello.txt");
        std::fs::write(&test_file, "hello world\nfoo bar baz\n").unwrap();

        let tool = WordCount;
        let result = tool.call(
            json!({ "path": test_file.to_str().unwrap() }),
            &ctx,
        ).await.unwrap();

        assert_eq!(result["words"], 5);
        assert_eq!(result["lines"], 2);
    }

    #[tokio::test]
    async fn word_count_missing_file() {
        let (_dir, ctx) = make_ctx().await;

        let tool = WordCount;
        let result = tool.call(
            json!({ "path": "/nonexistent/file.txt" }),
            &ctx,
        ).await;

        assert!(result.is_err());
    }
}
```

Run with:

```bash
cargo test word_count
```

The server-level tests in `src/server.rs` also verify invariants across all
registered tools:
- `server_registers_all_tools` — checks that every tool appears in `list_tools`
- `tool_names_are_unique` — no two tools share a name
- `all_tools_have_valid_schemas` — every schema is valid JSON with a `type` field
- `all_tools_have_descriptions` — no empty descriptions

These run automatically when you add your tool to `from_parts()`.

---

## Summary

The full recipe:

1. Create a struct: `pub struct MyTool;`
2. Implement `Tool` with `name()`, `description()`, `input_schema()`, `call()`
3. Register in `from_parts()` with `Arc::new(MyTool)`
4. Add `OutputGuard` if the tool returns unbounded lists
5. Write tests against `ToolContext`

For the API reference of `Tool`, `ToolContext`, `OutputGuard`, and error
handling, see [The Tool Trait](tool-trait.md).
