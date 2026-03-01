# Workflow & Config Tools

These tools manage the agent's working context: which project is active, whether it has been set up, how to run build and test commands, and how to inspect or change configuration.

## `onboarding`

**Purpose:** Perform initial project discovery — detect languages, list the top-level directory structure, create a default config file, and write a startup memory entry.

**Parameters:** None. Requires an active project (set one with `activate_project` first).
**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `force` | boolean | no | `false` | Re-run full discovery even if already onboarded |

Requires an active project (set one with `activate_project` first).
**Example:**

```json
{}
```

**Output:**

```json
{
  "languages": ["rust", "toml", "markdown"],
  "top_level": [
    ".code-explorer/",
    ".git/",
    "Cargo.lock",
    "Cargo.toml",
    "docs/",
    "src/",
    "tests/"
  ],
  "config_created": true,
  "instructions": "..."
}
```

`config_created` is `true` when `.code-explorer/project.toml` did not exist and was created by this call. The `instructions` field contains a prompt with guidance for working on this project — read it before starting work.

**Tips:** Call `onboarding` once per project, the first time you work on it. It writes a memory entry under the topic `"onboarding"` with a summary of what it found. On subsequent sessions, call `onboarding` with `force: false` (the default) — it detects previous onboarding and returns existing memories without re-running discovery.

---

## `run_command`

**Purpose:** Run a shell command in the active project root and return its stdout, stderr, and exit code.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `command` | string | yes | — | The shell command to run |
| `timeout_secs` | integer | no | `30` | Maximum seconds to wait before killing the process |

**Example:**

```json
{ "command": "cargo test --lib 2>&1 | tail -20", "timeout_secs": 120 }
```

**Output (success):**

```json
{
  "stdout": "running 42 tests\ntest result: ok. 42 passed; 0 failed",
  "stderr": "",
  "exit_code": 0,
  "warning": "Shell commands execute with full user permissions. Only use for build/test commands."
}
```

**Output (timeout):**

```json
{
  "timed_out": true,
  "stdout": "",
  "stderr": "Command timed out after 30 seconds",
  "exit_code": null
}
```

**Output (large output truncated):**

```json
{
  "stdout": "...(first 100KB)...",
  "stderr": "",
  "exit_code": 0,
  "stdout_truncated": true,
  "stdout_total_bytes": 524288,
  "warning": "Shell commands execute with full user permissions. Only use for build/test commands."
}
```

### Security

Shell execution is **disabled by default**. To enable it, add to `.code-explorer/project.toml`:

```toml
[security]
shell_command_mode = "warn"   # or "unrestricted"
```

The `shell_command_mode` setting controls behaviour:

| Value | Behaviour |
|-------|-----------|
| `"disabled"` | All calls return an error. This is the default. |
| `"warn"` | Commands execute; output includes a reminder about permissions. |
| `"unrestricted"` | Commands execute; no warning is added to the output. |

Output is capped at `shell_output_limit_bytes` (default 102400, i.e. 100 KB) per stream. When output is truncated, the response includes `stdout_truncated: true` and `stdout_total_bytes` so you know how much was omitted. Pipe through `tail`, `head`, or `grep` inside the command to stay within the limit for verbose commands.

On Unix the command runs under `sh -c`. On Windows it runs under `cmd /C`.

**Tips:** Use `run_command` for build, test, and lint commands where the output tells you whether your changes are correct. Increase `timeout_secs` for slow build steps like `cargo build` or full test suites. Pipe verbose output through `tail -N` to avoid hitting the output limit.

---

## `activate_project`

**Purpose:** Switch the active project to a different directory. All subsequent tool calls operate relative to the new project root.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | Absolute path to the project root directory |

**Example:**

```json
{ "path": "/home/user/projects/my-service" }
```

**Output:**

```json
{
  "status": "ok",
  "activated": {
    "project_root": "/home/user/projects/my-service",
    "config": {
      "project": {
        "name": "my-service",
        "languages": ["rust", "toml"],
        "encoding": "utf-8",
        "tool_timeout_secs": 60
      },
      "embeddings": { "model": "...", "chunk_size": 512, "chunk_overlap": 64 },
      "ignored_paths": { "patterns": ["target/", "*.lock"] },
      "security": { "shell_command_mode": "warn", "shell_output_limit_bytes": 102400, "shell_enabled": false, "file_write_enabled": true, "git_enabled": true, "indexing_enabled": true }
    }
  }
}
```

The tool returns an error if the path does not exist or is not a directory.

**Tips:** When working across multiple projects in a single session, call `activate_project` to switch between them. After activating, call `onboarding` to see whether the new project has been set up. The server starts with no active project — you must call `activate_project` (or have it activated via the `--project` CLI flag) before using any tool that requires a project context.

---

## `get_config`

**Purpose:** Display the active project root and the full contents of its configuration.

**Parameters:** None.

**Example:**

```json
{}
```

**Output:**

```json
{
  "project_root": "/home/user/projects/my-service",
  "config": {
    "project": {
      "name": "my-service",
      "languages": ["rust", "toml"],
      "encoding": "utf-8",
      "tool_timeout_secs": 60
    },
    "embeddings": {
      "model": "sentence-transformers/all-MiniLM-L6-v2",
      "chunk_size": 512,
      "chunk_overlap": 64
    },
    "ignored_paths": {
      "patterns": ["target/", "*.lock", ".git/"]
    },
    "security": {
      "shell_command_mode": "warn",
      "shell_output_limit_bytes": 102400,
      "shell_enabled": false,
      "file_write_enabled": true,
      "git_enabled": true,
      "indexing_enabled": true
    }
  }
}
```



---

## `get_usage_stats`

**Purpose:** Get tool call statistics for the current project. Returns per-tool call counts, error rates, overflow rates, and latency percentiles (p50/p99) for a time window. Use this to diagnose agent behavior: high `overflow_rate_pct` means queries are too broad; high `error_rate_pct` on a tool means it is failing repeatedly.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `window` | string | no | `"30d"` | Time window: `"1h"`, `"24h"`, `"7d"`, or `"30d"` |

**Example:**

```json
{ "window": "7d" }
```

**Output:**

```json
{
  "window": "7d",
  "tools": [
    {
      "tool_name": "semantic_search",
      "call_count": 142,
      "error_rate_pct": 0.0,
      "overflow_rate_pct": 12.7,
      "p50_ms": 210,
      "p99_ms": 890
    },
    {
      "tool_name": "find_symbol",
      "call_count": 98,
      "error_rate_pct": 2.0,
      "overflow_rate_pct": 4.1,
      "p50_ms": 35,
      "p99_ms": 180
    }
  ]
}
```

**Tips:**

- Use `get_usage_stats` to understand how an agent is using the tool set. A high `overflow_rate_pct` for `semantic_search` suggests the agent is querying too broadly — it should use `find_symbol` for known names instead.
- Compare error rates across windows to spot regressions after changes to the project or config.
- The stats are read from `.code-explorer/usage.db` which is populated by the usage recorder built into the MCP server. The database is created automatically on the first tool call.
- Stats are only available for the active project. Switch projects with `activate_project` before calling `get_usage_stats`.

> **See also:** [Dashboard](../concepts/dashboard.md) — the Tool Stats page
> shows the same data as `get_usage_stats` with charts, time-window filtering,
> and per-error inspection, without writing a tool call.
**Tips:** Use this to verify which project is active and to check security settings before attempting shell commands or indexing. If you need to change configuration, edit `.code-explorer/project.toml` directly — the config is re-read on each tool call, so changes take effect immediately without restarting the server.
