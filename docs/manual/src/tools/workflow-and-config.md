# Workflow & Config Tools

These tools manage the agent's working context: which project is active, whether it has been set up, how to run build and test commands, and how to inspect or change configuration.

## `onboarding`

**Purpose:** Perform initial project discovery — detect languages, list the top-level directory structure, create a default config file, and write a startup memory entry.

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
  "hardware": {
    "ollama_available": true,
    "ollama_host": "http://localhost:11434",
    "gpu": { "vendor": "nvidia", "name": "RTX 3080", "vram_mb": 10240 },
    "ram_gb": 32,
    "cpu_cores": 16
  },
  "model_options": [
    { "id": "ollama:nomic-embed-text", "dims": 768, "context_tokens": 8192,
      "reason": "fast, good general baseline", "available": true, "recommended": true },
    { "id": "ollama:bge-m3", "dims": 1024, "context_tokens": 8192,
      "reason": "best quality, slower indexing (~1.2 GB)", "available": true, "recommended": false },
    { "id": "local:JinaEmbeddingsV2BaseCode", "dims": 768, "context_tokens": 8192,
      "reason": "code-specific, no Ollama needed (~300 MB)", "available": true, "recommended": false }
  ],
  "instructions": "..."
}
```

`config_created` is `true` when `.codescout/project.toml` did not exist and was created by this call.
The `hardware` field reports what was detected (Ollama, GPU, RAM, CPU cores). The `model_options`
array contains exactly 3 ranked choices; `recommended: true` marks the one written to `project.toml`.
The `instructions` field contains a prompt — including a **Phase 0.5** model selection menu — that
guides the LLM through confirming or changing the embedding model before indexing begins.

**Tips:** Call `onboarding` once per project, the first time you work on it. It writes a memory entry under the topic `"onboarding"` with a summary of what it found. On subsequent sessions, call `onboarding` with `force: false` (the default) — it detects previous onboarding and returns existing memories without re-running discovery.

---

## `run_command`

**Purpose:** Run a shell command in the active project root. Short output is returned inline; large output is stored in a session buffer and returned as a `@cmd_*` handle that you can query in follow-up calls.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `command` | string | yes | — | Shell command to execute. May reference output buffer handles with `@cmd_*` syntax (e.g. `grep FAILED @cmd_a1b2`). Never prefix with `cd /abs/path &&` — already in the project root. |
| `cwd` | string | no | — | Subdirectory relative to project root to run the command in. Validated to stay within the project. |
| `timeout_secs` | integer | no | `30` | Max execution time in seconds. Ignored when `run_in_background` is true. |
| `acknowledge_risk` | boolean | no | `false` | Bypass the dangerous-command check directly. Prefer the `@ack_*` handle protocol — see below. |
| `run_in_background` | boolean | no | `false` | Spawn the command detached and return immediately with a log path. Use for long-running processes or commands that background subprocesses with `&`. |

**Example (run a test suite):**

```json
{ "command": "cargo test", "timeout_secs": 120 }
```

**Example (run from a subdirectory):**

```json
{ "command": "npm test", "cwd": "frontend" }
```

---

### Output shapes

#### Short output (< ~50 lines) — inline

```json
{
  "stdout": "running 42 tests\ntest result: ok. 42 passed; 0 failed",
  "stderr": "",
  "exit_code": 0
}
```

#### Large output — buffered

When output exceeds the inline threshold, the full content is stored in a session buffer and a smart summary is returned alongside the handle:

```json
{
  "output_id": "@cmd_a1b2c3",
  "exit_code": 1,
  "passed": 39,
  "failed": 3,
  "failures": ["test_foo", "test_bar", "test_baz"]
}
```

Query the buffer in a follow-up call — **do not pipe**:

```json
{ "command": "grep 'FAILED\\|error' @cmd_a1b2c3" }
```

The `output_id` handle stays valid for the session (LRU, max 50 entries). Buffer queries can also use `sed -n 'N,Mp' @cmd_a1b2c3` to page through output.

#### Dangerous command — pending acknowledgement

Commands matching dangerous patterns (e.g. `rm -rf`, `git reset --hard`) return a `pending_ack` handle instead of executing:

```json
{
  "pending_ack": "@ack_d4e5f6",
  "message": "Dangerous command detected: rm -rf target/",
  "command": "rm -rf target/"
}
```

Re-run with just the handle to confirm:

```json
{ "command": "@ack_d4e5f6" }
```

Handles expire at end of session. Alternatively, pass `acknowledge_risk: true` on the original call to skip the confirmation step.

#### Background command

```json
{
  "status": "running",
  "log_file": "/tmp/codescout-bg-xxxx.log",
  "ref_id": "@cmd_a1b2c3"
}
```

Monitor with `run_command("tail -50 /tmp/codescout-bg-xxxx.log")`.

---

### Security

> See [Security & Permissions](../concepts/security.md) for the full permission model, including write sandboxing and the built-in credential deny list.

Shell execution is **disabled by default**. To enable it, add to `.codescout/project.toml`:

```toml
[security]
shell_command_mode = "warn"   # or "unrestricted"
```

| Value | Behaviour |
|-------|-----------|
| `"disabled"` | All calls return an error. This is the default. |
| `"warn"` | Commands execute normally. |
| `"unrestricted"` | Commands execute normally (alias for `warn`, no functional difference). |

On Unix the command runs under `sh -c`. On Windows it runs under `cmd /C`.

---

**Tips:**

- **Never pipe inside the command to filter output** (`cargo test 2>&1 | grep FAILED`). Run the command bare, then use `grep FAILED @cmd_id` in a follow-up call. Buffer queries preserve your context window; piped commands waste it.
- For slow build steps (`cargo build`, full test suites), increase `timeout_secs` to 120–300.
- Use `cwd` to run commands in subdirectories rather than `cd subdir &&` prefixes.
- For commands that background subprocesses with `&`, use `run_in_background: true` — otherwise `run_command` will hang until timeout waiting for the shell to exit.

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

**Purpose:** Display the full state of the active project in one call: config, semantic index health, library registry, and memory staleness. Combines what was previously `get_config` and `index_status`.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `threshold` | number | no | — | When provided, includes drift data: minimum `avg_drift` to include (0.0–1.0) |
| `path` | string | no | — | SQL LIKE pattern to filter drift results by file path (e.g. `"src/tools/%"`) |
| `detail_level` | string | no | `"exploring"` | Drift output detail: `"full"` includes most-drifted chunk content |

**Example (basic):**

```json
{}
```

**Example (with drift query):**

```json
{ "threshold": 0.2 }
```

**Output:**

```json
{
  "project_root": "/home/user/projects/my-service",
  "config": {
    "project": { "name": "my-service", "languages": ["rust", "toml"] },
    "embeddings": { "model": "ollama:mxbai-embed-large" }
  },
  "index": {
    "indexed": true,
    "files": 47,
    "chunks": 312,
    "model": "ollama:mxbai-embed-large",
    "last_updated": "2026-03-08T10:30:00Z",
    "git_sync": { "status": "up_to_date" }
  },
  "libraries": { "count": 2, "indexed": 1 },
  "memory_staleness": {
    "stale": ["architecture", "conventions/naming"],
    "fresh": ["onboarding", "gotchas"],
    "untracked": ["debugging/lsp-timeouts"]
  }
}
```

#### `memory_staleness` section

This section is always included. It categorises memory topics by anchor health:

| Key | Meaning |
|-----|---------|
| `stale` | Topics where anchored source files have changed since the memory was last written — the memory may be outdated |
| `fresh` | Topics whose anchored files match the stored hashes — memory is current |
| `untracked` | Topics with no anchor sidecars — staleness cannot be determined |

When topics appear in `stale`, review them and either rewrite the memory (`action: "write"`) or confirm it is still accurate and call `memory(action: "refresh_anchors", topic: ...)` to clear the warning.

**Tips:**

- Use `project_status` to verify which project is active and to check security settings before attempting shell commands or indexing.
- Pass `threshold: 0.1` after re-indexing to surface files that changed semantically — a whitespace reformat scores near `0.0`, a full function rewrite approaches `1.0`.
- If you need to change configuration, edit `.codescout/project.toml` directly — the config is re-read on each tool call, so changes take effect immediately without restarting the server.
- For full per-tool call stats with charts and time-window filtering, see the [Dashboard](../concepts/dashboard.md).
