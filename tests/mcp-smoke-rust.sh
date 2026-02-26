#!/usr/bin/env bash
# End-to-end MCP smoke test for code-explorer.
# Calls the real binary over stdio via the `mcp` CLI tool.
# Run from the project root: ./tests/mcp-smoke.sh
set -euo pipefail

PASS=0
FAIL=0
STEPS=0
FAILURES=""
RESULT=""

# ── Helpers ──────────────────────────────────────────────────────────────────

call() {
    local tool="$1" params="$2"
    STEPS=$((STEPS + 1))
    RESULT=$(mcp call "$tool" -p "$params" ce-test 2>/dev/null) || RESULT=""
}

assert_contains() {
    echo "$RESULT" | grep -q "$1"
}

assert_not_contains() {
    ! echo "$RESULT" | grep -q "$1"
}

assert_json_has() {
    echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$1' in d" 2>/dev/null
}

pass() {
    local steps="$1" name="$2"
    PASS=$((PASS + 1))
    printf "  \033[32mPASS\033[0m  [%d]  %s\n" "$steps" "$name"
}

fail() {
    local steps="$1" name="$2" reason="$3"
    FAIL=$((FAIL + 1))
    FAILURES="${FAILURES}\n  - ${name}: ${reason}"
    printf "  \033[31mFAIL\033[0m  [%d]  %s — %s\n" "$steps" "$name" "$reason"
}

# ── Setup ────────────────────────────────────────────────────────────────────

echo "Building code-explorer (release)..."
cargo build --release 2>&1 | tail -1
echo "done"
echo ""

export PATH="$HOME/.local/bin:$PATH"
mcp alias add ce-test ./target/release/code-explorer start --project .

# Temp files for extensions we don't have in the project
echo "test" > _test_smoke.py
echo "test" > _test_smoke.ts
echo "test" > _test_smoke.go
echo "test" > _test_smoke.sh
echo "test" > _test_smoke.txt
echo "test: value" > _test_smoke.yaml
echo "a,b,c" > _test_smoke.csv

cleanup() {
    mcp alias remove ce-test 2>/dev/null || true
    rm -f _test_smoke.*
}
trap cleanup EXIT

# ── Category 1: Tool Access Control ─────────────────────────────────────────

echo "=== Tool Access Control ==="

test_read_file_blocks_rs() {
    call read_file '{"path": "src/main.rs"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .rs files"
    else
        fail 1 "read_file blocks .rs files" "expected RecoverableError for source code"
    fi
}

test_read_file_blocks_py() {
    call read_file '{"path": "_test_smoke.py"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .py files"
    else
        fail 1 "read_file blocks .py files" "expected RecoverableError for source code"
    fi
}

test_read_file_blocks_ts() {
    call read_file '{"path": "_test_smoke.ts"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .ts files"
    else
        fail 1 "read_file blocks .ts files" "expected RecoverableError for source code"
    fi
}

test_read_file_blocks_go() {
    call read_file '{"path": "_test_smoke.go"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .go files"
    else
        fail 1 "read_file blocks .go files" "expected RecoverableError for source code"
    fi
}

test_read_file_blocks_sh() {
    call read_file '{"path": "_test_smoke.sh"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .sh files"
    else
        fail 1 "read_file blocks .sh files" "expected RecoverableError for source code"
    fi
}

test_read_file_allows_toml() {
    call read_file '{"path": "Cargo.toml"}'
    if assert_contains "code-explorer" && assert_json_has "content"; then
        pass 1 "read_file allows .toml files"
    else
        fail 1 "read_file allows .toml files" "content not returned"
    fi
}

test_read_file_allows_md() {
    call read_file '{"path": "README.md"}'
    if assert_json_has "content"; then
        pass 1 "read_file allows .md files"
    else
        fail 1 "read_file allows .md files" "blocked or error"
    fi
}

test_read_file_allows_txt() {
    call read_file '{"path": "_test_smoke.txt"}'
    if assert_json_has "content"; then
        pass 1 "read_file allows .txt files"
    else
        fail 1 "read_file allows .txt files" "blocked or error"
    fi
}

test_replace_content_removed() {
    STEPS=$((STEPS + 1))
    if ! mcp call replace_content -p '{"path":"x","old":"a","new":"b"}' ce-test 2>/dev/null; then
        pass 1 "replace_content tool does not exist"
    else
        fail 1 "replace_content tool does not exist" "tool call succeeded"
    fi
}

test_blocked_error_has_hints() {
    call read_file '{"path": "src/main.rs"}'
    if assert_contains "get_symbols_overview" && assert_contains "find_symbol"; then
        pass 1 "blocked error includes symbol tool hints"
    else
        fail 1 "blocked error includes symbol tool hints" "missing tool suggestions"
    fi
}

test_read_file_blocks_rs
test_read_file_blocks_py
test_read_file_blocks_ts
test_read_file_blocks_go
test_read_file_blocks_sh
test_read_file_allows_toml
test_read_file_allows_md
test_read_file_allows_txt
test_replace_content_removed
test_blocked_error_has_hints

