# Shell Integration

## Overview

`run_command` executes any shell command from the project root with stderr
capture, Output Buffer support, and a thin safety layer. It is the primary way
the AI interacts with the build system, test runner, version control, and any
other CLI tooling.

```
run_command("cargo build")
run_command("git log --oneline -20")
run_command("grep FAILED @cmd_a1b2c3")   # query a previous buffer
run_command("diff @cmd_abc @file_def")   # compose refs freely
```

Stderr is captured automatically alongside stdout — no `2>&1` needed. Use the
`cwd` parameter to run in a subdirectory (path traversal outside the project
root is rejected).

## Safety Layer

### Dangerous Command Detection

Commands with destructive potential are detected before execution and blocked
until the AI explicitly passes `acknowledge_risk: true`. The detection covers:

- Filesystem destruction: `rm -rf`, `rmdir`, `shred`
- Git rewrites: `reset --hard`, `push --force`, `clean -f`, `rebase`
- Database mutations: `DROP TABLE`, `DELETE FROM`, `TRUNCATE`
- Process termination: `kill -9`, `pkill`

The intent is a deliberate pause, not an impenetrable wall. The AI can always
pass `acknowledge_risk: true` to proceed — it just has to do so explicitly
rather than accidentally.

> **In practice:** Over 6+ months of daily use, Claude has never triggered this
> unprompted on a command that would actually cause damage. The main effect is
> an extra confirmation click for legitimate destructive operations (e.g.
> cleaning build artifacts). It's still worth keeping — MCP tools run with your
> full user permissions, and the occasional pause costs almost nothing compared
> to what it could prevent.

### Source File Access Blocking

`cat`, `grep`, `head`, `tail`, `sed`, and `awk` used directly on source files
(`.rs`, `.py`, `.ts`, `.go`, `.java`, `.kt`, etc.) are blocked at the tool
level. The error message suggests the appropriate code-explorer equivalent:

| Blocked pattern | Redirect to |
|---|---|
| `cat src/foo.rs` | `read_file("src/foo.rs")` or `find_symbol("Foo")` |
| `grep 'fn parse' src/` | `search_pattern("fn parse", path="src/")` |
| `head -20 main.py` | `read_file("main.py", start_line=1, end_line=20)` |

This enforces token-efficient navigation. Reading an entire file to find one
function is the antipattern code-explorer is designed to eliminate.

Pass `acknowledge_risk: true` to bypass when you genuinely need raw access
(e.g. checking file encoding, binary content, or files code-explorer can't
parse).

### Path Traversal Protection

The `cwd` parameter is validated before the command runs. Any path that
attempts to escape the project root — via `../`, symlink chains, or absolute
paths outside the tree — is rejected with an error naming the violation.

Output Buffer refs (`@cmd_id`, `@file_id`) are resolved within the session
and are read-only when materialised as temporary files for Unix tool access.
They never expose raw filesystem paths outside the buffer system.
