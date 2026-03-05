# The Tool Trait

API reference for the `Tool` trait and its supporting types. For a tutorial
walkthrough, see [Writing Tools](writing-tools.md).

---

## The `Tool` Trait

Defined in `src/tools/mod.rs`:

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as exposed over MCP (e.g. "find_symbol").
    /// Must be unique across all registered tools.
    fn name(&self) -> &str;

    /// Short description shown to the LLM in the tool listing.
    /// Should be one or two sentences explaining what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's input parameters.
    /// Must return a valid JSON Schema object with at minimum a "type" field.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input.
    /// `input` is already parsed from the MCP request's JSON arguments.
    /// Returns a JSON value that will be serialized and sent to the LLM.
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;
}
```

### `name()`

Returns the tool's MCP identifier. This is the string the LLM uses to invoke
the tool. Must be unique across all registered tools — the
`tool_names_are_unique` test enforces this.

Convention: `snake_case`, matching the struct name in lowercase
(e.g. `FindSymbol` -> `"find_symbol"`).

### `description()`

A brief explanation shown in the MCP `list_tools` response. The LLM reads this
to decide which tool to use, so be precise about what the tool does and what it
does not do. Must not be empty.

### `input_schema()`

Returns a JSON Schema object describing the tool's parameters. Built with
`serde_json::json!()`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {
                "type": "string",
                "description": "File path (absolute or relative to project root)"
            },
            "detail_level": {
                "type": "string",
                "description": "Output detail: omit for compact, 'full' for complete"
            },
            "offset": {
                "type": "integer",
                "description": "Skip this many results (focused mode pagination)"
            },
            "limit": {
                "type": "integer",
                "description": "Max results per page (default 50)"
            }
        }
    })
}
```

Must have `"type": "object"` at the root. The `all_tools_have_valid_schemas`
test verifies this for every registered tool.

### `call()`

