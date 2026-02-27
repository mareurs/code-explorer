# Tool Usage Monitor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Record every tool call's outcome, latency, and overflow status to SQLite, and expose a `get_usage_stats` MCP tool for querying call patterns.

**Architecture:** A `UsageRecorder` struct in a new `src/usage/` module wraps the `call_tool` dispatch in `server.rs`. It opens `.code-explorer/usage.db` per-call (same pattern as `embeddings.db`), writes a row, and prunes rows older than 30 days. A new `GetUsageStats` tool queries the DB and returns per-tool aggregates.

**Tech Stack:** `rusqlite` (already in Cargo.toml), `serde_json` (already present), `std::time::Instant` for latency, SQLite `datetime('now')` for timestamps.

---

### Task 1: DB layer — schema, open, write

**Files:**
- Create: `src/usage/db.rs`
- Create: `src/usage/mod.rs`
- Modify: `src/lib.rs` (add `pub mod usage;`)

**Context:** Follow the pattern in `src/embed/index.rs`: `open_db(project_root)` creates the file + tables on first call, safe to call on every request. Use `datetime('now')` in SQL so timestamps are ISO 8601 and directly comparable to `datetime('now', '-30 days')`.

**Step 1: Write the failing tests**

Add to `src/usage/db.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let conn = open_db(dir.path()).unwrap();
        (dir, conn)
    }

    #[test]
    fn open_db_creates_table() {
        let (_dir, conn) = tmp();
        // table exists if this doesn't error
        conn.execute("SELECT 1 FROM tool_calls LIMIT 0", []).unwrap();
    }

    #[test]
    fn write_record_roundtrip() {
        let (_dir, conn) = tmp();
        write_record(&conn, "find_symbol", 42, "success", false, None).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn write_record_stores_all_fields() {
        let (_dir, conn) = tmp();
        write_record(&conn, "semantic_search", 150, "recoverable_error", false, Some("path not found")).unwrap();
        let (name, latency, outcome, overflowed, msg): (String, i64, String, i64, Option<String>) = conn
            .query_row(
                "SELECT tool_name, latency_ms, outcome, overflowed, error_msg FROM tool_calls",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(name, "semantic_search");
        assert_eq!(latency, 150);
        assert_eq!(outcome, "recoverable_error");
        assert_eq!(overflowed, 0);
        assert_eq!(msg.as_deref(), Some("path not found"));
    }

    #[test]
    fn write_record_overflow_flag() {
        let (_dir, conn) = tmp();
        write_record(&conn, "list_symbols", 80, "success", true, None).unwrap();
        let overflowed: i64 = conn
            .query_row("SELECT overflowed FROM tool_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(overflowed, 1);
    }

    #[test]
    fn retention_prunes_old_rows() {
        let (_dir, conn) = tmp();
        // Insert a row with a timestamp 31 days ago
        conn.execute(
            "INSERT INTO tool_calls (tool_name, called_at, latency_ms, outcome, overflowed)
             VALUES ('old_tool', datetime('now', '-31 days'), 10, 'success', 0)",
            [],
        ).unwrap();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 1);

        // Next write triggers pruning
        write_record(&conn, "new_tool", 5, "success", false, None).unwrap();
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls WHERE tool_name = 'old_tool'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0);
    }
}
```

**Step 2: Run to verify they fail**

```bash
cargo test usage::db::tests 2>&1 | head -20
```
Expected: compile error — module doesn't exist yet.

**Step 3: Implement**

`src/usage/db.rs`:
```rust
use std::path::Path;
use anyhow::Result;
use rusqlite::{Connection, params};

pub fn open_db(project_root: &Path) -> Result<Connection> {
    let path = project_root.join(".code-explorer").join("usage.db");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;

        CREATE TABLE IF NOT EXISTS tool_calls (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_name  TEXT NOT NULL,
            called_at  TEXT NOT NULL DEFAULT (datetime('now')),
            latency_ms INTEGER NOT NULL,
            outcome    TEXT NOT NULL,
            overflowed INTEGER NOT NULL DEFAULT 0,
            error_msg  TEXT
        );",
    )?;
    Ok(conn)
}

pub fn write_record(
    conn: &Connection,
    tool_name: &str,
    latency_ms: i64,
    outcome: &str,
    overflowed: bool,
    error_msg: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO tool_calls (tool_name, called_at, latency_ms, outcome, overflowed, error_msg)
         VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5)",
        params![tool_name, latency_ms, outcome, overflowed as i64, error_msg],
    )?;
    conn.execute(
        "DELETE FROM tool_calls WHERE called_at < datetime('now', '-30 days')",
        [],
    )?;
    Ok(())
}
```

