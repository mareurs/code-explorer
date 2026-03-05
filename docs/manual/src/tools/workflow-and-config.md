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
    ".codescout/",
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

`config_created` is `true` when `.codescout/project.toml` did not exist and was created by this call. The `instructions` field contains a prompt with guidance for working on this project — read it before starting work.

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

> See [Security & Permissions](../concepts/security.md) for the full permission model, including write sandboxing and the built-in credential deny list.

Shell execution is **disabled by default**. To enable it, add to `.codescout/project.toml`:

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

## `project_status`

**Purpose:** Display the full state of the active project in one call: config, semantic index health, usage telemetry summary, and library registry. Combines what was previously `get_config` and `index_status`.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `threshold` | number | no | — | When provided, includes drift data: minimum `avg_drift` to include (0.0–1.0) |
| `path` | string | no | — | SQL LIKE pattern to filter drift results by file path (e.g. `"src/tools/%"`) |
| `detail_level` | string | no | `"exploring"` | Drift output detail: `"full"` includes most-drifted chunk content |
| `window` | string | no | `"30d"` | Time window for usage summary: `"1h"`, `"24h"`, `"7d"`, or `"30d"` |

**Example (basic):**

```json
{}
```

**Example (with drift query):**

```json
{ "threshold": 0.2, "window": "7d" }
```

**Output:**

```json
{
  "project_root": "/home/user/projects/my-service",
  "config": {
    "project": { "name": "my-service", "languages": ["rust", "toml"] },
    "embeddings": { "model": "sentence-transformers/all-MiniLM-L6-v2" }
  },
  "index": {
    "indexed": true,
    "files": 47,
    "chunks": 312,
    "model": "sentence-transformers/all-MiniLM-L6-v2",
    "last_updated": "2025-01-15T10:30:00Z"
  },
  "libraries": { "count": 2, "indexed": 1 }
}
```

**Tips:**

- Use `project_status` to verify which project is active and to check security settings before attempting shell commands or indexing.
- Pass `threshold: 0.1` after re-indexing to surface files that changed semantically — a whitespace reformat scores near `0.0`, a full function rewrite approaches `1.0`.
- If you need to change configuration, edit `.codescout/project.toml` directly — the config is re-read on each tool call, so changes take effect immediately without restarting the server.
- For full per-tool call stats with charts and time-window filtering, see the [Dashboard](../concepts/dashboard.md).
