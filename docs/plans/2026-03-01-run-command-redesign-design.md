# run_command Redesign: Smart Output, Buffer References, and Safety

**Date:** 2026-03-01
**Status:** Approved

## Problem

Subagents using Claude Code's native `Bash` tool cause constant permission
prompts. The user must approve every `cargo test`, `git status`, etc. Meanwhile,
code-explorer already has `run_command` which does the exact same thing
(`sh -c "<command>"`) — with zero overhead difference — but agents don't use it.

Additionally, command output floods the context window. A `cargo test` run can
produce 500+ lines, most of which are irrelevant. There's no way to explore
output selectively.

## Goals

1. **Route all Bash through `run_command`** — eliminate permission fatigue
2. **Smart first response** — context-aware summaries instead of raw dumps
3. **Buffer references (`@output_id`)** — let agents query stored output with
   standard Unix tools (grep, tail, awk, sed) instead of inventing new params
4. **Speed bump for dangerous commands** — two-round-trip safety net, not a
   hard block
5. **Minimal schema** — no new parameters for search/pagination; Unix tools
   handle it

## Non-Goals

- Semantic/embedding search on command output (regex is sufficient for
  structured text)
- Generic OutputBuffer for all tools (build for run_command first, extract if
  a second tool needs it)
- Command sandboxing or environment isolation
- Streaming output

## Design

### Architecture

```
run_command("cargo test")
       │
       ▼
┌──────────────────────────┐
│  1. Security checks      │  ← speed bump? dangerous pattern?
│  2. Execute sh -c "..."  │  ← same as native Bash
│  3. Store in OutputBuffer │  ← HashMap<String, BufferEntry>
│  4. Detect command type   │  ← test? build? git? generic?
│  5. Build smart summary   │  ← extract pass/fail, errors, etc.
└──────────────────────────┘
       │
       ▼
  Smart first response + @output_id
       │
       ▼
  Agent decides next move:
  ├─ Happy with summary → done (most cases)
  ├─ run_command("grep -n FAILED @cmd_a1b2c3")
  ├─ run_command("tail -50 @cmd_a1b2c3")
  ├─ run_command("awk '/error/{print NR, $0}' @cmd_a1b2c3")
  └─ run_command("diff @cmd_x @cmd_y")  ← compare two runs!
```

### Input Schema

```json
{
  "type": "object",
  "required": ["command"],
  "properties": {
    "command": {
      "type": "string",
      "description": "Shell command to execute. May reference stored output buffers with @output_id syntax."
    },
    "timeout_secs": {
      "type": "integer",
      "default": 30,
      "description": "Max execution time in seconds."
    },
    "cwd": {
      "type": "string",
      "description": "Subdirectory relative to project root. Validated to stay within project."
    },
    "acknowledge_risk": {
      "type": "boolean",
      "description": "Bypass speed bump for dangerous commands. Required after a destructive command is detected."
    }
  }
}
```

No `output_id`, `search`, `offset`, `limit`, or `detail_level` params.
The agent uses Unix tools on buffer references instead.

### Output: Smart First Response

When output exceeds a line threshold (default: 50 lines), `run_command` detects
the command type and returns a tailored summary.

#### Command Type Detection

Regex on the command string:

| Type | Pattern | Summarizer |
|------|---------|------------|
| **Test** | `cargo test`, `pytest`, `npm test`, `jest`, `go test`, `mvn test` | Extract pass/fail/ignored counts + full failure output |
| **Build** | `cargo build`, `cargo clippy`, `gcc`, `make`, `npm run build`, `tsc` | Error/warning count + first error with full context |
| **Git** | `git log`, `git diff`, `git status` | Usually short — show all up to line limit |
| **Generic** | Anything else | Head 20 lines + tail 10 lines + total line count |

#### Test Summary Example

```json
{
  "exit_code": 1,
  "output_id": "@cmd_a1b2c3",
  "summary": {
    "type": "test",
    "passed": 541,
    "failed": 2,
    "ignored": 12
  },
  "failures": "test tools::file::tests::edit_lines_multiline ... FAILED\n\nthread 'tools::file::tests::edit_lines_multiline' panicked at src/tools/file.rs:592:9:\nassertion `left == right` failed\n  left: \"alpha\\nbeta\"\n right: \"alpha\\ngamma\"\n",
  "total_stdout_lines": 587,
  "hint": "Full output stored. Query with: grep/tail/awk/sed @cmd_a1b2c3"
}
```