`src/usage/mod.rs`:
```rust
pub mod db;
```

Add to `src/lib.rs`:
```rust
pub mod usage;
```

**Step 4: Run tests**

```bash
cargo test usage::db::tests
```
Expected: all 5 pass.

**Step 5: Lint and commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/usage/ src/lib.rs
git commit -m "feat(usage): add usage.db layer — open_db, write_record, retention"
```

---

### Task 2: DB layer — query_stats

**Files:**
- Modify: `src/usage/db.rs`

**Context:** `query_stats` returns per-tool aggregates for a time window. SQLite has no native percentile function — compute p50/p99 with `LIMIT 1 OFFSET` on a sorted subquery. `window_to_modifier` maps "1h"/"24h"/"7d"/"30d" to SQLite interval strings.

**Step 1: Write the failing tests**

Add to the `tests` module in `src/usage/db.rs`:

```rust
    fn insert_call(conn: &Connection, tool: &str, latency: i64, outcome: &str, overflowed: bool) {
        conn.execute(
            "INSERT INTO tool_calls (tool_name, called_at, latency_ms, outcome, overflowed)
             VALUES (?1, datetime('now'), ?2, ?3, ?4)",
            params![tool, latency, outcome, overflowed as i64],
        ).unwrap();
    }

    #[test]
    fn query_stats_empty_db() {
        let (_dir, conn) = tmp();
        let stats = query_stats(&conn, "30d").unwrap();
        assert_eq!(stats.total_calls, 0);
        assert!(stats.by_tool.is_empty());
    }

    #[test]
    fn query_stats_counts_correctly() {
        let (_dir, conn) = tmp();
        insert_call(&conn, "find_symbol", 100, "success", false);
        insert_call(&conn, "find_symbol", 200, "success", false);
        insert_call(&conn, "find_symbol", 300, "error", false);
        insert_call(&conn, "semantic_search", 500, "success", true);

        let stats = query_stats(&conn, "30d").unwrap();
        assert_eq!(stats.total_calls, 4);
        assert_eq!(stats.by_tool.len(), 2);

        // find_symbol should be first (3 calls > 1)
        let fs = &stats.by_tool[0];
        assert_eq!(fs.tool, "find_symbol");
        assert_eq!(fs.calls, 3);
        assert_eq!(fs.errors, 1);
        assert_eq!(fs.overflows, 0);

        let ss = &stats.by_tool[1];
        assert_eq!(ss.tool, "semantic_search");
        assert_eq!(ss.overflows, 1);
    }

    #[test]
    fn query_stats_percentiles() {
        let (_dir, conn) = tmp();
        // Insert 10 calls with known latencies 10..100ms
        for i in 1..=10 {
            insert_call(&conn, "find_symbol", i * 10, "success", false);
        }
        let stats = query_stats(&conn, "30d").unwrap();
        let fs = &stats.by_tool[0];
        // p50 = 50ms (5th of 10, 0-indexed offset 5)
        assert_eq!(fs.p50_ms, 50);
        // p99 = ~100ms (last item)
        assert_eq!(fs.p99_ms, 100);
    }

    #[test]
    fn query_stats_window_excludes_old_rows() {
        let (_dir, conn) = tmp();
        // Insert a row 2 days ago
        conn.execute(
            "INSERT INTO tool_calls (tool_name, called_at, latency_ms, outcome, overflowed)
             VALUES ('old_tool', datetime('now', '-2 days'), 50, 'success', 0)",
            [],
        ).unwrap();
        insert_call(&conn, "new_tool", 10, "success", false);

        let stats_1h = query_stats(&conn, "1h").unwrap();
        // Only new_tool (inserted now) should appear in 1h window
        assert_eq!(stats_1h.total_calls, 1);
        assert_eq!(stats_1h.by_tool[0].tool, "new_tool");
    }
