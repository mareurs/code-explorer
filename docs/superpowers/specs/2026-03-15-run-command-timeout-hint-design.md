# run_command Timeout Parameter Leniency — Design Spec

**Date:** 2026-03-15
**Status:** Approved
**Scope:** `src/tools/workflow.rs` — `RunCommand::call()` and `format_run_command()` only

---

## Problem

Agents frequently pass `timeout: 120000` (wrong key, millisecond value) instead of
`timeout_secs: 120`. The current parser reads only `input["timeout_secs"]`; any other key
falls through to the `_ => 30` default. The command then silently runs with a 30-second
timeout, which may be far too short (e.g. `cargo publish` takes 60–120 s).

Two failure modes observed:
1. **Wrong key** — agent passes `timeout` instead of `timeout_secs`
2. **Likely milliseconds** — agent passes `timeout_secs: 120000` thinking the unit is ms

---

## Design

### Parse helper: `parse_timeout_input`

Replace the existing 4-line parse block in `RunCommand::call()` with a dedicated function:

```rust
fn parse_timeout_input(input: &Value) -> (u64, Option<String>)
```

Returns `(timeout_secs: u64, hint: Option<String>)`.

### Decision table

| Condition | Resolved seconds | `hint` emitted |
|-----------|-----------------|----------------|
| `timeout_secs` present, value = 0 | 30 (default) | `"timeout_secs: 0 is invalid — using default of 30s."` |
| `timeout_secs` present, 1 ≤ value ≤ 86 400 | value as-is | none |
| `timeout_secs` present, value > 86 400 | value / 1 000 | `"timeout_secs: {raw} looks like milliseconds — converted to {converted}s. Use timeout_secs with a value in seconds."` |
| `timeout` present, value = 0 | 30 (default) | `"Unknown parameter 'timeout' — use timeout_secs. Value 0 is invalid, using default of 30s."` |
| `timeout` present, 1 ≤ value < 1 000 | value as-is (seconds) | `"Unknown parameter 'timeout' — use timeout_secs. Interpreted {value} as seconds."` |
| `timeout` present, value ≥ 1 000 | value / 1 000 | `"Unknown parameter 'timeout' — use timeout_secs. Converted {raw}ms → {converted}s."` |
| Neither key present | 30 (default) | none |

**Priority:** `timeout_secs` takes precedence over `timeout` when both are present.
The `timeout` key is then silently ignored (no hint about `timeout` when `timeout_secs`
is also provided and valid). If `timeout_secs` is present but triggers a hint, that hint
covers the case — no additional hint for the ignored `timeout` key.

**Threshold rationale (86 400):** Using 1 hour (3 600) as the threshold creates an
unacceptable false-positive zone: `timeout_secs: 3 601` (a legitimate 61-minute timeout)
would be silently converted to 3s. 86 400 (24 hours) is the practical ceiling for any
real command. Genuine 24-hour timeouts are essentially unheard of in MCP tool use; values
above this are almost certainly milliseconds.

**Note on `timeout: 1 000`:** A value of exactly 1 000 on the `timeout` key is ambiguous
(1 000 seconds or 1 000 ms = 1 s). We resolve this as milliseconds (converted to 1s)
because `timeout` is not our canonical parameter name and the ≥ 1 000 convention is
consistent throughout the table. The hint will direct the agent to use `timeout_secs`
explicitly.

### Hint delivery

The hint is attached to the tool result as a top-level `"timeout_hint"` field:

```json
{
  "stdout": "...",
  "stderr": "...",
  "exit_code": 0,
  "timeout_hint": "Unknown parameter 'timeout' — use timeout_secs. Converted 120000ms → 120s."
}
```

This keeps the hint out of `stdout`/`stderr` (which belong to the subprocess) while
ensuring it surfaces in the same response as the command result.

### Compact format (`format_run_command`)

`format_run_command` currently produces a one-line summary used in `format_compact`. When a
`timeout_hint` field is present, append it as a second line:

```
exit:0 · 2.1s  cargo publish --allow-dirty
⚠ timeout: Unknown parameter 'timeout' — use timeout_secs. Converted 120000ms → 120s.
```

The hint append is applied to the final string returned by `format_run_command`, after all
branch logic (`output_id` buffered, `timed_out`, inline), so it covers all output shapes
uniformly. The compact example above is illustrative — the real inline format omits timing
and the command text.

### `pending_ack` re-dispatch path

`RunCommand::call()` has an early-return path for `@ack_*` handles that calls
`run_command_inner` with `stored.timeout_secs`. This stored value was produced by
`parse_timeout_input` during the first (pending) call, so it is already the corrected
seconds value. No changes are needed to the re-dispatch path; the invariant holds
automatically.

### No schema changes

`timeout` is intentionally NOT added to the JSON schema. Adding it as a documented
parameter would encourage its use. The hint directs agents to the canonical `timeout_secs`.

---

## Affected Files

| File | Change |
|------|--------|
| `src/tools/workflow.rs` | Replace timeout parse block in `call()` with `parse_timeout_input()`. Add the helper function. Attach `timeout_hint` to result. Update `format_run_command` to render hint. |

No changes to `run_command_inner`, the server, or any other file.

---

## Tests

| Test name | What it covers |
|-----------|----------------|
| `parse_timeout_input_correct_key_small` | `timeout_secs: 120` → 120s, no hint |
| `parse_timeout_input_correct_key_large` | `timeout_secs: 120000` → 120s, hint present |
| `parse_timeout_input_correct_key_zero` | `timeout_secs: 0` → 30s, hint present |
| `parse_timeout_input_correct_key_boundary` | `timeout_secs: 86400` → 86400s, no hint (at threshold) |
| `parse_timeout_input_correct_key_over_boundary` | `timeout_secs: 86401` → 86s, hint contains "86401" and "86s" |
| `parse_timeout_input_wrong_key_small` | `timeout: 300` → 300s, hint present |
| `parse_timeout_input_wrong_key_large` | `timeout: 120000` → 120s, hint present |
| `parse_timeout_input_wrong_key_zero` | `timeout: 0` → 30s, hint present |
| `parse_timeout_input_neither_key` | no key → 30s, no hint |
| `parse_timeout_input_both_keys_valid` | `timeout_secs: 60`, `timeout: 5000` → 60s, no hint (timeout_secs wins, both valid, no conversion needed) |
| `parse_timeout_input_both_keys_secs_large` | `timeout_secs: 120000`, `timeout: 5000` → 120s, hint for timeout_secs conversion (timeout_secs wins; its hint is emitted) |

All tests are unit tests on `parse_timeout_input` — no MCP server spin-up needed.

---

## Out of Scope

- `timeout_ms` as an additional alias (YAGNI — `timeout` covers the observed failure mode)
- Schema-level declaration of `timeout` as an alias
- Changes to `tool_timeout_secs` (project config) — unrelated
