# vec0 Virtual Table Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the plain `chunk_embeddings` BLOB table with a `vec0` virtual table so similarity search uses ANN indexing instead of an O(n) full scan.

**Architecture:** `maybe_migrate_to_vec0` runs at the end of every `open_db` — if `embedding_dims` is in `meta` and `chunk_embeddings` is still a plain table, it renames the old table, creates the `vec0` virtual table, copies the existing blobs (already le-f32 format — no re-embedding), and drops the backup. `build_index` / `build_library_index` write `embedding_dims` to `meta` after the first successful embedding batch so the migration triggers automatically on the next open. `search_scoped` detects the active table type and routes to a KNN subquery when on vec0.

**Tech Stack:** Rust, rusqlite 0.31, sqlite-vec 0.1 (already loaded via `init_sqlite_vec`), tempfile (tests).

**File:** `src/embed/index.rs` — all changes are in this single file.

---

### Task 1: Add `maybe_migrate_to_vec0` + `open_test_db_vec0`

**Files:**
- Modify: `src/embed/index.rs` (after `open_db`, before `hash_file` — around line 128)

**Step 1: Write the failing tests**

Add these three tests to the `tests` module at the bottom of `src/embed/index.rs`:

```rust
#[test]
fn vec0_migration_skips_when_no_dims() {
    let (_dir, conn) = open_test_db();
    // No embedding_dims in meta → migration is a no-op, plain table stays
    maybe_migrate_to_vec0(&conn).unwrap();
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunk_embeddings'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let sql = sql.unwrap();
    assert!(!sql.contains("USING vec0"), "expected plain table, got: {sql}");
}

#[test]
fn vec0_migration_upgrades_plain_table() {
    let (_dir, conn) = open_test_db();
    // Insert a chunk so there is data to migrate
    insert_chunk(&conn, &dummy_chunk("a.rs", "fn a() {}"), &[0.1_f32, 0.2_f32]).unwrap();
    set_meta(&conn, "embedding_dims", "2").unwrap();

    maybe_migrate_to_vec0(&conn).unwrap();

    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='chunk_embeddings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("USING vec0"), "expected vec0 virtual table, got: {sql}");

    // Data must survive the migration
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn vec0_migration_is_idempotent() {
    let (_dir, conn) = open_test_db();
    insert_chunk(&conn, &dummy_chunk("a.rs", "fn a() {}"), &[0.1_f32, 0.2_f32]).unwrap();
    set_meta(&conn, "embedding_dims", "2").unwrap();

    maybe_migrate_to_vec0(&conn).unwrap();
    // Second call must not error
    maybe_migrate_to_vec0(&conn).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test vec0_migration 2>&1 | tail -20
```
Expected: three compile errors — `maybe_migrate_to_vec0` not found.

**Step 3: Implement `maybe_migrate_to_vec0` and `open_test_db_vec0`**

Add this block immediately after the closing `}` of `open_db` (around line 127), before `hash_file`:

```rust
/// Migrate `chunk_embeddings` from a plain BLOB table to a `vec0` virtual
/// table if `embedding_dims` is stored in meta and the table is not yet
/// a virtual table. Safe to call multiple times (idempotent).
pub fn maybe_migrate_to_vec0(conn: &Connection) -> Result<()> {
    use rusqlite::OptionalExtension;

    let dims: usize = match get_meta(conn, "embedding_dims")? {
        Some(s) => s.parse().unwrap_or(0),
        None => return Ok(()),
    };
    if dims == 0 {
        return Ok(());
    }

    // Check whether chunk_embeddings is still a plain table.
    // Both plain tables and virtual tables have type='table' in sqlite_master,
    // but the sql column differs.
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunk_embeddings'",
            [],
            |r| r.get(0),
        )
        .optional()?;

    match sql {
        None => return Ok(()), // table doesn't exist yet
        Some(s) if s.contains("USING vec0") => return Ok(()), // already migrated
        _ => {}
    }

    tracing::info!("Migrating chunk_embeddings to vec0 virtual table (dims={dims})");

    conn.execute_batch("ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1")?;
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[{dims}])"
    ))?;
    conn.execute_batch(
        "INSERT INTO chunk_embeddings(rowid, embedding) \
         SELECT rowid, embedding FROM chunk_embeddings_v1",
    )?;
    conn.execute_batch("DROP TABLE chunk_embeddings_v1")?;

    tracing::info!("vec0 migration complete");
    Ok(())
}
```