```

**Step 2: Run to verify they fail**

```bash
cargo test usage::db::tests::query_stats 2>&1 | head -10
```
Expected: compile errors — `query_stats`, `ToolStats`, `UsageStats` not defined.

**Step 3: Implement**

Add to `src/usage/db.rs`:

```rust
#[derive(Debug, serde::Serialize)]
pub struct ToolStats {
    pub tool: String,
    pub calls: i64,
    pub errors: i64,
    pub error_rate_pct: f64,
    pub overflows: i64,
    pub overflow_rate_pct: f64,
    pub p50_ms: i64,
    pub p99_ms: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct UsageStats {
    pub window: String,
    pub total_calls: i64,
    pub by_tool: Vec<ToolStats>,
}

pub fn query_stats(conn: &Connection, window: &str) -> Result<UsageStats> {
    let modifier = window_to_modifier(window);
    let mut stmt = conn.prepare(
        "SELECT tool_name,
                COUNT(*) as calls,
                SUM(CASE WHEN outcome IN ('error', 'recoverable_error') THEN 1 ELSE 0 END) as errors,
                SUM(overflowed) as overflows
         FROM tool_calls
         WHERE called_at >= datetime('now', ?)
         GROUP BY tool_name
         ORDER BY calls DESC",
    )?;

    let rows: Vec<(String, i64, i64, i64)> = stmt
        .query_map([modifier], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let total_calls: i64 = rows.iter().map(|r| r.1).sum();

    let mut by_tool = Vec::new();
    for (tool_name, calls, errors, overflows) in rows {
        let p50_ms = percentile(conn, &tool_name, modifier, 50)?;
        let p99_ms = percentile(conn, &tool_name, modifier, 99)?;
        by_tool.push(ToolStats {
            error_rate_pct: if calls > 0 { errors as f64 / calls as f64 * 100.0 } else { 0.0 },
            overflow_rate_pct: if calls > 0 { overflows as f64 / calls as f64 * 100.0 } else { 0.0 },
            tool: tool_name,
            calls,
            errors,
            overflows,
            p50_ms,
            p99_ms,
        });
    }

    Ok(UsageStats { window: window.to_string(), total_calls, by_tool })
}

fn percentile(conn: &Connection, tool_name: &str, modifier: &str, pct: i64) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_calls WHERE tool_name = ? AND called_at >= datetime('now', ?)",
        params![tool_name, modifier],
        |r| r.get(0),
    )?;
    if count == 0 {
        return Ok(0);
    }
    let offset = (count * pct / 100).max(0);
    let val: i64 = conn.query_row(
        "SELECT latency_ms FROM tool_calls
         WHERE tool_name = ? AND called_at >= datetime('now', ?)
         ORDER BY latency_ms
         LIMIT 1 OFFSET ?",
        params![tool_name, modifier, offset],
        |r| r.get(0),
    )?;
    Ok(val)
}

fn window_to_modifier(window: &str) -> &'static str {
    match window {
        "1h"  => "-1 hours",
        "24h" => "-24 hours",
        "7d"  => "-7 days",
        _     => "-30 days",
    }
}
```

**Step 4: Run tests**

```bash
cargo test usage::db::tests
```
Expected: all tests pass.

**Step 5: Lint and commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/usage/db.rs
git commit -m "feat(usage): add query_stats with per-tool counts, error/overflow rates, p50/p99"
```

---

### Task 3: `UsageRecorder` struct

**Files:**
- Modify: `src/usage/mod.rs`

**Context:** `UsageRecorder` holds an `Agent` clone. `record()` times the closure, classifies `Result<Value>` into outcome/overflow, and writes to the DB best-effort. Classification reads `v["error"]` for recoverable errors and `v["overflow"]` for overflow — both are existing patterns in tool output JSON.

**Step 1: Write the failing tests**

Add to `src/usage/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_error_result() {
        let r: anyhow::Result<serde_json::Value> = Err(anyhow::anyhow!("boom"));
        let (outcome, overflowed, msg) = classify_result(&r);
        assert_eq!(outcome, "error");
        assert!(!overflowed);
        assert_eq!(msg.as_deref(), Some("boom"));
    }

    #[test]
    fn classify_recoverable_error() {
        let v = json!({ "error": "path not found", "hint": "check path" });
        let r: anyhow::Result<serde_json::Value> = Ok(v);
        let (outcome, overflowed, msg) = classify_result(&r);
        assert_eq!(outcome, "recoverable_error");
        assert!(!overflowed);
        assert_eq!(msg.as_deref(), Some("path not found"));
    }

    #[test]
    fn classify_overflow_success() {
        let v = json!({ "symbols": [], "overflow": { "shown": 200, "total": 500 } });
        let r: anyhow::Result<serde_json::Value> = Ok(v);
        let (outcome, overflowed, _msg) = classify_result(&r);
        assert_eq!(outcome, "success");
        assert!(overflowed);
    }

    #[test]
    fn classify_clean_success() {
        let v = json!({ "symbols": [{"name": "foo"}] });
        let r: anyhow::Result<serde_json::Value> = Ok(v);
        let (outcome, overflowed, msg) = classify_result(&r);
        assert_eq!(outcome, "success");
        assert!(!overflowed);
        assert!(msg.is_none());
    }
}
```

