# vec0 Virtual Table Migration Design

**Date:** 2026-02-28
**Status:** Approved
**Goal:** Replace `chunk_embeddings` plain BLOB table with a `vec0` virtual table for ANN-indexed KNN search, eliminating the O(n) full scan on large codebases.

---

## Context

`sqlite-vec` is already loaded and `search_scoped` already calls `vec_distance_cosine` in SQL, but `chunk_embeddings` is still a plain table. Every search scans all rows. For large repos (10k+ chunks) this is a meaningful bottleneck. The `vec0` virtual table provides ANN indexing for sub-linear KNN queries.

---

## Schema

### Before (plain table)
```sql
CREATE TABLE IF NOT EXISTS chunk_embeddings (
    rowid     INTEGER PRIMARY KEY,
    embedding BLOB NOT NULL
);
```

### After (vec0 virtual table)
```sql
CREATE VIRTUAL TABLE chunk_embeddings USING vec0(
    embedding float[N]   -- N stored in meta as "embedding_dims"
);
```

`vec0` auto-creates a `rowid` column. The existing `INSERT INTO chunk_embeddings (rowid, embedding)` pattern in `insert_chunk` is unchanged.

---

## Dimension Discovery

`vec0(embedding float[N])` requires `N` at `CREATE TABLE` time. `RemoteEmbedder::dimensions()` returns `0` (unknown until first embed). Strategy: **lazy, stored in meta**.

- After the first successful embedding batch in `build_index` / `build_library_index`, call `set_meta(conn, "embedding_dims", &dims.to_string())` before inserting any chunks.
- Subsequent `open_db` calls read `embedding_dims` from meta and trigger migration if needed.

---

## Migration Flow

`maybe_migrate_to_vec0(conn: &Connection) -> Result<()>` — called at the end of `open_db`.

1. Read `embedding_dims` from `meta`. If missing → return `Ok(())` (index never built, nothing to migrate).
2. Check `sqlite_master`: `SELECT type FROM sqlite_master WHERE name = 'chunk_embeddings'`.
   - `'table'` → migration needed.
   - `'shadow'` or missing → already migrated or empty, return `Ok(())`.
3. `ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1`
4. `CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[N])`
5. `INSERT INTO chunk_embeddings(rowid, embedding) SELECT rowid, embedding FROM chunk_embeddings_v1`
6. `DROP TABLE chunk_embeddings_v1`

Migration is idempotent: if it fails mid-way, the next `open_db` recreates the plain `chunk_embeddings` via `CREATE TABLE IF NOT EXISTS`, detects `type = 'table'` again, and retries from the renamed `chunk_embeddings_v1` data.

---

## Search Query Changes

### Current (full O(n) scan)
```sql
SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source,
       COALESCE(vec_distance_cosine(vec_f32(ce.embedding), vec_f32(?1)), 1.0) AS distance
FROM chunks c JOIN chunk_embeddings ce ON c.id = ce.rowid
ORDER BY distance ASC LIMIT ?2
```

### New (vec0 KNN — sub-linear)
```sql
SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source,
       knn.distance
FROM chunks c
JOIN (
    SELECT rowid, distance FROM chunk_embeddings
    WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2
) knn ON c.id = knn.rowid
ORDER BY knn.distance ASC
```

- `LIMIT` must be in the inner KNN subquery — `vec0` requires it there.
- Distance is cosine distance ∈ [0, 1]. `score = (1.0 - distance).clamp(0.0, 1.0)` unchanged.
- Source filters (`WHERE c.source = ?`, `WHERE c.source != 'project'`) apply in the outer query. KNN fetches N nearest globally, outer filters by source. May return fewer than `limit` results when source-filtered — same behaviour as today.

**`read_file_embeddings`** (drift detection): reads `ce.embedding` via `JOIN chunk_embeddings ce ON c.id = ce.rowid`. `vec0` exposes `embedding` as a readable column — no change needed.

---

## Error Handling

**Migration failure mid-way:** If the rename succeeds but copy fails, DB is left with `chunk_embeddings_v1` but no `chunk_embeddings`. Next `open_db` recreates the plain table via `CREATE TABLE IF NOT EXISTS`, migration re-detects `type = 'table'` and retries. Effectively idempotent.

**`sqlite-vec` unavailable:** `init_sqlite_vec()` runs inside a `Once` guard. If the extension fails to load, `vec_version()` will be uncallable. `maybe_migrate_to_vec0` should verify the extension is present (attempt `SELECT vec_version()`) before migrating; on failure, log a warning and skip — search falls back to the `vec_distance_cosine` full-scan path.

**Dimension mismatch:** The existing `check_model_mismatch` already errors on model change. Model change implies dimension change, which already forces a full re-index and DB recreation. No new case to handle.

---

## Testing

### New tests
| Test | What it verifies |
|------|-----------------|
| `vec0_migration_upgrades_plain_table` | After migration, `sqlite_master` type is virtual; all rowids present |
| `vec0_migration_is_idempotent` | Running migration twice: no error, data intact |
| `vec0_migration_skips_when_no_dims` | Fresh DB without `embedding_dims` → no-op |
| `search_uses_vec0_after_migration` | Results from vec0 KNN match pre-migration results |
| `vec0_search_respects_limit` | KNN returns exactly `limit` results |
| `vec0_search_scoped_filters_by_source` | Source filter works on vec0 path |

### Helper
`open_test_db_vec0(dims: usize)` — sets `embedding_dims` in meta before returning, so vec0 tests exercise the migrated path without touching `open_test_db()`.

### Existing tests that must keep passing
`cosine_search_returns_closest_vector`, `cosine_search_respects_limit`, `search_returns_source`, `search_scoped_filters_by_source`, `delete_file_chunks_removes_chunks_and_hash`, `read_file_embeddings_returns_content_and_vectors` — these use `open_test_db()` (no `embedding_dims`), so they run on the plain table path. No changes needed.

---

## Files Changed

- `src/embed/index.rs` — `open_db`, `maybe_migrate_to_vec0`, `build_index`, `build_library_index`, `search_scoped`, new tests, `open_test_db_vec0`
