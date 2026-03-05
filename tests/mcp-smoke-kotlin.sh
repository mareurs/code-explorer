#!/usr/bin/env bash
# End-to-end MCP smoke test for codescout against a Kotlin project.
# Calls the real binary over stdio via the `mcp` CLI tool.
# Run from the project root: ./tests/mcp-smoke-kotlin.sh
#
# Requires:
#   - `mcp` CLI (~/.local/bin/mcp)
#   - Kotlin project at /home/marius/work/mirela/backend-kotlin
set -euo pipefail

PASS=0
FAIL=0
STEPS=0
FAILURES=""
RESULT=""

KOTLIN_PROJECT="/home/marius/work/mirela/backend-kotlin"

# ── Helpers ──────────────────────────────────────────────────────────────────

call() {
    local tool="$1" params="$2"
    STEPS=$((STEPS + 1))
    RESULT=$(mcp call "$tool" -p "$params" ce-kt-test 2>/dev/null) || RESULT=""
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

echo "Building codescout (release)..."
cargo build --release 2>&1 | tail -1
echo "done"
echo ""

export PATH="$HOME/.local/bin:$PATH"
mcp alias add ce-kt-test ./target/release/codescout start --project "$KOTLIN_PROJECT"

cleanup() {
    mcp alias remove ce-kt-test 2>/dev/null || true
}
trap cleanup EXIT

# ── Category 1: Tool Access Control ─────────────────────────────────────────

echo "=== Tool Access Control ==="

test_read_file_blocks_kt() {
    call read_file '{"path": "src/main/kotlin/edu/planner/service/AuthService.kt"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .kt files"
    else
        fail 1 "read_file blocks .kt files" "expected RecoverableError for source code"
    fi
}

test_read_file_blocks_kts() {
    call read_file '{"path": "build.gradle.kts"}'
    if assert_contains "error" && assert_contains "source code"; then
        pass 1 "read_file blocks .kts files"
    else
        fail 1 "read_file blocks .kts files" "expected RecoverableError for source code"
    fi
}

test_read_file_allows_properties() {
    call read_file '{"path": "gradle.properties"}'
    if assert_contains "kotlin.code.style" && assert_json_has "content"; then
        pass 1 "read_file allows .properties files"
    else
        fail 1 "read_file allows .properties files" "content not returned"
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

test_read_file_allows_yml() {
    call read_file '{"path": "docker-compose.yml"}'
    if assert_contains "services" && assert_json_has "content"; then
        pass 1 "read_file allows .yml files"
    else
        fail 1 "read_file allows .yml files" "blocked or error"
    fi
}

test_blocked_error_has_hints() {
    call read_file '{"path": "src/main/kotlin/edu/planner/service/AuthService.kt"}'
    if assert_contains "get_symbols_overview" && assert_contains "find_symbol"; then
        pass 1 "blocked error includes symbol tool hints"
    else
        fail 1 "blocked error includes symbol tool hints" "missing tool suggestions"
    fi
}

test_read_file_blocks_kt
test_read_file_blocks_kts
test_read_file_allows_properties
test_read_file_allows_md
test_read_file_allows_yml
test_blocked_error_has_hints

# ── Category 2: Symbol Navigation ───────────────────────────────────────────

echo ""
echo "=== Symbol Navigation ==="

test_symbols_overview_auth_service() {
    call get_symbols_overview '{"relative_path": "src/main/kotlin/edu/planner/service/AuthService.kt"}'
    if assert_contains "AuthService" && assert_contains "login" && assert_contains "AuthError"; then
        pass 1 "get_symbols_overview finds symbols in AuthService.kt"
    else
        fail 1 "get_symbols_overview finds symbols in AuthService.kt" "missing expected symbols"
    fi
}

test_find_symbol_login() {
    call find_symbol '{"pattern": "login", "path": "src/main/kotlin/edu/planner/service/AuthService.kt"}'
    if assert_contains "login" && assert_contains "AuthService"; then
        pass 1 "find_symbol locates login method"
    else
        fail 1 "find_symbol locates login method" "function not found"
    fi
}

test_find_symbol_with_body() {
    call find_symbol '{"pattern": "login", "path": "src/main/kotlin/edu/planner/service/AuthService.kt", "include_body": true}'
    if assert_contains "verifyPassword" && assert_contains "generateTokens"; then
        pass 1 "find_symbol with include_body returns login source"
    else
        fail 1 "find_symbol with include_body returns login source" "body not returned or missing expected code"
    fi
}

test_find_symbol_sealed_class() {
    call find_symbol '{"pattern": "AuthError", "path": "src/main/kotlin/edu/planner/service/AuthService.kt", "include_body": true}'
    if assert_contains "AuthError" && assert_contains "InvalidCredentials" && assert_contains "sealed class"; then
        pass 1 "find_symbol finds AuthError sealed class with subclasses"
    else
        fail 1 "find_symbol finds AuthError sealed class with subclasses" "sealed class not found"
    fi
}

test_list_functions_calendar() {
    call list_functions '{"path": "src/main/kotlin/edu/planner/service/CalendarService.kt"}'
    if assert_contains "getAvailableTeachingDays" && assert_contains "getVacationSummary"; then
        pass 1 "list_functions returns CalendarService signatures"
    else
        fail 1 "list_functions returns CalendarService signatures" "missing expected functions"
    fi
}

test_symbols_overview_auth_service
test_find_symbol_login
test_find_symbol_with_body
test_find_symbol_sealed_class
test_list_functions_calendar

# ── Category 3: Search Workflows ────────────────────────────────────────────

echo ""
echo "=== Search ==="

test_search_pattern() {
    call search_for_pattern '{"pattern": "suspend fun", "max_results": 5}'
    if assert_contains "suspend fun"; then
        pass 1 "search_for_pattern finds suspend fun declarations"
    else
        fail 1 "search_for_pattern finds suspend fun declarations" "no matches found"
    fi
}

test_find_file_services() {
    call find_file '{"pattern": "**/*Service*.kt"}'
    if assert_contains "AuthService" && assert_contains "CalendarService"; then
        pass 1 "find_file locates service files"
    else
        fail 1 "find_file locates service files" "service files not found"
    fi
}

test_semantic_search() {
    call semantic_search '{"query": "authentication login password verification"}'
    if assert_contains "auth" || assert_contains "login" || assert_contains "password"; then
        pass 1 "semantic_search finds auth-related content"
    elif assert_contains "index" || assert_contains "No index"; then
        pass 1 "semantic_search reports index status (index not built or stale)"
    else
        fail 1 "semantic_search finds auth-related content" "no relevant results"
    fi
}

test_search_pattern
test_find_file_services
test_semantic_search

# ── Category 4: Non-code file access ────────────────────────────────────────

echo ""
echo "=== Non-code Files ==="

test_read_gradle_properties() {
    call read_file '{"path": "gradle.properties"}'
    if assert_contains "kotlin.code.style" && assert_json_has "content"; then
        pass 1 "read_file returns gradle.properties content"
    else
        fail 1 "read_file returns gradle.properties content" "content not returned"
    fi
}

test_read_readme() {
    call read_file '{"path": "README.md"}'
    if assert_contains "EDU-Planner" && assert_json_has "content"; then
        pass 1 "read_file returns README.md with project name"
    else
        fail 1 "read_file returns README.md with project name" "blocked or error"
    fi
}

test_read_gradle_properties
test_read_readme

# ── Category 5: Multi-step Exploration ───────────────────────────────────────

echo ""
echo "=== Multi-step Exploration ==="

test_explore_auth_flow() {
    # Step 1: Find the AuthService class
    call get_symbols_overview '{"relative_path": "src/main/kotlin/edu/planner/service/AuthService.kt"}'
    if ! assert_contains "AuthService" || ! assert_contains "login"; then
        fail 1 "explore auth: find AuthService structure" "class or methods not found"
        return
    fi
    # Step 2: Read login method body
    call find_symbol '{"pattern": "login", "path": "src/main/kotlin/edu/planner/service/AuthService.kt", "include_body": true}'
    if assert_contains "verifyPassword" && assert_contains "AuthError"; then
        pass 2 "explore auth: discover login flow (find class → read method)"
    else
        fail 2 "explore auth: discover login flow (find class → read method)" "login body incomplete"
    fi
}

test_explore_service_architecture() {
    # Step 1: Find all service files (need enough results to include both)
    call find_file '{"pattern": "**/*Service*.kt"}'
    if ! assert_contains "CalendarService" || ! assert_contains "NotificationService"; then
        fail 1 "explore architecture: find service layer" "service files not found"
        return
    fi
    # Step 2: Overview a specific service
    call list_functions '{"path": "src/main/kotlin/edu/planner/service/CalendarService.kt"}'
    if assert_contains "getAvailableTeachingDays" && assert_contains "isInstitutionalVacation"; then
        pass 2 "explore architecture: discover service layer (find files → list functions)"
    else
        fail 2 "explore architecture: discover service layer (find files → list functions)" "functions not found"
    fi
}

test_explore_auth_flow
test_explore_service_architecture

# ── Report ───────────────────────────────────────────────────────────────────

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
total=$((PASS + FAIL))
echo "$PASS/$total passed ($FAIL failed) in $STEPS steps"
if [ "$FAIL" -gt 0 ]; then
    printf "\nFailures:%b\n" "$FAILURES"
    exit 1
fi
