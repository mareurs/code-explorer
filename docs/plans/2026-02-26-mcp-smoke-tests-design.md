# Design: MCP Smoke Test Suite

**Date:** 2026-02-26
**Status:** Approved

## Problem

Unit tests verify internal behavior but don't test the real MCP transport path. We need
end-to-end regression tests that exercise code-explorer as Claude Code would — over stdio
via JSON-RPC — to catch tool access regressions, broken symbol navigation, and routing
issues.

## Design

### File and runner

A single shell script at `tests/mcp-smoke.sh`. Run manually or in CI:
```bash
./tests/mcp-smoke.sh
```

Requires: `mcp` CLI (`~/.local/bin/mcp`), Rust toolchain for building.

The script builds the release binary, creates a temp `mcp` alias, runs all scenarios,
tears down, and reports results. Exit code is nonzero if any test fails (CI-friendly).

### Scenario categories

**1. Tool access control (~6 tests)**
- `read_file` blocks `.rs`, `.py`, `.ts`, `.go`, `.sh` (source files)
- `read_file` allows `.toml`, `.md`, `.yaml`, `.txt` (non-source)
- `replace_content` tool does not exist
- Blocked error message contains symbol tool hints

**2. Symbol navigation (~4 tests)**
- `get_symbols_overview` on a known file returns expected symbols
- `find_symbol` locates a known function by name
- `find_symbol` with `include_body=true` returns actual source
- `list_functions` returns signatures

**3. Search workflows (~3 tests)**
- `search_for_pattern` finds a known regex match
- `find_file` locates a known file by glob
- `semantic_search` returns relevant results for a concept query

**4. Read/write non-code files (~2 tests)**
- `read_file` on `Cargo.toml` returns content
- `read_file` on `README.md` returns content (markdown exception)

**5. Multi-step exploration (~2 tests)**
- "How does error routing work?" — find_symbol → verify body
- "What tools exist?" — get_symbols_overview on server.rs → verify structure

### Per-scenario structure

Each scenario is a shell function with:
- Natural-language task comment (future prompt for LLM benchmarking)
- Sequence of `mcp call` invocations
- Assertions on results
- Step counter (number of tool calls)

### Helper functions

- `call(tool, params)` — wraps `mcp call`, increments step counter, captures result
- `assert_contains(result, string)` — check output contains expected text
- `assert_not_contains(result, string)` — check output does NOT contain text
- `assert_json_key(result, key)` — check JSON key exists
- `pass(name)` / `fail(name, reason)` — track results, print colored output

### Output format

```
Building code-explorer... done
Starting MCP smoke tests against own codebase

=== Tool Access Control ===
PASS  [1]  read_file blocks .rs files
PASS  [1]  read_file allows .toml files
...

=== Symbol Navigation ===
PASS  [1]  get_symbols_overview finds symbols in server.rs
...

17/17 passed (0 failed) in 22 steps
```

### Future (b) extension

Each scenario's natural-language comment serves as the prompt catalog for LLM benchmarking.
The step count per scenario establishes the "optimal budget" — when running with a real LLM,
compare its step count against the scripted optimal to measure navigation efficiency.
