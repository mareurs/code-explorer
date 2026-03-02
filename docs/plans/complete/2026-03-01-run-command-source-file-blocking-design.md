# Design: run_command Source-File Access Blocking

**Date:** 2026-03-01  
**Status:** Approved  
**Context:** The companion plugin (`pre-tool-guard.sh`) hard-blocks native `Read`, `Grep`, `Glob`, and `Bash` on source files, redirecting agents to code-explorer tools. However, `run_command` itself has no equivalent check — `cat src/main.rs`, `sed -n '1,50p' foo.ts`, etc. pass through unblocked, defeating the plugin's intent.

---

## Goal

Block file-reading shell commands (`cat`, `head`, `tail`, `sed`, `awk`, `less`, `more`, `wc`) when they target source code files, with a soft-block / escape-hatch consistent with the existing dangerous-command pattern. Redirect agents to code-explorer tools with actionable hints.

## Non-Goals

- Blocking `grep`/`rg` (separate concern, not in scope)
- Parsing complex shell pipelines or variable expansions
- Hard-blocking with no escape (use `acknowledge_risk: true` as the escape hatch)

---

## Detection

### New function: `check_source_file_access`

Location: `src/util/path_security.rs`, alongside `is_dangerous_command`.

```rust
pub fn check_source_file_access(command: &str) -> Option<String>
```

Two-part heuristic — both conditions must match:

1. **Blocked command present** (word-boundary match):
   ```
   \b(cat|head|tail|sed|awk|less|more|wc)\b
   ```

2. **Source file extension present** in the command string:
   ```
   \.(rs|py|ts|tsx|js|cjs|mjs|jsx|go|java|kt|kts|c|cpp|cc|cxx|cs|rb|php|swift|scala|ex|exs|hs|lua|sh|bash)\b
   ```

Returns `Some(hint_string)` when both match, `None` otherwise.

The extension list mirrors `detect_language()` in `src/ast/mod.rs` exactly, minus `.md`/`.markdown` (markdown is readable via `read_file` and is not source code).

### Known limits (accepted)

- Quoted paths with spaces: `cat 'path with spaces/main.rs'` — extension still visible in the string, still caught
- Variable expansion: `cat $FILE` — not detectable at command-parse time; accepted limitation
- Buffer refs: `sed ... @cmd_abc` — caught by `buffer_only=true` flag, bypasses check correctly

---

## Integration

In `run_command_inner` (`src/tools/workflow.rs`), after the existing dangerous-command speed bump (step 2), add step 2.5:

```rust
// --- Step 2.5: Source file access block ---
if !buffer_only && !acknowledge_risk {
    if let Some(hint) = check_source_file_access(resolved_command) {
        return Err(RecoverableError::with_hint(
            "shell access to source files is blocked",
            &hint,
        ).into());
    }
}
```

### Escape hatches (both bypass the check)

- `buffer_only = true` — command operates only on `@cmd_xxx`/`@file_xxx` buffer refs; skips all safety checks (already established)
- `acknowledge_risk: true` — explicit user override; same pattern as dangerous commands

No new parameters needed.

---

## Error Messages

The `hint` string from `check_source_file_access` is tailored by the detected command:

| Command | Suggested alternatives |
|---------|----------------------|
| `cat`, `head`, `tail` | `read_file(path, start_line, end_line)` + `list_symbols(path)` + `find_symbol(name, include_body=true)` |
| `sed`, `awk` | same + `search_pattern(regex)` for pattern-based extraction |
| `wc`, `less`, `more` | `read_file(path)` — buffered for large files with `@file_*` handle |

Example full error:
```
shell access to source files is blocked: use read_file(path) or
list_symbols(path) + find_symbol(name, include_body=true) instead.
Re-run with acknowledge_risk: true if you need raw shell access.
```

---

## Testing

### Unit tests in `src/util/path_security.rs`

Blocked cases:
- `cat src/main.rs`
- `head -20 src/tools/mod.rs`
- `tail -n 50 server.ts`
- `sed -n '1,100p' lib.go`
- `awk '{print}' foo.py`
- `less src/agent.rs`
- `wc -l src/lib.rs`

Allowed cases (must NOT block):
- `cat README.md` (markdown excluded)
- `wc -l output.txt` (no source extension)
- `sed 's/foo/bar/g' config.toml` (no source extension)
- `head @cmd_abc` (buffer ref — `buffer_only` bypasses at call site, but even as a string it has no source extension)

### Integration tests in `src/tools/workflow.rs`

- `run_command("cat src/main.rs")` → blocked without `acknowledge_risk`
- `run_command("cat src/main.rs", acknowledge_risk=true)` → executes
- `run_command("sed ... @cmd_abc")` → passes (buffer_only bypasses check)
- `run_command("cat README.md")` → passes (not a source file)

---

## Files Changed

1. `src/util/path_security.rs` — add `check_source_file_access` function + unit tests
2. `src/tools/workflow.rs` — call `check_source_file_access` in `run_command_inner` at step 2.5 + integration tests