#### Build Error Example

```json
{
  "exit_code": 1,
  "output_id": "@cmd_d4e5f6",
  "summary": {
    "type": "build",
    "errors": 3,
    "warnings": 7
  },
  "first_error": "error[E0308]: mismatched types\n --> src/tools/file.rs:42:5\n  |\n42 |     let x: String = 42;\n  |                     ^^ expected `String`, found integer",
  "total_stdout_lines": 124,
  "total_stderr_lines": 89,
  "hint": "Full output stored. Query with: grep/tail/awk/sed @cmd_d4e5f6"
}
```

#### Short Output (below threshold)

No buffer, no summary — just return everything directly:

```json
{
  "exit_code": 0,
  "stdout": "On branch master\nnothing to commit, working tree clean\n",
  "stderr": ""
}
```

### Buffer References (`@output_id`)

#### How It Works

1. `OutputBuffer` is a session-scoped `HashMap<String, BufferEntry>`
2. `BufferEntry` stores stdout and stderr as strings, plus metadata (command,
   timestamp, exit_code)
3. When `command` contains `@cmd_*` tokens:
   - Look up each referenced buffer
   - Write contents to read-only temp files
   - Substitute `@cmd_*` with temp file paths in the command string
   - Execute the modified command
   - Clean up temp files
4. Stderr is accessible via `@cmd_a1b2c3.err` suffix

#### Examples

```
grep -n "FAILED" @cmd_a1b2c3           → search stdout
grep -c "warning" @cmd_a1b2c3.err      → count warnings in stderr
tail -20 @cmd_a1b2c3                   → last 20 lines
sed -n '50,100p' @cmd_a1b2c3           → lines 50-100
diff @cmd_a1b2c3 @cmd_d4e5f6           → compare two runs
awk '/^test.*FAILED/{print}' @cmd_a1b2c3  → custom extraction
wc -l @cmd_a1b2c3                      → line count
```

#### Security: Buffer-Only Commands Are Safe

If a command ONLY references `@cmd_*` buffers and contains no other file paths
or destructive operations, it operates on ephemeral read-only data. These
commands:

- **Skip the speed bump** (no dangerous command check needed)
- **Skip the shell_command_mode check** (even in "warn" mode, no warning)
- **Always execute** (buffer exploration should never be blocked)

Detection: command contains `@cmd_` AND all file-like arguments resolve to
buffer temp files.

#### Buffer Lifecycle

- **Created**: when command output exceeds line threshold
- **Eviction**: LRU with configurable max count (default: 20 buffers)
- **Session-scoped**: cleared when MCP server restarts
- **No persistence**: buffers are ephemeral, never written to disk permanently

### Security: Speed Bump for Dangerous Commands

#### Two-Round-Trip Pattern

**First call** — command matches a dangerous pattern:

```json
{
  "error": "Destructive command detected: `git push --force` can overwrite remote history.",
  "hint": "Re-run with acknowledge_risk: true to proceed.",
  "matched_pattern": "git push --force"
}
```

This is a `RecoverableError` (`isError: false`) — sibling parallel calls are
NOT aborted.

**Second call** — same command with `acknowledge_risk: true`:

Executes normally. No further warnings.

#### Default Dangerous Patterns

```
rm\s+(-[a-zA-Z]*f|-[a-zA-Z]*r|--force|--recursive)
git\s+push\s+.*--force
git\s+reset\s+--hard
git\s+branch\s+-D
git\s+checkout\s+--\s+\.
git\s+clean\s+-[a-zA-Z]*f
DROP\s+(TABLE|DATABASE)
chmod\s+777
kill\s+-9
mkfs
dd\s+if=
```

#### Configuration Override (`project.toml`)

```toml
[security]
# Skip speed bump for these patterns (trusted in this project)
shell_allow_always = ["git push --force"]

# Add project-specific dangerous patterns
shell_dangerous_patterns = ["kubectl delete", "terraform destroy"]
```