Also add `open_test_db_vec0` inside the `#[cfg(test)]` tests module (after `open_test_db`):

```rust
fn open_test_db_vec0(dims: usize) -> (tempfile::TempDir, Connection) {
    let (dir, conn) = open_test_db();
    set_meta(&conn, "embedding_dims", &dims.to_string()).unwrap();
    maybe_migrate_to_vec0(&conn).unwrap();
    (dir, conn)
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test vec0_migration 2>&1 | tail -20
```
Expected: `test tests::vec0_migration_skips_when_no_dims ... ok`, `...upgrades_plain_table ... ok`, `...is_idempotent ... ok`

**Step 5: Verify no regressions**

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass.

**Step 6: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): add maybe_migrate_to_vec0 for lazy schema upgrade"
```

---

### Task 2: Call `maybe_migrate_to_vec0` from `open_db`

**Files:**
- Modify: `src/embed/index.rs:open_db` (around line 61–127)

**Step 1: Write the failing test**

Add to the tests module:

```rust
#[test]
fn open_db_auto_migrates_when_dims_present() {
    let dir = tempfile::tempdir().unwrap();
    let conn1 = open_db(dir.path()).unwrap();
    // Insert a chunk and store dims (simulates a post-indexing state)
    insert_chunk(&conn1, &dummy_chunk("x.rs", "fn x() {}"), &[1.0_f32, 0.0_f32]).unwrap();
    set_meta(&conn1, "embedding_dims", "2").unwrap();
    drop(conn1);

    // Re-open — open_db should auto-migrate
    let conn2 = open_db(dir.path()).unwrap();
    let sql: String = conn2
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='chunk_embeddings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("USING vec0"), "expected vec0 after reopen, got: {sql}");
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test open_db_auto_migrates 2>&1 | tail -10
```
Expected: FAIL — assertion `sql.contains("USING vec0")` fails.

**Step 3: Add the call to `open_db`**

In `open_db`, after the two migration blocks for `mtime` and `source` (around line 120), add:

```rust
    maybe_migrate_to_vec0(&conn)?;
```

The end of `open_db` should look like:

```rust
    // Migrate: add source column to chunks if missing (safe no-op if already present)
    let has_source: bool = conn.prepare("SELECT source FROM chunks LIMIT 0").is_ok();
    if !has_source {
        conn.execute_batch("ALTER TABLE chunks ADD COLUMN source TEXT NOT NULL DEFAULT 'project'")?;
    }

    maybe_migrate_to_vec0(&conn)?;

    Ok(conn)
```

**Step 4: Run test to verify it passes**

```bash
cargo test open_db_auto_migrates 2>&1 | tail -10
```
Expected: PASS.

**Step 5: Verify no regressions**

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass.

**Step 6: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): auto-migrate to vec0 on open_db when embedding_dims known"
```

---

### Task 3: Store `embedding_dims` in `build_index` and `build_library_index`

**Files:**
- Modify: `src/embed/index.rs:build_index` (Phase 3, around line 638)
- Modify: `src/embed/index.rs:build_library_index` (Phase 3, around line 851)

No new unit tests — this is an integration concern verified by the Task 2 test path (the combination of storing dims + auto-migration is tested end-to-end by `open_db_auto_migrates_when_dims_present`).

**Step 1: Update `build_index` Phase 3**

In `build_index`, find the line `conn.execute_batch("BEGIN")?;` (Phase 3, around line 638). Add the dims storage immediately after it:

```rust
    conn.execute_batch("BEGIN")?;
    // Store embedding dims for vec0 migration. Derived from the first result's
    // first embedding so no extra API call is needed. No-op if no files indexed.
    if let Some(dims) = results
        .first()
        .and_then(|r| r.embeddings.first())
        .map(|e| e.len())
    {
        set_meta(&conn, "embedding_dims", &dims.to_string())?;
    }
    // Always clear drift data ...
    clear_drift_report(&conn)?;
```

**Step 2: Update `build_library_index` Phase 3**

In `build_library_index`, find `conn.execute_batch("BEGIN")?;` (around line 851). Add the same block after it:

```rust
    conn.execute_batch("BEGIN")?;
    if let Some(dims) = results
        .first()
        .and_then(|r| r.embeddings.first())
        .map(|e| e.len())
    {
        set_meta(&conn, "embedding_dims", &dims.to_string())?;
    }
    for result in results {
```

**Step 3: Build to verify no compile errors**

```bash
cargo build 2>&1 | tail -10
```
Expected: `Compiling code-explorer` then `Finished`.

**Step 4: Run all tests**

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass.

**Step 5: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): store embedding_dims in meta during indexing"
```

---

### Task 4: Update `search_scoped` to use vec0 KNN query

**Files:**
- Modify: `src/embed/index.rs` — add `is_vec0_active`, add `search_scoped_vec0`, update `search_scoped`

**Step 1: Write the failing tests**

Add to the tests module:

```rust
#[test]
fn vec0_search_returns_closest_vector() {
    let (_dir, conn) = open_test_db_vec0(4);
    insert_chunk(&conn, &dummy_chunk("a.rs", "fn a() {}"), &[1.0_f32, 0.0, 0.0, 0.0]).unwrap();
    insert_chunk(&conn, &dummy_chunk("b.rs", "fn b() {}"), &[0.0_f32, 1.0, 0.0, 0.0]).unwrap();

    let results = search(&conn, &[0.9_f32, 0.1, 0.0, 0.0], 1).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "a.rs");
    assert!(results[0].score > 0.9, "score={}", results[0].score);
}