The main execution method. Receives the parsed JSON input and a `ToolContext`.
Returns `Result<Value>` — see [Error Handling](#error-handling) below.

---

## `ToolContext`

Defined in `src/tools/mod.rs`:

```rust
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<LspManager>,
}
```

A `ToolContext` is constructed fresh for each tool invocation in the server's
`call_tool()` handler. Both fields are cheaply cloneable (`Agent` wraps an
`Arc` internally).

### `agent: Agent`

The `Agent` holds the active project state. Key methods:

| Method | Returns | Description |
|--------|---------|-------------|
| `project_root().await` | `Option<PathBuf>` | Active project root, or `None` |
| `require_project_root().await?` | `Result<PathBuf>` | Same, but errors if no project |
| `security_config().await` | `PathSecurityConfig` | Path deny-list configuration |
| `with_project(\|proj\| { ... }).await?` | `Result<T>` | Access `ActiveProject` (config, memory) |
| `activate(path).await?` | `Result<()>` | Switch active project |

The `ActiveProject` struct (accessible via `with_project`) contains:

| Field | Type | Description |
|-------|------|-------------|
| `root` | `PathBuf` | Project root directory |
| `config` | `ProjectConfig` | Settings from `.codescout/project.toml` |
| `memory` | `MemoryStore` | Markdown-based key-value store |

### `lsp: Arc<LspManager>`

The `LspManager` manages LSP server lifecycles. Key methods:

| Method | Returns | Description |
|--------|---------|-------------|
| `get_or_start(lang, root).await?` | `Result<Arc<LspClient>>` | Get or launch an LSP server |
| `get(lang).await` | `Option<Arc<LspClient>>` | Get existing client without starting |
| `active_languages().await` | `Vec<String>` | Languages with running servers |
| `shutdown_all().await` | `()` | Stop all servers |

`get_or_start()` is the primary entry point. It starts a new server if none
exists, or returns the existing one if it is alive and pointed at the correct
workspace root. Dead or mismatched servers are automatically replaced.

---

## `OutputGuard`

The progressive disclosure system. Defined in `src/tools/output.rs`.

### `OutputMode`

```rust
pub enum OutputMode {
    /// Compact output, capped at max_results / max_files.
    Exploring,
    /// Full detail with offset/limit pagination.
    Focused,
}
```

### `OutputGuard` struct

```rust
pub struct OutputGuard {
    pub mode: OutputMode,
    pub max_files: usize,    // Default: 200
    pub max_results: usize,  // Default: 200
    pub offset: usize,       // Default: 0
    pub limit: usize,        // Default: 50
}
```

### `OutputGuard::from_input(input: &Value) -> Self`

Constructs an `OutputGuard` from a tool's JSON input by reading three optional
fields:

| Input field | Effect |
|-------------|--------|
| `detail_level: "full"` | Switches to `Focused` mode |
| `offset: N` | Sets pagination offset (default 0) |
| `limit: N` | Sets page size (default 50); also caps exploring mode when explicit |

Any other `detail_level` value (or omission) defaults to `Exploring` mode.

```rust
let guard = OutputGuard::from_input(&input);
```

### `guard.should_include_body() -> bool`

Returns `true` in `Focused` mode, `false` in `Exploring`. Use this to decide
whether to include source code bodies in symbol results.

### `guard.cap_items<T>(items: Vec<T>, hint: &str) -> (Vec<T>, Option<OverflowInfo>)`

Caps a list of items according to the active mode.

- **Exploring:** Keeps the first `max_results` items. If truncated, returns
  `OverflowInfo` with `next_offset: None`.
- **Focused:** Applies `offset`/`limit` pagination. If more pages remain,
  returns `OverflowInfo` with `next_offset: Some(offset + limit)`.

The `hint` parameter is a human-readable suggestion included in the overflow
metadata (e.g. `"Narrow with a path filter"` or `"Use offset/limit to
paginate"`).

Returns `(truncated_items, None)` if everything fits.

### `guard.cap_files<T>(files: Vec<T>, hint: &str) -> (Vec<T>, Option<OverflowInfo>)`

Same as `cap_items()` but uses `max_files` instead of `max_results` for the
exploring-mode cap. Use this for file-list results.

### `OutputGuard::overflow_json(info: &OverflowInfo) -> Value`

Serializes overflow metadata to JSON for inclusion in tool responses:

```json
{
  "shown": 200,
  "total": 1423,
  "hint": "Narrow with a path filter",
  "next_offset": 200
}
```

The `next_offset` field is only present in focused mode when more pages exist.

### `OverflowInfo`

```rust
pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,
    /// In focused mode, the offset for the next page (None in exploring mode).
    pub next_offset: Option<usize>,
}
```

### Typical usage pattern

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
    let guard = OutputGuard::from_input(&input);

    let all_results = do_expensive_work();

    let (results, overflow) = guard.cap_items(all_results, "Use offset/limit to paginate");

    let mut response = json!({
        "results": results,
    });

    if let Some(info) = overflow {
        response["overflow"] = OutputGuard::overflow_json(&info);
    }

    Ok(response)
}
```

---

## Error Handling

### Tool errors are content, not protocol errors

When a tool's `call()` returns `Err(e)`, the server does **not** return an MCP
protocol error. Instead, it wraps the error message in a `CallToolResult` with
`is_error: true`:

```rust
// From src/server.rs call_tool():
match tool.call(input, &ctx).await {
    Ok(output) => {
        let text = serde_json::to_string_pretty(&output)
            .unwrap_or_else(|_| output.to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
    Err(e) => {
        // Error surfaces to the LLM as text, not a protocol error
        Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
    }
}
```

This means the LLM sees the error message and can react to it (e.g., try a
different path, fix a parameter). Protocol-level errors (`McpError`) are only
used for truly invalid requests like unknown tool names.

### Error patterns

**Validation errors** — use `anyhow::bail!()` for clear, immediate failures:

```rust
if path_str.is_empty() {
    anyhow::bail!("path must not be empty");
}
```

**Propagation** — use `?` to propagate errors from I/O, LSP calls, etc.:

```rust
let content = std::fs::read_to_string(&path)?;
let client = ctx.lsp.get_or_start(lang, &root).await?;
```

**Missing required parameters:**

```rust
let path = input["path"]
    .as_str()
    .ok_or_else(|| anyhow::anyhow!("missing 'path' parameter"))?;
```

**Tool access control** — the server checks tool access before dispatching.
Restricted tools (like `run_command`) are blocked at the server level,
not inside the tool itself.

### What not to do

- Do not `panic!()` in tools. Panics crash the server process.
- Do not return `Err` for "no results found" — return an empty result set
  instead. Errors mean something went wrong, not that the result is empty.
- Do not return `McpError` from tools. That type is for the server layer only.

---

## The `#[async_trait]` Requirement

All tools must be `Send + Sync` because the MCP server is async and supports
multiple concurrent connections. The `Tool` trait enforces this:

```rust
pub trait Tool: Send + Sync { ... }
```

In practice, this means:

- Tool structs must not hold non-`Send` types (use `Arc<Mutex<_>>` if you need
  shared mutable state).
- Unit structs (`pub struct MyTool;`) are always `Send + Sync`.
- The `async fn call()` implementation uses `#[async_trait]` to enable async
  methods in the trait. This desugars to `Pin<Box<dyn Future + Send>>`.
- Holding a `MutexGuard` across an `.await` point will cause a compile error.
  Release the guard before awaiting.

---

## Tool Registration

Tools are registered in `src/server.rs` in the `from_parts()` method as a
`Vec<Arc<dyn Tool>>`. The server uses this vector for two things:

1. **`list_tools`** — iterates all tools and builds MCP `ToolInfo` from
   `name()`, `description()`, and `input_schema()`.
2. **`call_tool`** — looks up a tool by matching `tool.name()` against the
   request's tool name, then calls `tool.call()`.

There is no macro, attribute, or inventory system. Adding a tool is adding one
line to the vector.