### `cwd` Parameter

Allows running commands from subdirectories:

```json
{
  "command": "npm test",
  "cwd": "frontend"
}
```

- Resolved relative to project root: `<project_root>/frontend`
- Validated: must be within project root (no `../../../etc` escapes)
- Uses existing `validate_write_path` infrastructure

### Plugin Hook Change

The routing plugin's `pre-tool-guard.sh` Bash case gets expanded:

**Current**: only blocks `grep`/`cat`/`head`/`tail`/`sed -i` on source files.

**New**: blocks ALL Bash calls when code-explorer is detected, with message:

```
⛔ BLOCKED: Use run_command("your command") instead of Bash.
run_command provides:
  - Smart output summaries (test results, build errors)
  - Output buffers queryable with grep/tail/awk/sed @output_id
  - Dangerous command detection with escape hatch
  - Runs in project root with optional cwd
```

**Exception**: if `HAS_CODE_EXPLORER=false` (no MCP server), Bash works
normally.

### Server Instructions Update

Replace the `run_command` line and update `read_file` in `server_instructions.md`.
Introduce a shared **Output buffers** concept block so neither tool needs to
re-explain what `@ref` means:

```markdown
**Output buffers:**
Large content — whether from a command or a file read — is stored in an
`OutputBuffer` rather than dumped into your context. You get a smart summary
and an `@ref` handle (`@cmd_*` for commands, `@file_*` for files). The full
content costs you nothing to hold. Query it via `run_command` + Unix tools:
  run_command("grep FAILED @cmd_a1b2c3")
  run_command("sed -n '42,80p' @file_abc123")
  run_command("diff @cmd_a1b2c3 @file_abc123")
**Be targeted:** extract what you need in one well-crafted query per buffer —
don't probe the same `@ref` multiple times for overlapping information.

**Run shell commands:**
- `run_command(command)` — execute a shell command. Run freely even if output
  might be large; the buffer handles it. Returns content directly for short
  output, smart summary + `@cmd_*` ref for large output.
  - `cwd` — run from a subdirectory (relative to project root)
  - `acknowledge_risk` — bypass safety check for destructive commands

**Read files:**
- `read_file(path)` — read a file. Returns content directly for short files,
  smart summary + `@file_*` ref for large files (> 200 lines). For source code
  files the summary includes top-level symbols. Prefer `list_symbols` /
  `find_symbol` for source code navigation — they are more structured and
  token-efficient.
```

## `read_file` Buffer Extension

### Design

`read_file` reuses the same `OutputBuffer` infrastructure from `run_command`.
The threshold is **200 lines** (4× run_command's 50 — files are more structured
than command output, less need for aggressive buffering).

When a file exceeds the threshold and no `start_line`/`end_line` is given:
1. Read the full file
2. Store content in `OutputBuffer` under a `@file_` + 8-char hex ID
3. Generate a smart summary based on file type
4. Return summary + `@file_id` + hint

When `start_line`/`end_line` is specified: always return the lines directly
(no buffer — the caller already knows what they need).

When file ≤ 200 lines: return content directly (no buffer).

### Summaries by File Type

| File type | Summary content |
|-----------|----------------|
| Source (`.rs`, `.py`, `.ts`, …) | Top-level symbols via AST (`list_functions` logic) + line count |
| Markdown | H1/H2 heading outline + line count |
| Config (`.toml`, `.yaml`, `.json`) | Line count + first 30 lines |
| Other | First 20 + last 10 lines (same as `run_command` generic) |

### Buffer ID

`@file_` + 8-char hex (same generation logic as `@cmd_`). Stored in the same
`OutputBuffer` map; `resolve_refs()` handles both `@cmd_*` and `@file_*` the
same way — write to temp file, substitute path in command string.

### Behavior change in `read_file`

The current "source file without line range → error, use symbol tools" behavior
is removed. Large source files now return a symbol summary + `@file_id` instead
of an error. The server instructions still prefer symbol tools, but `read_file`
is no longer a hard rejection.

### Example Response