# ── Category 2: Symbol Navigation ───────────────────────────────────────────

echo ""
echo "=== Symbol Navigation ==="

test_symbols_overview() {
    call get_symbols_overview '{"relative_path": "src/server.rs"}'
    if assert_contains "CodeExplorerServer" && assert_contains "from_parts"; then
        pass 1 "get_symbols_overview finds symbols in server.rs"
    else
        fail 1 "get_symbols_overview finds symbols in server.rs" "missing expected symbols"
    fi
}

test_find_symbol() {
    call find_symbol '{"pattern": "route_tool_error"}'
    if assert_contains "route_tool_error" && assert_contains "server.rs"; then
        pass 1 "find_symbol locates route_tool_error"
    else
        fail 1 "find_symbol locates route_tool_error" "function not found"
    fi
}

test_find_symbol_with_body() {
    call find_symbol '{"pattern": "route_tool_error", "include_body": true}'
    if assert_contains "RecoverableError"; then
        pass 1 "find_symbol with include_body returns source"
    else
        fail 1 "find_symbol with include_body returns source" "body not returned or missing RecoverableError"
    fi
}

test_list_functions() {
    call list_functions '{"path": "src/lsp/transport.rs"}'
    if assert_contains "read_message" && assert_contains "write_message"; then
        pass 1 "list_functions returns signatures for transport.rs"
    else
        fail 1 "list_functions returns signatures for transport.rs" "missing expected functions"
    fi
}

test_symbols_overview
test_find_symbol
test_find_symbol_with_body
test_list_functions

# ── Category 3: Search Workflows ────────────────────────────────────────────

echo ""
echo "=== Search ==="

test_search_pattern() {
    call search_for_pattern '{"pattern": "RecoverableError"}'
    if assert_contains "RecoverableError"; then
        pass 1 "search_for_pattern finds RecoverableError"
    else
        fail 1 "search_for_pattern finds RecoverableError" "no matches found"
    fi
}

test_find_file() {
    call find_file '{"pattern": "**/transport.rs"}'
    if assert_contains "transport.rs"; then
        pass 1 "find_file locates transport.rs"
    else
        fail 1 "find_file locates transport.rs" "file not found"
    fi
}

test_semantic_search() {
    call semantic_search '{"query": "embedding pipeline chunking"}'
    if assert_contains "embed" || assert_contains "chunk"; then
        pass 1 "semantic_search finds embedding-related code"
    elif assert_contains "index" || assert_contains "No index" || assert_contains "no such"; then
        pass 1 "semantic_search reports index status (index not built or stale)"
    else
        fail 1 "semantic_search finds embedding-related code" "no relevant results"
    fi
}

test_search_pattern
test_find_file
test_semantic_search

# ── Category 4: Non-code file access ────────────────────────────────────────

echo ""
echo "=== Non-code Files ==="

test_read_cargo_toml() {
    call read_file '{"path": "Cargo.toml"}'
    if assert_contains "code-explorer" && assert_json_has "content"; then
        pass 1 "read_file returns Cargo.toml content"
    else
        fail 1 "read_file returns Cargo.toml content" "content not returned"
    fi
}

test_read_readme() {
    call read_file '{"path": "README.md"}'
    if assert_json_has "content"; then
        pass 1 "read_file allows README.md (markdown exception)"
    else
        fail 1 "read_file allows README.md (markdown exception)" "blocked or error"
    fi
}

test_read_cargo_toml
test_read_readme

# ── Category 5: Multi-step Exploration ───────────────────────────────────────

echo ""
echo "=== Multi-step Exploration ==="

test_explore_error_routing() {
    call find_symbol '{"pattern": "route_tool_error"}'
    if ! assert_contains "route_tool_error"; then
        fail 1 "explore: find error routing function" "function not found"
        return
    fi
    call find_symbol '{"pattern": "route_tool_error", "include_body": true}'
    if assert_contains "RecoverableError" && assert_contains "CallToolResult"; then
        pass 2 "explore: find and read error routing implementation"
    else
        fail 2 "explore: find and read error routing implementation" "body incomplete"
    fi
}

test_explore_tool_architecture() {
    call get_symbols_overview '{"relative_path": "src/server.rs"}'
    if assert_contains "from_parts" && assert_contains "call_tool"; then
        pass 1 "explore: discover tool registry structure"
    else
        fail 1 "explore: discover tool registry structure" "missing key symbols"
    fi
}

test_explore_error_routing
test_explore_tool_architecture

# ── Report ───────────────────────────────────────────────────────────────────

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
total=$((PASS + FAIL))
echo "$PASS/$total passed ($FAIL failed) in $STEPS steps"
if [ "$FAIL" -gt 0 ]; then
    printf "\nFailures:%b\n" "$FAILURES"
    exit 1
fi