#[test]
fn vec0_search_respects_limit() {
    let (_dir, conn) = open_test_db_vec0(2);
    for i in 0..5u8 {
        insert_chunk(
            &conn,
            &dummy_chunk(&format!("{i}.rs"), "fn f() {}"),
            &[i as f32, 0.0_f32],
        )
        .unwrap();
    }
    let results = search(&conn, &[1.0_f32, 0.0], 3).unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn vec0_search_scoped_filters_by_source() {
    let (_dir, conn) = open_test_db_vec0(2);
    let mut proj = dummy_chunk_with_source("p.rs", "fn p() {}", "project");
    insert_chunk(&conn, &proj, &[1.0_f32, 0.0]).unwrap();
    proj = dummy_chunk_with_source("l.rs", "fn l() {}", "mylib");
    insert_chunk(&conn, &proj, &[0.9_f32, 0.1]).unwrap();

    let all = search_scoped(&conn, &[1.0_f32, 0.0], 10, None).unwrap();
    assert_eq!(all.len(), 2);

    let proj_only = search_scoped(&conn, &[1.0_f32, 0.0], 10, Some("project")).unwrap();
    assert_eq!(proj_only.len(), 1);
    assert_eq!(proj_only[0].source, "project");

    let libs_only = search_scoped(&conn, &[1.0_f32, 0.0], 10, Some("libraries")).unwrap();
    assert_eq!(libs_only.len(), 1);
    assert_eq!(libs_only[0].source, "mylib");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test vec0_search 2>&1 | tail -20
```
Expected: all three FAIL — `is_vec0_active` and `search_scoped_vec0` not found, or assertion failures because search still uses full-scan query that works on plain table (so the tests actually pass with the plain-table path). Re-check: `open_test_db_vec0` sets up a vec0 table, but `search_scoped` still uses the full-scan query. The `vec_distance_cosine(vec_f32(ce.embedding), vec_f32(?1))` call on a vec0 table may or may not work — vec0 exposes `embedding` as readable, so it likely works.

**Important:** Before writing the new implementation, confirm the tests currently pass or fail by running them. If they already pass (full-scan still works on vec0), the implementation goal is performance not correctness, so the refactor is safe to do without "breaking first".

```bash
cargo test vec0_search 2>&1 | tail -20
```
Note the results. Proceed regardless — we're refactoring to the KNN path for performance.

**Step 3: Add `is_vec0_active` helper**

Add this private function immediately before `search_scoped` (around line 286):

```rust
/// Returns true when `chunk_embeddings` is a vec0 virtual table.
/// Checked via sqlite_master DDL — O(1) index lookup.
fn is_vec0_active(conn: &Connection) -> bool {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunk_embeddings'",
        [],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .map(|sql| sql.contains("USING vec0"))
    .unwrap_or(false)
}
```

**Step 4: Add `search_scoped_vec0` function**

Add this function immediately after `search_scoped` (around line 346):

```rust
/// KNN search via vec0 virtual table. Called by `search_scoped` when the
/// table has been migrated. `ORDER BY + LIMIT` must live inside the vec0
/// subquery — this is a vec0 requirement, not a SQL convention.
fn search_scoped_vec0(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    source_filter: Option<&str>,
) -> Result<Vec<SearchResult>> {
    let query_blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<SearchResult> {
        let distance: f64 = row.get(6)?;
        let score = (1.0_f32 - distance as f32).clamp(0.0, 1.0);
        Ok(SearchResult {
            file_path: row.get(0)?,
            language: row.get(1)?,
            content: row.get(2)?,
            start_line: row.get(3)?,
            end_line: row.get(4)?,
            source: row.get(5)?,
            score,
        })
    };

    // KNN subquery: LIMIT must be here for vec0's query planner.
    let knn = "SELECT rowid, distance FROM chunk_embeddings \
               WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2";

    let sel = format!(
        "SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source, \
         knn.distance \
         FROM chunks c JOIN ({knn}) knn ON c.id = knn.rowid"
    );

    match source_filter {
        None => {
            let sql = format!("{sel} ORDER BY knn.distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some("libraries") => {
            let sql = format!("{sel} WHERE c.source != 'project' ORDER BY knn.distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some(source) => {
            let sql = format!("{sel} WHERE c.source = ?3 ORDER BY knn.distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64, source], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
    }
}
```

**Step 5: Update `search_scoped` to dispatch to vec0 path**

At the very start of `search_scoped`, before the `query_blob` declaration, add the dispatch:

```rust
pub fn search_scoped(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    source_filter: Option<&str>,
) -> Result<Vec<SearchResult>> {
    if is_vec0_active(conn) {
        return search_scoped_vec0(conn, query_embedding, limit, source_filter);
    }
    // ... rest of existing full-scan implementation unchanged ...
```

**Step 6: Run the vec0 search tests**

```bash
cargo test vec0_search 2>&1 | tail -20
```
Expected: all three pass.

**Step 7: Run full test suite**

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass. Key tests to watch: `cosine_search_returns_closest_vector`, `cosine_search_respects_limit`, `search_returns_source`, `search_scoped_filters_by_source` — these use `open_test_db()` (no dims), so they exercise the plain-table path and must still pass.

**Step 8: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): use vec0 KNN query in search_scoped when table is migrated"
```

---

### Task 5: Final polish — clippy, fmt, full verification

**Step 1: Format**

```bash
cargo fmt
```

**Step 2: Lint**

```bash
cargo clippy -- -D warnings 2>&1 | tail -20
```
Fix any warnings before continuing.

**Step 3: Full test suite**

```bash
cargo test 2>&1 | tail -10
```
Expected: all tests pass. Note total test count — should be higher than the pre-task count by 7 (3 migration + 1 auto-migrate + 3 vec0 search).

**Step 4: Final commit (if fmt/clippy made changes)**

```bash
git add -p  # review before staging
git commit -m "style: fmt and clippy fixes for vec0 migration"
```

If no changes, skip this step.

---

## Summary of Changes

| Function | Change |
|----------|--------|
| `maybe_migrate_to_vec0` (new) | Detects plain table + known dims → renames, creates vec0, copies blobs, drops backup |
| `open_db` | Calls `maybe_migrate_to_vec0` at end |
| `build_index` Phase 3 | Stores `embedding_dims` in meta after `BEGIN` |
| `build_library_index` Phase 3 | Same |
| `is_vec0_active` (new) | Checks `sqlite_master` DDL for `USING vec0` |
| `search_scoped_vec0` (new) | KNN query using `WHERE embedding MATCH vec_f32(?) ORDER BY distance LIMIT ?` |
| `search_scoped` | Dispatches to `search_scoped_vec0` when vec0 active |
| `open_test_db_vec0` (new, tests) | Helper that sets dims and migrates before returning |

## New Tests Added

`vec0_migration_skips_when_no_dims`, `vec0_migration_upgrades_plain_table`, `vec0_migration_is_idempotent`, `open_db_auto_migrates_when_dims_present`, `vec0_search_returns_closest_vector`, `vec0_search_respects_limit`, `vec0_search_scoped_filters_by_source`