```json
{
  "file_id": "@file_a1b2c3",
  "summary": {
    "type": "rust",
    "line_count": 543,
    "symbols": [
      "fn call(input: Value, ctx: &ToolContext) -> Result<Value> (line 38)",
      "fn summarize_test_output(stdout: &str) -> TestSummary (line 142)",
      "struct RunCommand (line 12)"
    ]
  },
  "hint": "Full file stored. Query with: grep/sed/awk @file_a1b2c3 via run_command"
}
```

## Implementation Scope

### Changes in code-explorer (Rust)

1. **`OutputBuffer` struct** — new module `src/tools/output_buffer.rs`
   - `HashMap<String, BufferEntry>` with LRU eviction
   - `BufferEntry`: content (bytes), kind (`Command`/`File`), metadata
     (command/path, timestamp, exit_code)
   - `store()`, `get()`, `resolve_refs()` (substitute `@cmd_*`/`@file_*` refs
     with temp file paths)
   - ID generation: `cmd_` or `file_` prefix + 8-char hex from
     timestamp+counter

2. **`RunCommand::call` rewrite** — in `src/tools/workflow.rs`
   - Buffer reference detection and substitution
   - Smart summary generation (command type detection + summarizers)
   - Speed bump logic (dangerous pattern check + acknowledge_risk)
   - `cwd` parameter support
   - Buffer storage for large output
   - Buffer-only command safety bypass

3. **Command type summarizers** — in `src/tools/workflow.rs` or separate module
   - `summarize_test_output()` — parse test framework output
   - `summarize_build_output()` — parse compiler output
   - `summarize_generic()` — head + tail

4. **`ReadFile::call` update** — in `src/tools/file.rs`
   - Remove "source file without line range = error" behavior
   - Add: if `line_count > 200` and no explicit range → store in `OutputBuffer`,
     return smart summary + `@file_id`
   - File type summarizers: `summarize_source_file()` (AST symbols),
     `summarize_markdown()` (headings), `summarize_config()` (first 30 lines),
     `summarize_generic_file()` (head + tail)

5. **Security config additions** — in `src/util/path_security.rs` +
   `src/config/mod.rs`
   - `shell_allow_always: Vec<String>` in `PathSecurityConfig`
   - `shell_dangerous_patterns: Vec<String>` in `PathSecurityConfig`
   - Default dangerous patterns list

### Changes in routing plugin

6. **`pre-tool-guard.sh`** — expand Bash case to block all Bash calls

## Test Plan

- [ ] Unit: OutputBuffer store, get, LRU eviction
- [ ] Unit: `@ref` substitution — `@cmd_*` and `@file_*` both resolve correctly
- [ ] Unit: dangerous pattern matching + acknowledge_risk bypass
- [ ] Unit: command type detection regex
- [ ] Unit: test summarizer parses `cargo test` output correctly
- [ ] Unit: build summarizer parses `cargo build` / `cargo clippy` output
- [ ] Unit: short command output returns directly without buffer
- [ ] Unit: `cwd` validation rejects path traversal
- [ ] Unit: buffer-only commands skip speed bump
- [ ] Unit: `read_file` large file → buffer + summary (rust source type)
- [ ] Unit: `read_file` small file → content directly, no buffer
- [ ] Unit: `read_file` with explicit line range → always direct, no buffer
- [ ] Unit: `read_file` markdown summary extracts headings
- [ ] Unit: `read_file` config summary returns first 30 lines
- [ ] Integration: run_command round-trip — execute, get summary, query with grep
- [ ] Integration: read_file round-trip — read large file, query @file_* with grep
- [ ] Integration: cross-type query — diff @cmd_* vs @file_*
- [ ] Integration: speed bump flow — first call rejected, second with
  acknowledge_risk succeeds
- [ ] Manual: verify plugin hook blocks native Bash and suggests run_command

## Future Ideas

**Approach 3 — Generalized content store**: Rename `OutputBuffer` to a
first-class `ContentStore` with explicit `@cmd_*` and `@file_*` namespaces and
unified querying semantics. Enables cross-type operations like
`diff @file_abc @cmd_def` naturally, and opens the door to other tools
(e.g., `git_blame`, `find_references`) storing large results as `@ref` handles.
Build this when a third tool needs the pattern — don't abstract prematurely.
