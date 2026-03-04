# Project Configuration

Every project managed by codescout has an optional configuration file at
`.code-explorer/project.toml`. The file uses [TOML](https://toml.io/) syntax.

## File Location and Auto-Creation

The file lives at:

```
<project-root>/.code-explorer/project.toml
```

If the file does not exist when a project is first activated, codescout creates it with
sensible defaults derived from the directory name. You can also create it manually before
activating a project.

The configuration is loaded by `ProjectConfig::load_or_default`: if the file is present it is
parsed, otherwise defaults are applied. All sections except `[project]` are fully optional —
omit any section and its defaults take effect silently.

---

## `[project]` — General Settings

```toml
[project]
name = "my-service"
languages = ["rust", "toml", "markdown"]
encoding = "utf-8"
tool_timeout_secs = 60
```

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | directory basename | Human-readable project name shown in tool output. |
| `languages` | array of strings | `[]` | Languages detected in the project. Populated automatically by `onboarding`. You can set this manually if auto-detection is wrong. |
| `encoding` | string | `"utf-8"` | Character encoding used when reading source files. |
| `tool_timeout_secs` | integer | `60` | Maximum seconds any single tool call may run before it is cancelled. Increase this for very large projects where LSP startup or indexing takes longer. |

**Note:** `languages` is populated by the `onboarding` tool and written back to the config file.
You rarely need to set it by hand.

---

## `[embeddings]` — Semantic Search Settings

Controls which embedding model is used and how source files are chunked before embedding.

```toml
[embeddings]
model = "ollama:mxbai-embed-large"
drift_detection_enabled = true
```

| Field | Type | Default | Description |
|---|---|---|---|
| `model` | string | `"ollama:mxbai-embed-large"` | Embedding model. The prefix selects the backend. See [Embedding Backends](embedding-backends.md) for the full list of supported prefixes and models. |
| `drift_detection_enabled` | bool | `true` | Enable semantic drift detection during index builds. `index_project` compares old and new chunk embeddings to score how much each changed file's *meaning* shifted. Results queryable via `project_status(threshold)`. Set to `false` to opt out. Experimental — adds memory overhead proportional to changed-file count. |

> **Note — chunk size is automatic.** codescout derives the chunk budget
> directly from the model's published context window using a conservative
> `max_tokens × 3 chars/token` formula at 85 % utilisation. There is no
> `chunk_size` or `chunk_overlap` setting — they were removed because manual
> tuning was error-prone and the model string already encodes everything needed.
> Existing `project.toml` files containing these keys are silently ignored.

**Changing the model after indexing:** If you change `model`, you must rebuild the index
(`index_project` with `force: true`). codescout detects model mismatches and will warn
rather than return wrong results.

---

## `[ignored_paths]` — Indexing Exclusions

Glob patterns for directories and files that should be excluded from semantic search indexing,
`list_dir`, and file traversal.

```toml
[ignored_paths]
patterns = [
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".code-explorer",
]
```

| Field | Type | Default | Description |
|---|---|---|---|
| `patterns` | array of strings | (list above) | Path components or glob patterns to skip during traversal. Each pattern is matched against every path segment, not just the full path. |

The default list covers common build artifact and dependency directories. To add your own
exclusions, replace the entire `patterns` array — there is no "append" mode:

```toml
[ignored_paths]
patterns = [
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".code-explorer",
    "vendor",
    "generated",
    "*.pb.go",
]
```

---

## `[security]` — Access Controls

> See [Security & Permissions](../concepts/security.md) for the rationale behind these settings and how the permission model works end-to-end.

Controls what operations the AI agent is permitted to perform. These settings are intentionally
conservative by default: shell execution is off, file writes are on, git reads are on.

```toml
[security]
denied_read_patterns = []
extra_write_roots = []
shell_command_mode = "warn"
shell_output_limit_bytes = 102400
shell_enabled = false
file_write_enabled = true
git_enabled = true
indexing_enabled = true
```

| Field | Type | Default | Description |
|---|---|---|---|
| `denied_read_patterns` | array of strings | `[]` | Additional path prefixes to block from `read_file` and other read tools, beyond the built-in deny-list (see below). |
| `extra_write_roots` | array of strings | `[]` | Additional directories where file write tools are allowed. By default writes are restricted to the project root. |
| `shell_command_mode` | string | `"warn"` | Controls `run_command` behaviour. One of `"unrestricted"`, `"warn"`, or `"disabled"`. |
| `shell_output_limit_bytes` | integer | `102400` | Maximum bytes captured from shell command stdout or stderr. Output beyond this limit is truncated and flagged in the response. |
| `shell_enabled` | bool | `false` | Master switch for shell execution. Must be `true` for `run_command` to run any command regardless of `shell_command_mode`. |
| `file_write_enabled` | bool | `true` | Enables file write tools: `create_file` and the symbol write tools. Set to `false` for a read-only session. |
| `git_enabled` | bool | `true` | Enables git operations via `run_command`. |
| `indexing_enabled` | bool | `true` | Enables `index_project` and `project_status`. Set to `false` to prevent the agent from kicking off potentially long-running indexing. |

### Built-in Read Deny-List

Regardless of `denied_read_patterns`, codescout always blocks reads from these locations:

```
~/.ssh
~/.aws
~/.gnupg
~/.config/gcloud
~/.config/gh
~/.docker/config.json
~/.netrc
~/.npmrc
~/.kube/config
```

On Linux, `/etc/shadow` and `/etc/gshadow` are also blocked. On macOS, `/etc/master.passwd`
is blocked.

Use `denied_read_patterns` to extend this list with additional secrets or sensitive files
specific to your environment:

```toml
[security]
denied_read_patterns = [
    "~/.config/my-app/credentials",
    "/etc/private",
]
```

### Shell Command Mode

The `shell_command_mode` field fine-tunes what happens when `run_command` is called
(assuming `shell_enabled = true`):

| Value | Behaviour |
|---|---|
| `"disabled"` | All shell calls return an error immediately. |
| `"warn"` | Commands execute; a reminder about full user permissions is appended to the response. This is the default. |
| `"unrestricted"` | Commands execute with no warning added to the output. |

### Enabling Shell Execution

Shell execution requires two settings to both be enabled:

```toml
[security]
shell_enabled = true
shell_command_mode = "warn"   # or "unrestricted"
```

The two-field design means you can grant shell access (`shell_enabled = true`) while still
keeping the warning visible (`shell_command_mode = "warn"`), which is recommended for shared
or CI environments.

---

## Complete Example

A complete `project.toml` for a Rust service that uses local CPU embeddings and has shell
execution enabled for running tests:

```toml
[project]
name = "payment-service"
languages = ["rust", "toml", "sql", "markdown"]
encoding = "utf-8"
tool_timeout_secs = 120

[embeddings]
model = "local:AllMiniLML6V2Q"
drift_detection_enabled = true    # set to false to opt out of semantic drift scoring

[ignored_paths]
patterns = [
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".code-explorer",
    "vendor",
    "migrations/archive",
]

[security]
denied_read_patterns = ["~/.config/stripe"]
shell_command_mode = "warn"
shell_output_limit_bytes = 204800
shell_enabled = true
file_write_enabled = true
git_enabled = true
indexing_enabled = true
```

---

## How Configuration Is Loaded

At startup and whenever `activate_project` is called, codescout:

1. Looks for `.code-explorer/project.toml` in the project root.
2. If found, parses it. Any section that is missing falls back to its defaults.
3. If not found, constructs a default config using the directory name as the project name.

The effective configuration is always visible via the `project_status` tool:

```json
{ "name": "project_status", "arguments": {} }
```

Changes to `project.toml` take effect the next time the project is activated — either by
restarting the MCP server or by calling `activate_project` again with the same path.
