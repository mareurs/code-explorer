use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

pub fn open_db(project_root: &Path) -> Result<Connection> {
    let path = project_root.join(".codescout").join("usage.db");
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
        .query_map([modifier], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let total_calls: i64 = rows.iter().map(|r| r.1).sum();

    let mut by_tool = Vec::new();
    for (tool_name, calls, errors, overflows) in rows {
        let p50_ms = percentile(conn, &tool_name, modifier, 50)?;
        let p99_ms = percentile(conn, &tool_name, modifier, 99)?;
        by_tool.push(ToolStats {
            error_rate_pct: if calls > 0 {
                errors as f64 / calls as f64 * 100.0
            } else {
                0.0
            },
            overflow_rate_pct: if calls > 0 {
                overflows as f64 / calls as f64 * 100.0
            } else {
                0.0
            },
            tool: tool_name,
            calls,
            errors,
            overflows,
            p50_ms,
            p99_ms,
        });
    }

    Ok(UsageStats {
        window: window.to_string(),
        total_calls,
        by_tool,
    })
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
    // Nearest-rank method: ceil(count * pct / 100) - 1 (0-indexed)
    let offset = ((count * pct + 99) / 100 - 1).max(0);
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
        "1h" => "-1 hours",
        "24h" => "-24 hours",
        "7d" => "-7 days",
        _ => "-30 days",
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ErrorRecord {
    pub tool: String,
    pub timestamp: String,
    pub outcome: String,
    pub message: Option<String>,
}

pub fn recent_errors(conn: &Connection, limit: i64) -> Result<Vec<ErrorRecord>> {
    let mut stmt = conn.prepare(
        "SELECT tool_name, called_at, outcome, error_msg
         FROM tool_calls
         WHERE outcome IN ('error', 'recoverable_error')
         ORDER BY called_at DESC, rowid DESC
         LIMIT ?",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(ErrorRecord {
                tool: r.get(0)?,
                timestamp: r.get(1)?,
                outcome: r.get(2)?,
                message: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

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
        conn.execute("SELECT 1 FROM tool_calls LIMIT 0", [])
            .unwrap();
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
        write_record(
            &conn,
            "semantic_search",
            150,
            "recoverable_error",
            false,
            Some("path not found"),
        )
        .unwrap();
        let (name, latency, outcome, overflowed, msg): (String, i64, String, i64, Option<String>) =
            conn.query_row(
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
        )
        .unwrap();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 1);

        // Next write triggers pruning
        write_record(&conn, "new_tool", 5, "success", false, None).unwrap();
        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE tool_name = 'old_tool'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 0);
    }

    fn insert_call(conn: &Connection, tool: &str, latency: i64, outcome: &str, overflowed: bool) {
        conn.execute(
            "INSERT INTO tool_calls (tool_name, called_at, latency_ms, outcome, overflowed)
             VALUES (?1, datetime('now'), ?2, ?3, ?4)",
            params![tool, latency, outcome, overflowed as i64],
        )
        .unwrap();
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
        )
        .unwrap();
        insert_call(&conn, "new_tool", 10, "success", false);

        let stats_1h = query_stats(&conn, "1h").unwrap();
        // Only new_tool (inserted now) should appear in 1h window
        assert_eq!(stats_1h.total_calls, 1);
        assert_eq!(stats_1h.by_tool[0].tool, "new_tool");
    }

    #[test]
    fn recent_errors_returns_latest_errors() {
        let (_dir, conn) = tmp();
        write_record(&conn, "find_symbol", 50, "success", false, None).unwrap();
        write_record(
            &conn,
            "semantic_search",
            100,
            "error",
            false,
            Some("index missing"),
        )
        .unwrap();
        write_record(
            &conn,
            "list_symbols",
            30,
            "recoverable_error",
            false,
            Some("path not found"),
        )
        .unwrap();

        let errors = recent_errors(&conn, 10).unwrap();
        assert_eq!(errors.len(), 2);
        // Most recent first
        assert_eq!(errors[0].tool, "list_symbols");
        assert_eq!(errors[1].tool, "semantic_search");
    }

    #[test]
    fn recent_errors_respects_limit() {
        let (_dir, conn) = tmp();
        for i in 0..5 {
            write_record(
                &conn,
                &format!("tool_{}", i),
                10,
                "error",
                false,
                Some("fail"),
            )
            .unwrap();
        }
        let errors = recent_errors(&conn, 3).unwrap();
        assert_eq!(errors.len(), 3);
    }
}