**Step 2: Run to verify they fail**

```bash
cargo test usage::tests 2>&1 | head -10
```
Expected: compile error — `classify_result` not defined.

**Step 3: Implement**

Replace `src/usage/mod.rs` with:

```rust
pub mod db;

use std::time::Instant;
use anyhow::Result;
use serde_json::Value;
use crate::agent::Agent;

pub struct UsageRecorder {
    agent: Agent,
}

impl UsageRecorder {
    pub fn new(agent: Agent) -> Self {
        Self { agent }
    }

    pub async fn record<F, Fut>(&self, tool_name: &str, f: F) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Value>>,
    {
        let start = Instant::now();
        let result = f().await;
        let latency_ms = start.elapsed().as_millis() as i64;
        // Best-effort — never let recording fail the tool call
        let _ = self.write(tool_name, latency_ms, &result).await;
        result
    }

    async fn write(&self, tool_name: &str, latency_ms: i64, result: &Result<Value>) -> Result<()> {
        let project_root = self.agent.with_project(|p| Ok(p.root.clone())).await?;
        let conn = db::open_db(&project_root)?;
        let (outcome, overflowed, error_msg) = classify_result(result);
        db::write_record(&conn, tool_name, latency_ms, outcome, overflowed, error_msg.as_deref())?;
        Ok(())
    }
}

pub(crate) fn classify_result(result: &Result<Value>) -> (&'static str, bool, Option<String>) {
    match result {
        Err(e) => ("error", false, Some(e.to_string())),
        Ok(v) => {
            if let Some(msg) = v.get("error").and_then(Value::as_str) {
                ("recoverable_error", false, Some(msg.to_string()))
            } else if v.get("overflow").is_some() {
                ("success", true, None)
            } else {
                ("success", false, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // ... (tests from above)
}
```

**Step 4: Run tests**

```bash
cargo test usage::tests
```
Expected: all 4 classification tests pass.

