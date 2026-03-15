# run_command Timeout Leniency Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `run_command` tolerate agents passing `timeout` (wrong key) or suspiciously large `timeout_secs` values (likely milliseconds), converting them and returning a hint.

**Architecture:** Add a pure helper `parse_timeout_input(input: &Value) -> (u64, Option<String>)` that encodes all timeout parsing logic. Wire it into `RunCommand::call()` and surface the hint in both the JSON result and the compact format string.

**Tech Stack:** Rust, `serde_json::Value`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-15-run-command-timeout-hint-design.md`

---

## Chunk 1: Implement and wire `parse_timeout_input`

### Task 1: Write failing unit tests for `parse_timeout_input`

**Files:**
- Modify: `src/tools/workflow.rs` — add tests to the existing `#[cfg(test)]` module at the bottom of the file

- [ ] **Step 1: Find the test module**

  Open `src/tools/workflow.rs` and scroll to the bottom. There is a `#[cfg(test)]` module. Add the following tests inside it:

```rust
#[test]
fn parse_timeout_input_correct_key_small() {
    let input = serde_json::json!({ "timeout_secs": 120 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 120);
    assert!(hint.is_none());
}

#[test]
fn parse_timeout_input_correct_key_boundary() {
    let input = serde_json::json!({ "timeout_secs": 86400 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 86400);
    assert!(hint.is_none());
}

#[test]
fn parse_timeout_input_correct_key_over_boundary() {
    let input = serde_json::json!({ "timeout_secs": 86401 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 86);
    let h = hint.unwrap();
    assert!(h.contains("86401"), "hint should contain raw value: {h}");
    assert!(h.contains("86s"), "hint should contain converted value: {h}");
}

#[test]
fn parse_timeout_input_correct_key_large() {
    let input = serde_json::json!({ "timeout_secs": 120_000u64 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 120);
    assert!(hint.is_some());
}

#[test]
fn parse_timeout_input_correct_key_zero() {
    let input = serde_json::json!({ "timeout_secs": 0 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 30);
    assert!(hint.is_some());
}

#[test]
fn parse_timeout_input_wrong_key_small() {
    let input = serde_json::json!({ "timeout": 300 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 300);
    assert!(hint.is_some());
}

#[test]
fn parse_timeout_input_wrong_key_large() {
    let input = serde_json::json!({ "timeout": 120_000u64 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 120);
    assert!(hint.is_some());
}

#[test]
fn parse_timeout_input_wrong_key_zero() {
    let input = serde_json::json!({ "timeout": 0 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 30);
    assert!(hint.is_some());
}

#[test]
fn parse_timeout_input_neither_key() {
    let input = serde_json::json!({});
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 30);
    assert!(hint.is_none());
}

#[test]
fn parse_timeout_input_both_keys_valid() {
    // timeout_secs wins; timeout is silently ignored; no hint (timeout_secs value is valid)
    let input = serde_json::json!({ "timeout_secs": 60, "timeout": 5000 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 60);
    assert!(hint.is_none());
}

#[test]
fn parse_timeout_input_both_keys_secs_large() {
    // timeout_secs wins and triggers conversion hint; timeout is ignored
    let input = serde_json::json!({ "timeout_secs": 120_000u64, "timeout": 5000 });
    let (secs, hint) = parse_timeout_input(&input);
    assert_eq!(secs, 120);
    assert!(hint.is_some());
}
```

- [ ] **Step 2: Run the tests — expect compile error (function not yet defined)**

```bash
cargo test parse_timeout_input 2>&1 | head -20
```

Expected: compile error — `cannot find function 'parse_timeout_input'`

---

### Task 2: Implement `parse_timeout_input`

**Files:**
- Modify: `src/tools/workflow.rs` — add the helper function near the top of the `RunCommand` impl block, before `impl Tool for RunCommand`

- [ ] **Step 1: Add a private helper `get_timeout_u64` and `parse_timeout_input` above the `impl Tool for RunCommand` block**

  Find the line `impl Tool for RunCommand` (around line 1367). Insert these two functions immediately before it:

```rust
/// Extract a u64 from a JSON value that may be a Number or a numeric String.
fn get_timeout_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

/// Parse the timeout from run_command input with leniency for:
/// - wrong key name (`timeout` instead of `timeout_secs`)
/// - millisecond values passed as `timeout_secs` (value > 86_400)
///
/// Returns `(resolved_seconds, optional_hint_for_agent)`.
fn parse_timeout_input(input: &Value) -> (u64, Option<String>) {
    // Canonical key: timeout_secs
    if let Some(v) = get_timeout_u64(&input["timeout_secs"]) {
        if v == 0 {
            return (
                30,
                Some("timeout_secs: 0 is invalid — using default of 30s.".to_string()),
            );
        }
        if v > 86_400 {
            let converted = v / 1_000;
            return (
                converted,
                Some(format!(
                    "timeout_secs: {v} looks like milliseconds — converted to {converted}s. \
                     Use timeout_secs with a value in seconds."
                )),
            );
        }
        return (v, None);
    }

    // Fallback: wrong key name `timeout`
    if let Some(v) = get_timeout_u64(&input["timeout"]) {
        if v == 0 {
            return (
                30,
                Some(
                    "Unknown parameter 'timeout' — use timeout_secs. \
                     Value 0 is invalid, using default of 30s."
                        .to_string(),
                ),
            );
        }
        if v >= 1_000 {
            let converted = v / 1_000;
            return (
                converted,
                Some(format!(
                    "Unknown parameter 'timeout' — use timeout_secs. \
                     Converted {v}ms → {converted}s."
                )),
            );
        }
        // v < 1000 → already seconds
        return (
            v,
            Some(format!(
                "Unknown parameter 'timeout' — use timeout_secs. \
                 Interpreted {v} as seconds."
            )),
        );
    }

    // Neither key present
    (30, None)
}
```

- [ ] **Step 2: Run the unit tests — expect all 11 to pass**

```bash
cargo test parse_timeout_input 2>&1 | tail -5
```

Expected: `test result: ok. 11 passed; 0 failed`

- [ ] **Step 3: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(run_command): add parse_timeout_input helper with leniency for wrong key and ms values"
```

---

### Task 3: Wire `parse_timeout_input` into `RunCommand::call()`

**Files:**
- Modify: `src/tools/workflow.rs` — `RunCommand::call()` around line 1411

- [ ] **Step 1: Replace the existing 4-line timeout parse block**

  In `RunCommand::call()`, find and replace:

```rust
        let timeout_secs = match &input["timeout_secs"] {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(30),
            serde_json::Value::String(s) => s.parse::<u64>().unwrap_or(30),
            _ => 30,
        };
```

Replace with:

```rust
        let (timeout_secs, timeout_hint) = parse_timeout_input(&input);
```

- [ ] **Step 2: Attach `timeout_hint` to the result**

  In `RunCommand::call()`, find the `refreshed_handles` injection block — it is the last
  block before the bare `result` return expression (around line 1503). Insert the hint
  attachment **between** the closing `}` of that block and the bare `result`, keeping
  `result` as the final expression:

```rust
        // Attach timeout hint when the timeout parameter was auto-corrected.
        if let Some(ref hint) = timeout_hint {
            if let Ok(ref mut val) = result {
                val["timeout_hint"] = json!(hint);
            }
        }

        result
```

  The bare `result` at the end is unchanged — it remains the return expression of `call()`.

- [ ] **Step 3: Build — ensure it compiles**

```bash
cargo build 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 4: Run all tests**

```bash
cargo test 2>&1 | grep "test result"
```

Expected: all suites pass, 0 failed.

- [ ] **Step 5: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(run_command): wire parse_timeout_input into call(), attach timeout_hint to result"
```

---

### Task 4: Update `format_run_command` to surface hint in compact view

**Files:**
- Modify: `src/tools/workflow.rs` — `format_run_command` at line 1530

- [ ] **Step 1: Restructure `format_run_command` to append hint after all branch logic**

  Replace the entire `format_run_command` function with:

```rust
fn format_run_command(result: &Value) -> String {
    let mut s = if result["output_id"].is_string() {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        let output_id = result["output_id"].as_str().unwrap_or("");
        match result["type"].as_str() {
            Some("test") => {
                let passed = result["passed"].as_u64().unwrap_or(0);
                let failed = result["failed"].as_u64().unwrap_or(0);
                let ignored = result["ignored"].as_u64().unwrap_or(0);
                let mut s = format!("{check} exit {exit} · {passed} passed");
                if failed > 0 {
                    s.push_str(&format!(" · {failed} FAILED"));
                }
                if ignored > 0 {
                    s.push_str(&format!(" · {ignored} ignored"));
                }
                s.push_str(&format!("  (query {output_id})"));
                s
            }
            Some("build") => {
                let errors = result["errors"].as_u64().unwrap_or(0);
                if errors > 0 {
                    format!("{check} exit {exit} · {errors} errors  (query {output_id})")
                } else {
                    format!("{check} exit {exit}  (query {output_id})")
                }
            }
            _ => format!("{check} exit {exit}  (query {output_id})"),
        }
    } else if result["timed_out"].as_bool().unwrap_or(false) {
        "✗ timed out".to_string()
    } else {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let stdout_lines = result["stdout"]
            .as_str()
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        format!("{check} exit {exit} · {stdout_lines} lines")
    };

    // Append timeout hint after all branch logic so it covers every output shape.
    if let Some(hint) = result["timeout_hint"].as_str() {
        s.push_str(&format!("\n⚠ timeout: {hint}"));
    }

    s
}
```

- [ ] **Step 2: Format and lint**

```bash
cargo fmt && cargo clippy -- -D warnings 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 3: Run all tests**

```bash
cargo test 2>&1 | grep "test result"
```

Expected: all suites pass, 0 failed.

- [ ] **Step 4: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(run_command): surface timeout_hint in compact format view"
```

---

### Task 5: Full verification

- [ ] **Step 1: Run the full check suite**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

Expected: format clean, clippy clean, all tests pass.

- [ ] **Step 2: Build release binary**

  ```bash
  cargo build --release 2>&1 | grep "^error" | head -5
  ```

  Expected: no errors. (Needed before testing via live MCP server.)