**Step 5: Lint and commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/usage/mod.rs
git commit -m "feat(usage): add UsageRecorder with best-effort record() and classify_result"
```

---

### Task 4: `get_usage_stats` tool

**Files:**
- Create: `src/tools/usage.rs`
- Modify: `src/tools/mod.rs` (add `pub mod usage;` and re-export `GetUsageStats`)

**Context:** Follow the pattern of any existing tool — implement the `Tool` trait with `name()`, `description()`, `input_schema()`, and `async call()`. Return `RecoverableError` when no project is active. The `description()` should include routing guidance so the LLM knows *when* to call it.

**Step 1: Write the failing test**

Add to `src/tools/usage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::tools::ToolContext;
    use std::sync::Arc;
    use crate::lsp::manager::LspManager;
    use tempfile::TempDir;

    async fn ctx_with_project(root: &std::path::Path) -> ToolContext {
        let agent = Agent::new(Some(root.to_path_buf())).await.unwrap();
        ToolContext {
            agent,
            lsp: Arc::new(LspManager::new()),
        }
    }

    #[tokio::test]
    async fn returns_empty_stats_on_fresh_project() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_project(dir.path()).await;
        let tool = GetUsageStats;
        let result = tool.call(serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(result["total_calls"], 0);
        assert_eq!(result["window"], "30d");
        assert!(result["by_tool"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn returns_error_without_active_project() {
        let agent = Agent::new(None).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: Arc::new(LspManager::new()),
        };
        let tool = GetUsageStats;
        let result = tool.call(serde_json::json!({}), &ctx).await.unwrap();
        // RecoverableError returns Ok with "error" key
        assert!(result["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn respects_window_parameter() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_project(dir.path()).await;
        let tool = GetUsageStats;
        let result = tool.call(serde_json::json!({"window": "1h"}), &ctx).await.unwrap();
        assert_eq!(result["window"], "1h");
    }
}
```

**Step 2: Run to verify they fail**

```bash
cargo test tools::usage::tests 2>&1 | head -10
```
Expected: compile error — module and struct not defined.

**Step 3: Implement**

`src/tools/usage.rs`:
```rust
use async_trait::async_trait;
use serde_json::Value;
use anyhow::Result;
use crate::tools::{Tool, ToolContext, RecoverableError};

pub struct GetUsageStats;

#[async_trait]
impl Tool for GetUsageStats {
    fn name(&self) -> &str {
        "get_usage_stats"
    }

    fn description(&self) -> &str {
        "Get tool call statistics for the current project. Returns per-tool call counts, \
         error rates, overflow rates, and latency percentiles (p50/p99) for a time window. \
         Use this to diagnose agent behavior: high overflow_rate_pct means queries are too \
         broad; high error_rate_pct on a tool means it is failing repeatedly. \
         Prefer this over manual log inspection."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "enum": ["1h", "24h", "7d", "30d"],
                    "description": "Time window for aggregation. Default: 30d."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        let window = input["window"].as_str().unwrap_or("30d");

        let project_root = ctx
            .agent
            .with_project(|p| Ok(p.root.clone()))
            .await
            .map_err(|_| {
                RecoverableError::with_hint(
                    "no active project",
                    "run activate_project first",
                )
            })?;

        let conn = crate::usage::db::open_db(&project_root)?;
        let stats = crate::usage::db::query_stats(&conn, window)?;
        Ok(serde_json::to_value(stats)?)
    }
}
```

Add to `src/tools/mod.rs`:
```rust
pub mod usage;
pub use usage::GetUsageStats;
```

**Step 4: Run tests**

```bash
cargo test tools::usage::tests
```
Expected: all 3 pass.

**Step 5: Lint and commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/tools/usage.rs src/tools/mod.rs
git commit -m "feat(tools): add get_usage_stats tool"
```

---

### Task 5: Wire `UsageRecorder` into `call_tool` and register tool

**Files:**
- Modify: `src/server.rs`

**Context:** `UsageRecorder` is constructed at the top of `call_tool` alongside `ToolContext`. The `tool.call(input, &ctx)` call becomes `recorder.record(&req.name, || tool.call(input, &ctx))`. The timeout wrapper stays around `recorder.record()` unchanged — see existing code at `server.rs:170-200`. Add `Arc::new(GetUsageStats)` to the tools vec at `server.rs:58-100`.

**Step 1: Add `GetUsageStats` to the tools registry**

In `src/server.rs`, find the `tools` vec in `from_parts` and add at the end:

```rust
// Usage monitoring
Arc::new(GetUsageStats),
```

Also add to the imports at the top of `server.rs`:
```rust
use crate::tools::GetUsageStats;
use crate::usage::UsageRecorder;
```

**Step 2: Wrap `tool.call()` in `call_tool`**

Find this line in `call_tool` (around line 186):
```rust
let result = if let Some(secs) = timeout_secs {
    tokio::time::timeout(..., tool.call(input, &ctx))
        ...
} else {
    tool.call(input, &ctx).await
};
```

Add `UsageRecorder` construction just before the `is_long_running` check, and wrap `tool.call`:

```rust
let recorder = UsageRecorder::new(self.agent.clone());

let result = if let Some(secs) = timeout_secs {
    tokio::time::timeout(
        std::time::Duration::from_secs(secs),
        recorder.record(&req.name, || tool.call(input, &ctx)),
    )
    .await
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "Tool '{}' timed out after {}s. \
             Increase tool_timeout_secs in .code-explorer/project.toml if needed.",
            req.name,
            secs
        ))
    })
} else {
    recorder.record(&req.name, || tool.call(input, &ctx)).await
};
```

**Step 3: Run the full test suite**

```bash
cargo test
```
Expected: all tests pass (no regressions).

**Step 4: Smoke test manually**

```bash
cargo run -- start --project .
# In another terminal, call get_usage_stats via MCP after a few tool calls
```

**Step 5: Final lint and commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/server.rs
git commit -m "feat(server): wire UsageRecorder into call_tool, register get_usage_stats"
```

---

## Summary

| Task | New files | Modified files |
|------|-----------|---------------|
| 1 — DB layer (schema + write) | `src/usage/db.rs`, `src/usage/mod.rs` | `src/lib.rs` |
| 2 — DB layer (query_stats) | — | `src/usage/db.rs` |
| 3 — UsageRecorder | — | `src/usage/mod.rs` |
| 4 — get_usage_stats tool | `src/tools/usage.rs` | `src/tools/mod.rs` |
| 5 — Wire up | — | `src/server.rs` |

Total: 2 new source files, 4 modified, ~5 commits.

---

*Created: 2026-02-28*
*Design doc: `docs/plans/2026-02-28-tool-usage-monitor-design.md`*
