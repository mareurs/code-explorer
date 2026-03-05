//! sqlite-vec based embedding index with incremental updates.
//!
//! Inspired by cocoindex-code's SQLite + sqlite-vec approach:
//! zero external services, embedded in the project directory.
//!
//! Schema:
//!   files(path TEXT, hash TEXT, mtime INTEGER) — tracks indexed file hashes + mtime
//!   chunks(id, file_path, language, content,   — code chunks
//!          start_line, end_line, file_hash,
//!          source)
//!   chunk_embeddings(rowid, embedding)         — blob table (sqlite-vec for search)
//!   meta(key TEXT, value TEXT)                  — stores embed_model, last_indexed_commit
//!
//! Change detection fallback chain:
//!   1. git diff last_indexed_commit..HEAD (tracked files)
//!   2. mtime comparison (untracked files or git unavailable)
//!   3. SHA-256 hash (final arbiter)
//!

use anyhow::Result;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Once;

use super::schema::{CodeChunk, SearchResult};

/// Path to the embedding database within a project.
pub fn db_path(project_root: &Path) -> PathBuf {
    project_root.join(".codescout").join("embeddings.db")
}

/// Open (or create) the embedding database and apply the schema.
/// Register sqlite-vec globally so every SQLite connection in this process
/// gets `vec_distance_cosine`, `vec_f32`, and the `vec0` virtual table module.
/// Uses `sqlite3_auto_extension` so the init runs on every `Connection::open`.
/// Safe to call multiple times — the `Once` guard makes it idempotent.
pub(crate) fn init_sqlite_vec() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: sqlite3_vec_init is a valid SQLite extension entry point.
        // sqlite3_auto_extension expects the full extension init signature:
        // fn(db, pzErrMsg, pApi) -> i32, but sqlite3_vec_init is declared as
        // extern "C" fn() in the crate. The transmute is safe because SQLite
        // will call it with the correct arguments at the C level.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *const i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

pub fn open_db(project_root: &Path) -> Result<Connection> {
    let path = db_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    init_sqlite_vec();
    let conn = Connection::open(&path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;

    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;

        CREATE TABLE IF NOT EXISTS files (
            path  TEXT PRIMARY KEY,
            hash  TEXT NOT NULL,
            mtime INTEGER
        );

        CREATE TABLE IF NOT EXISTS chunks (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path  TEXT NOT NULL,
            language   TEXT NOT NULL,
            content    TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line   INTEGER NOT NULL,
            file_hash  TEXT NOT NULL,
            source     TEXT NOT NULL DEFAULT 'project'
        );

        CREATE TABLE IF NOT EXISTS chunk_embeddings (
            rowid     INTEGER PRIMARY KEY,
            embedding BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS drift_report (
            file_path       TEXT PRIMARY KEY,
            avg_drift       REAL NOT NULL,
            max_drift       REAL NOT NULL,
            max_drift_chunk TEXT,
            chunks_added    INTEGER NOT NULL,
            chunks_removed  INTEGER NOT NULL,
            indexed_at      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS memories (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            bucket     TEXT NOT NULL DEFAULT 'unstructured',
            title      TEXT NOT NULL,
            content    TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_memories_bucket ON memories(bucket);
        ",
    )?;

    // Migrate: add mtime column if missing (safe no-op if already present)
    let has_mtime: bool = conn.prepare("SELECT mtime FROM files LIMIT 0").is_ok();
    if !has_mtime {
        conn.execute_batch("ALTER TABLE files ADD COLUMN mtime INTEGER")?;
    }

    // Migrate: add source column to chunks if missing (safe no-op if already present)
    let has_source: bool = conn.prepare("SELECT source FROM chunks LIMIT 0").is_ok();
    if !has_source {
        conn.execute_batch("ALTER TABLE chunks ADD COLUMN source TEXT NOT NULL DEFAULT 'project'")?;
    }

    maybe_migrate_to_vec0(&conn)?;

    Ok(conn)
}

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

    // sqlite-vec registers vec0 virtual tables as type='table' in sqlite_master
    // (same as plain tables). The sql column contains "USING vec0" which is how
    // we distinguish them. This is the correct detection idiom for this extension.
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

    // Use BEGIN IMMEDIATE to serialise concurrent migration attempts.  Only
    // one connection can hold the reserved (write) lock at a time, so the
    // second caller blocks here until the first commits.  Without IMMEDIATE
    // the check-then-migrate sequence is a classic TOCTOU race: two
    // connections can both observe "plain table" outside any transaction,
    // then the loser corrupts the database by renaming the already-migrated
    // vec0 virtual table (ALTER TABLE succeeds) and then failing on CREATE
    // VIRTUAL TABLE because the shadow tables (e.g. chunk_embeddings_info)
    // still exist under their original names.
    conn.execute_batch("BEGIN IMMEDIATE")?;

    // Re-check inside the exclusive transaction: another connection may have
    // migrated between our initial fast-path check above and acquiring the
    // write lock here.
    let sql_after_lock: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunk_embeddings'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match sql_after_lock {
        None => {
            conn.execute_batch("ROLLBACK")?;
            return Ok(());
        }
        Some(s) if s.contains("USING vec0") => {
            conn.execute_batch("ROLLBACK")?;
            return Ok(()); // another connection migrated first
        }
        _ => {} // still plain — proceed with migration
    }

    conn.execute_batch("ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1")?;
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[{dims}] distance_metric=cosine)"
    ))?;
    conn.execute_batch(
        "INSERT INTO chunk_embeddings(rowid, embedding) \
         SELECT rowid, embedding FROM chunk_embeddings_v1",
    )?;
    conn.execute_batch("DROP TABLE chunk_embeddings_v1")?;
    conn.execute_batch("COMMIT")?;

    tracing::info!("vec0 migration complete");
    Ok(())
}

/// Lazily create the `vec_memories` vec0 virtual table for semantic memory
/// search. Requires `embedding_dims` to be set in the `meta` table (which
/// happens after the first `build_index` run). Safe to call multiple times
/// (idempotent) — returns `Ok(())` if the table already exists. Returns a
/// `RecoverableError` if `embedding_dims` is not yet set.
pub fn ensure_vec_memories(conn: &Connection) -> Result<()> {
    use rusqlite::OptionalExtension;

    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='vec_memories'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if exists {
        return Ok(());
    }

    let dims = match get_meta(conn, "embedding_dims")? {
        Some(s) => s.parse::<usize>().unwrap_or(0),
        None => {
            return Err(crate::tools::RecoverableError::with_hint(
                "semantic index not built yet — cannot store memory embeddings",
                "Run index_project first, then try remember again.",
            )
            .into());
        }
    };
    if dims == 0 {
        return Ok(());
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE vec_memories USING vec0(\
         embedding float[{dims}] distance_metric=cosine)"
    ))?;
    Ok(())
}

/// A memory record returned by [`search_memories`].
pub struct MemoryResult {
    pub id: i64,
    pub bucket: String,
    pub title: String,
    pub content: String,
    pub similarity: f32,
    pub created_at: String,
}

/// Insert a new memory and its embedding into both `memories` and `vec_memories`.
///
/// Returns the row id of the newly inserted memory.
pub fn insert_memory(
    conn: &Connection,
    bucket: &str,
    title: &str,
    content: &str,
    embedding: &[f32],
) -> Result<i64> {
    let now = utc_now_display();
    conn.execute(
        "INSERT INTO memories (bucket, title, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![bucket, title, content, now, now],
    )?;
    let row_id = conn.last_insert_rowid();

    // Serialize embedding as little-endian f32 bytes (sqlite-vec format)
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO vec_memories (rowid, embedding) VALUES (?1, ?2)",
        params![row_id, blob],
    )?;

    Ok(row_id)
}

/// Search memories by embedding similarity, optionally filtered by bucket.
///
/// Returns up to `limit` results sorted by descending similarity (1.0 = identical).
pub fn search_memories(
    conn: &Connection,
    query_embedding: &[f32],
    bucket_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryResult>> {
    let query_blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryResult> {
        let distance: f64 = row.get(5)?;
        let similarity = (1.0_f32 - distance as f32).clamp(0.0, 1.0);
        Ok(MemoryResult {
            id: row.get(0)?,
            bucket: row.get(1)?,
            title: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
            similarity,
        })
    };

    match bucket_filter {
        None => {
            let knn = "SELECT rowid, distance FROM vec_memories \
                       WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2";
            let sql = format!(
                "SELECT m.id, m.bucket, m.title, m.content, m.created_at, \
                 COALESCE(knn.distance, 1.0) AS distance \
                 FROM memories m JOIN ({knn}) knn ON m.id = knn.rowid \
                 ORDER BY distance ASC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some(bucket) => {
            // Over-fetch from KNN (limit * 5) so that post-filtering by bucket
            // still has enough candidates to fill the requested limit.
            let inner_limit = (limit * 5) as i64;
            let knn = "SELECT rowid, distance FROM vec_memories \
                       WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2";
            let sql = format!(
                "SELECT m.id, m.bucket, m.title, m.content, m.created_at, \
                 COALESCE(knn.distance, 1.0) AS distance \
                 FROM memories m JOIN ({knn}) knn ON m.id = knn.rowid \
                 WHERE m.bucket = ?3 \
                 ORDER BY distance ASC \
                 LIMIT ?4"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    params![query_blob, inner_limit, bucket, limit as i64],
                    map_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
    }
}

/// Delete a memory from both `vec_memories` and `memories` by id.
pub fn delete_memory(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
    if conn.changes() == 0 {
        return Err(crate::tools::RecoverableError::with_hint(
            format!("memory with id {} not found", id),
            "Use recall to find memory IDs before deleting.",
        )
        .into());
    }
    // Then delete from vec_memories (best-effort, may not have embedding)
    let _ = conn.execute("DELETE FROM vec_memories WHERE rowid = ?1", params![id]);
    Ok(())
}

/// Insert or update a memory by title match within the same bucket.
///
/// If a memory with the same `title` already exists, its content, bucket, embedding,
/// and `updated_at` timestamp are updated. Otherwise a new memory is inserted.
/// Returns the row id.
pub fn upsert_memory_by_title(
    conn: &Connection,
    bucket: &str,
    title: &str,
    content: &str,
    embedding: &[f32],
) -> Result<i64> {
    use rusqlite::OptionalExtension;

    let existing_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM memories WHERE title = ?1",
            params![title],
            |r| r.get(0),
        )
        .optional()?;

    match existing_id {
        Some(id) => {
            let now = utc_now_display();
            conn.execute(
                "UPDATE memories SET content = ?1, bucket = ?2, updated_at = ?3 WHERE id = ?4",
                params![content, bucket, now, id],
            )?;
            let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
            conn.execute(
                "UPDATE vec_memories SET embedding = ?1 WHERE rowid = ?2",
                params![blob, id],
            )?;
            Ok(id)
        }
        None => insert_memory(conn, bucket, title, content, embedding),
    }
}

/// Hash the content of a file for change detection.
pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    Ok(hex::encode(digest))
}

/// Get file modification time as Unix epoch seconds.
pub fn file_mtime(path: &Path) -> Result<i64> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?;
    let duration = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(duration.as_secs() as i64)
}

/// Insert a chunk and its embedding into the database.
pub fn insert_chunk(conn: &Connection, chunk: &CodeChunk, embedding: &[f32]) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (file_path, language, content, start_line, end_line, file_hash, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            chunk.file_path,
            chunk.language,
            chunk.content,
            chunk.start_line,
            chunk.end_line,
            chunk.file_hash,
            chunk.source,
        ],
    )?;
    let row_id = conn.last_insert_rowid();

    // Serialize embedding as little-endian f32 bytes (sqlite-vec format)
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO chunk_embeddings (rowid, embedding) VALUES (?1, ?2)",
        params![row_id, blob],
    )?;

    Ok(row_id)
}

/// Remove all chunks for a given file path.
pub fn delete_file_chunks(conn: &Connection, file_path: &str) -> Result<()> {
    // Delete embeddings for this file's chunks
    conn.execute(
        "DELETE FROM chunk_embeddings
         WHERE rowid IN (SELECT id FROM chunks WHERE file_path = ?1)",
        params![file_path],
    )?;
    conn.execute(
        "DELETE FROM chunks WHERE file_path = ?1",
        params![file_path],
    )?;
    conn.execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
    Ok(())
}

/// Remove index entries for files that no longer exist on disk.
/// Returns the number of purged files.
pub fn purge_missing_files(conn: &Connection, project_root: &Path) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT path FROM files")?;
    let paths: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut purged = 0;
    for path in &paths {
        let full = project_root.join(path);
        if !full.exists() {
            delete_file_chunks(conn, path)?;
            purged += 1;
        }
    }
    Ok(purged)
}

/// Get the stored hash for a file (for incremental indexing).
pub fn get_file_hash(conn: &Connection, file_path: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT hash FROM files WHERE path = ?1")?;
    let mut rows = stmt.query(params![file_path])?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

/// Update or insert the file hash record.
pub fn upsert_file_hash(
    conn: &Connection,
    file_path: &str,
    hash: &str,
    mtime: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO files (path, hash, mtime) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET hash = excluded.hash, mtime = excluded.mtime",
        params![file_path, hash, mtime],
    )?;
    Ok(())
}

pub fn get_file_mtime(conn: &Connection, file_path: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT mtime FROM files WHERE path = ?1")?;
    let mut rows = stmt.query(params![file_path])?;
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
}

/// A chunk's content and embedding vector, read from the DB before deletion.
#[derive(Debug, Clone)]
pub struct OldChunk {
    pub content: String,
    pub embedding: Vec<f32>,
}

/// Read all chunk content + embedding vectors for a file.
/// Used to snapshot old state before `delete_file_chunks`.
pub fn read_file_embeddings(conn: &Connection, file_path: &str) -> Result<Vec<OldChunk>> {
    let mut stmt = conn.prepare(
        "SELECT c.content, ce.embedding
         FROM chunks c JOIN chunk_embeddings ce ON c.id = ce.rowid
         WHERE c.file_path = ?1
         ORDER BY c.start_line",
    )?;
    let chunks = stmt
        .query_map(params![file_path], |row| {
            let content: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok(OldChunk {
                content,
                embedding: bytes_to_f32(&blob),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(chunks)
}

/// Search for the most similar chunks across all sources.
///
/// Shorthand for [`search_scoped`] with no source filter.
pub fn search(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    search_scoped(conn, query_embedding, limit, None)
}
// Returns true when `chunk_embeddings` is a vec0 virtual table.
// Checked via sqlite_master DDL — O(1) index lookup.
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

/// Scoped cosine similarity search with optional source filtering.
///
/// `source_filter`:
///   - `None` → all sources (no filter)
///   - `Some("project")` → only project chunks
///   - `Some("libraries")` → all non-project chunks
///   - `Some("lib:<name>")` → only chunks from that specific library
pub fn search_scoped(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    source_filter: Option<&str>,
) -> Result<Vec<SearchResult>> {
    if is_vec0_active(conn) {
        return search_scoped_vec0(conn, query_embedding, limit, source_filter);
    }

    // Encode the query as little-endian f32 bytes — the format vec_f32() expects.
    let query_blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<SearchResult> {
        // vec_distance_cosine returns cosine distance ∈ [0, 1] (0 = identical).
        // COALESCE maps NULL (degenerate zero-vector) to 1.0 (maximum distance).
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

    // Common SELECT with sqlite-vec distance. ORDER BY + LIMIT pushed to SQLite.
    let sel = "SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source, \
               COALESCE(vec_distance_cosine(vec_f32(ce.embedding), vec_f32(?1)), 1.0) AS distance \
               FROM chunks c JOIN chunk_embeddings ce ON c.id = ce.rowid";

    match source_filter {
        None => {
            let sql = format!("{sel} ORDER BY distance ASC LIMIT ?2");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some("libraries") => {
            let sql = format!("{sel} WHERE c.source != 'project' ORDER BY distance ASC LIMIT ?2");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some(source) => {
            let sql = format!("{sel} WHERE c.source = ?2 ORDER BY distance ASC LIMIT ?3");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, source, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
    }
}

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
        // COALESCE maps NULL distance (degenerate zero-vector) to 1.0 (maximum
        // distance), matching the behaviour of the full-scan path.
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

    // KNN subquery: bare `distance` column required — vec0's query planner must
    // see it to honour the LIMIT constraint. COALESCE is applied at the outer
    // SELECT level so zero-vector NULLs are mapped to 1.0 (maximum distance).
    let knn = "SELECT rowid, distance FROM chunk_embeddings \
               WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2";

    let sel = format!(
        "SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source, \
         COALESCE(knn.distance, 1.0) AS distance \
         FROM chunks c JOIN ({knn}) knn ON c.id = knn.rowid"
    );

    match source_filter {
        None => {
            let sql = format!("{sel} ORDER BY distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some("libraries") => {
            let sql = format!("{sel} WHERE c.source != 'project' ORDER BY distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
        Some(source) => {
            let sql = format!("{sel} WHERE c.source = ?3 ORDER BY distance ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![query_blob, limit as i64, source], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }
    }
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

/// Result of change detection: which files need re-indexing and which were deleted.
#[derive(Debug)]
pub struct ChangeSet {
    /// Relative paths of files that need re-indexing (new or modified).
    pub changed: Vec<String>,
    /// Relative paths of files that were deleted and purged from the index.
    pub deleted: Vec<String>,
}

/// Detect which files changed since the last index, using the fallback chain:
/// 1. Git diff from last_indexed_commit to HEAD (tracked files)
/// 2. Mtime comparison (untracked or when git diff unavailable)
/// 3. SHA-256 hash as final arbiter
///
/// If `force` is true, returns all indexable files as changed.
pub fn find_changed_files(
    conn: &Connection,
    project_root: &Path,
    force: bool,
) -> Result<ChangeSet> {
    use crate::ast::detect_language;

    let config = crate::config::ProjectConfig::load_or_default(project_root)?;
    let ignored = config.ignored_paths.patterns.clone();

    // Walk all eligible files
    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(move |entry| {
            let name = entry.file_name().to_string_lossy();
            !ignored.iter().any(|p| p.as_str() == name.as_ref())
        })
        .build();

    let mut all_files: Vec<String> = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if detect_language(path).is_none() {
            continue;
        }
        let rel = path
            .strip_prefix(project_root)?
            .to_string_lossy()
            .replace('\\', "/");
        all_files.push(rel);
    }

    if force {
        return Ok(ChangeSet {
            changed: all_files,
            deleted: Vec::new(),
        });
    }

    // Try git-diff approach first
    let git_changed = try_git_diff_detection(conn, project_root);

    let mut changed = Vec::new();
    let mut deleted = Vec::new();

    if let Some(git_result) = git_changed {
        // Git told us which tracked files changed
        let git_changed_set: std::collections::HashSet<&str> =
            git_result.changed.iter().map(|s| s.as_str()).collect();
        let git_deleted_set: std::collections::HashSet<&str> =
            git_result.deleted.iter().map(|s| s.as_str()).collect();

        // Purge deleted files
        for path in &git_result.deleted {
            delete_file_chunks(conn, path)?;
            deleted.push(path.clone());
        }

        for rel in &all_files {
            if git_changed_set.contains(rel.as_str()) {
                // Git says changed -> trust it
                changed.push(rel.clone());
            } else if git_deleted_set.contains(rel.as_str()) {
                continue;
            } else {
                // Not in git diff -> check if it's untracked/new via mtime
                if is_file_changed_mtime_hash(conn, project_root, rel)? {
                    changed.push(rel.clone());
                }
            }
        }
    } else {
        // No git diff available -> fall back to mtime + hash for everything
        for rel in &all_files {
            if is_file_changed_mtime_hash(conn, project_root, rel)? {
                changed.push(rel.clone());
            }
        }
    }

    // Purge files in DB but not on disk (deleted untracked files)
    let purged = purge_missing_files(conn, project_root)?;
    if purged > 0 {
        tracing::debug!("Purged {} missing files from index", purged);
    }

    Ok(ChangeSet { changed, deleted })
}

struct GitDiffResult {
    changed: Vec<String>,
    deleted: Vec<String>,
}

/// Try to use git diff for change detection. Returns None if unavailable.
fn try_git_diff_detection(conn: &Connection, project_root: &Path) -> Option<GitDiffResult> {
    let last_commit = get_last_indexed_commit(conn).ok()??;
    let repo = crate::git::open_repo(project_root).ok()?;
    let head = repo.head().ok()?.peel_to_commit().ok()?;
    let head_sha = head.id().to_string();

    if last_commit == head_sha {
        return Some(GitDiffResult {
            changed: Vec::new(),
            deleted: Vec::new(),
        });
    }

    let entries = crate::git::diff_tree_to_tree(&repo, &last_commit, &head_sha).ok()?;

    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    for entry in entries {
        match entry.status {
            crate::git::DiffStatus::Added | crate::git::DiffStatus::Modified => {
                changed.push(entry.path);
            }
            crate::git::DiffStatus::Deleted => {
                deleted.push(entry.path);
            }
            crate::git::DiffStatus::Renamed { ref old_path } => {
                deleted.push(old_path.clone());
                changed.push(entry.path);
            }
        }
    }
    Some(GitDiffResult { changed, deleted })
}

fn is_file_changed_mtime_hash(conn: &Connection, project_root: &Path, rel: &str) -> Result<bool> {
    let full_path = project_root.join(rel);
    let current_mtime = file_mtime(&full_path)?;
    let stored_mtime = get_file_mtime(conn, rel)?;

    // Fast path: if mtime matches stored value, assume unchanged (cheap check).
    // Mtime is the pre-filter; SHA-256 is only used when mtime differs.
    if Some(current_mtime) == stored_mtime {
        return Ok(false);
    }

    // Mtime differs or no stored mtime → hash to confirm actual content change
    let current_hash = hash_file(&full_path)?;
    let stored_hash = get_file_hash(conn, rel)?;

    Ok(stored_hash.as_deref() != Some(current_hash.as_str()))
}

/// Build or incrementally update the embedding index for a project.
///
/// Three-phase pipeline for maximum throughput:
///   1. Change detection + chunk  (git diff → mtime → hash fallback)
///   2. Embed concurrently   (up to 4 in-flight HTTP requests at once)
///   3. DB writes in a single transaction  (eliminates per-chunk commit overhead)
pub async fn build_index(project_root: &Path, force: bool) -> Result<IndexReport> {
    use crate::config::ProjectConfig;
    use crate::embed::{create_embedder, Embedding};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let config = ProjectConfig::load_or_default(project_root)?;
    let conn = open_db(project_root)?;
    if !force {
        check_model_mismatch(&conn, &config.embeddings.model)?;
    }
    let embedder: Arc<dyn crate::embed::Embedder> =
        Arc::from(create_embedder(&config.embeddings.model).await?);

    // ── Phase 1: Detect changed files ─────────────────────────────────────────
    let change_set = find_changed_files(&conn, project_root, force)?;

    struct FileWork {
        rel: String,
        hash: String,
        mtime: i64,
        lang: String,
        chunks: Vec<super::chunker::RawChunk>,
    }

    let mut works: Vec<FileWork> = Vec::new();

    for rel in &change_set.changed {
        let path = project_root.join(rel);
        let Some(lang) = crate::ast::detect_language(&path) else {
            continue;
        };
        let hash = hash_file(&path)?;

        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let chunks = super::ast_chunker::split_file(
            &source,
            lang,
            &path,
            super::chunk_size_for_model(&config.embeddings.model),
        );
        if chunks.is_empty() {
            continue;
        }

        works.push(FileWork {
            rel: rel.clone(),
            hash,
            mtime: file_mtime(&path).unwrap_or(0),
            lang: lang.to_string(),
            chunks,
        });
    }

    // ── Phase 2: Concurrent embedding ─────────────────────────────────────────
    struct FileResult {
        rel: String,
        hash: String,
        mtime: i64,
        lang: String,
        chunks: Vec<super::chunker::RawChunk>,
        embeddings: Vec<Embedding>,
    }

    // Limit concurrent in-flight requests so we don't overwhelm Ollama
    const MAX_CONCURRENT: usize = 4;
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let mut tasks: JoinSet<Result<FileResult>> = JoinSet::new();

    for work in works {
        let embedder = Arc::clone(&embedder);
        let sem = Arc::clone(&sem);
        tasks.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let texts: Vec<&str> = work.chunks.iter().map(|c| c.content.as_str()).collect();
            let embeddings = embedder.embed(&texts).await?;
            Ok(FileResult {
                rel: work.rel,
                hash: work.hash,
                mtime: work.mtime,
                lang: work.lang,
                chunks: work.chunks,
                embeddings,
            })
        });
    }

    let mut results: Vec<FileResult> = Vec::new();
    while let Some(res) = tasks.join_next().await {
        results.push(res.map_err(|e| anyhow::anyhow!(e))??);
    }

    // ── Phase 3: Single transaction for all DB writes ─────────────────────────
    let indexed = results.len();
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
    // Always clear drift data so stale rows don't persist when the feature is toggled off
    clear_drift_report(&conn)?;
    let mut drift_results: Vec<crate::embed::drift::FileDrift> = Vec::new();
    for result in results {
        // Only snapshot old embeddings when drift detection is enabled
        let old_chunks = if config.embeddings.drift_detection_enabled {
            read_file_embeddings(&conn, &result.rel)?
        } else {
            Vec::new()
        };
        delete_file_chunks(&conn, &result.rel)?;
        for (raw, emb) in result.chunks.iter().zip(result.embeddings.iter()) {
            let chunk = CodeChunk {
                id: None,
                file_path: result.rel.clone(),
                language: result.lang.clone(),
                content: raw.content.clone(),
                start_line: raw.start_line,
                end_line: raw.end_line,
                file_hash: result.hash.clone(),
                source: "project".into(),
            };
            insert_chunk(&conn, &chunk, emb)?;
        }
        upsert_file_hash(&conn, &result.rel, &result.hash, Some(result.mtime))?;

        // Compute drift if enabled and we had old chunks (skip for newly indexed files)
        if config.embeddings.drift_detection_enabled && !old_chunks.is_empty() {
            let new_chunks: Vec<crate::embed::drift::NewChunk> = result
                .chunks
                .iter()
                .zip(result.embeddings.iter())
                .map(|(raw, emb)| crate::embed::drift::NewChunk {
                    content: raw.content.clone(),
                    embedding: emb.clone(),
                })
                .collect();
            let drift = crate::embed::drift::compute_file_drift(
                &conn,
                &result.rel,
                &old_chunks,
                &new_chunks,
            )?;
            upsert_drift_report(
                &conn,
                &drift.file_path,
                drift.avg_drift,
                drift.max_drift,
                drift.max_drift_chunk.as_deref(),
                drift.chunks_added,
                drift.chunks_removed,
            )?;
            drift_results.push(drift);
        }

        tracing::debug!("indexed {} ({} chunks)", result.rel, result.chunks.len());
    }
    set_meta(&conn, "embed_model", &config.embeddings.model)?;
    set_meta(&conn, "last_indexed_at", &utc_now_display())?;

    // Update last indexed commit
    if let Ok(repo) = crate::git::open_repo(project_root) {
        if let Ok(head) = repo.head() {
            if let Ok(commit) = head.peel_to_commit() {
                set_last_indexed_commit(&conn, &commit.id().to_string())?;
            }
        }
    }

    conn.execute_batch("COMMIT")?;
    tracing::info!(
        "Index complete: {} files indexed, {} deleted",
        indexed,
        change_set.deleted.len()
    );
    Ok(IndexReport {
        indexed,
        deleted: change_set.deleted.len(),
        skipped_msg: if force {
            "force rebuild".to_string()
        } else {
            format!("{} deleted", change_set.deleted.len())
        },
        drift: drift_results,
    })
}

/// Build or incrementally update the embedding index for a library.
///
/// Similar to `build_index` but walks `library_path` instead of the project root,
/// and tags all chunks with the given `source` string (e.g. "lib:serde").
/// The DB is stored under `project_root/.codescout/embeddings.db` (shared with project).
pub async fn build_library_index(
    project_root: &Path,
    library_path: &Path,
    source: &str,
    force: bool,
) -> Result<()> {
    use crate::ast::detect_language;
    use crate::config::ProjectConfig;
    use crate::embed::{create_embedder, Embedding};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let config = ProjectConfig::load_or_default(project_root)?;
    let conn = open_db(project_root)?;
    if !force {
        check_model_mismatch(&conn, &config.embeddings.model)?;
    }
    let embedder: Arc<dyn crate::embed::Embedder> =
        Arc::from(create_embedder(&config.embeddings.model).await?);

    // ── Phase 1: Walk library path, hash, chunk ───────────────────────────────
    struct FileWork {
        rel: String,
        hash: String,
        lang: String,
        chunks: Vec<super::chunker::RawChunk>,
    }

    let walker = ignore::WalkBuilder::new(library_path)
        .hidden(true)
        .git_ignore(true)
        .build();

    let mut works: Vec<FileWork> = Vec::new();
    let mut skipped = 0usize;

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(lang) = detect_language(path) else {
            continue;
        };

        // Use library-relative paths prefixed with source for uniqueness
        let rel = path
            .strip_prefix(library_path)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = format!("[{}]/{}", source, rel);
        let hash = hash_file(path)?;

        if !force {
            if let Some(stored) = get_file_hash(&conn, &rel)? {
                if stored == hash {
                    skipped += 1;
                    continue;
                }
            }
        }

        let file_source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let chunks = super::ast_chunker::split_file(
            &file_source,
            lang,
            path,
            super::chunk_size_for_model(&config.embeddings.model),
        );
        if chunks.is_empty() {
            continue;
        }

        works.push(FileWork {
            rel,
            hash,
            lang: lang.to_string(),
            chunks,
        });
    }

    // ── Phase 2: Concurrent embedding ─────────────────────────────────────────
    struct FileResult {
        rel: String,
        hash: String,
        lang: String,
        chunks: Vec<super::chunker::RawChunk>,
        embeddings: Vec<Embedding>,
    }

    const MAX_CONCURRENT: usize = 4;
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let mut tasks: JoinSet<Result<FileResult>> = JoinSet::new();

    for work in works {
        let embedder = Arc::clone(&embedder);
        let sem = Arc::clone(&sem);
        tasks.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let texts: Vec<&str> = work.chunks.iter().map(|c| c.content.as_str()).collect();
            let embeddings = embedder.embed(&texts).await?;
            Ok(FileResult {
                rel: work.rel,
                hash: work.hash,
                lang: work.lang,
                chunks: work.chunks,
                embeddings,
            })
        });
    }

    let mut results: Vec<FileResult> = Vec::new();
    while let Some(res) = tasks.join_next().await {
        results.push(res.map_err(|e| anyhow::anyhow!(e))??);
    }

    // ── Phase 3: Single transaction for all DB writes ─────────────────────────
    let indexed = results.len();
    let source_owned = source.to_string();
    conn.execute_batch("BEGIN")?;
    if let Some(dims) = results
        .first()
        .and_then(|r| r.embeddings.first())
        .map(|e| e.len())
    {
        set_meta(&conn, "embedding_dims", &dims.to_string())?;
    }
    for result in results {
        delete_file_chunks(&conn, &result.rel)?;
        for (raw, emb) in result.chunks.iter().zip(result.embeddings.iter()) {
            let chunk = CodeChunk {
                id: None,
                file_path: result.rel.clone(),
                language: result.lang.clone(),
                content: raw.content.clone(),
                start_line: raw.start_line,
                end_line: raw.end_line,
                file_hash: result.hash.clone(),
                source: source_owned.clone(),
            };
            insert_chunk(&conn, &chunk, emb)?;
        }
        upsert_file_hash(&conn, &result.rel, &result.hash, None)?;
        tracing::debug!("indexed {} ({} chunks)", result.rel, result.chunks.len());
    }
    set_meta(&conn, "embed_model", &config.embeddings.model)?;
    set_meta(&conn, "last_indexed_at", &utc_now_display())?;
    conn.execute_batch("COMMIT")?;
    tracing::info!(
        "Library index complete: {} files indexed, {} unchanged (source={})",
        indexed,
        skipped,
        source
    );
    Ok(())
}

#[derive(Debug)]
pub struct IndexReport {
    pub indexed: usize,
    pub deleted: usize,
    pub skipped_msg: String,
    pub drift: Vec<crate::embed::drift::FileDrift>,
}

/// Statistics about the embedding index.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStats {
    pub file_count: usize,
    pub chunk_count: usize,
    pub embedding_count: usize,
    /// Model string stored at index time, if any.
    pub model: Option<String>,
    /// Human-readable UTC timestamp of the last completed index run (e.g. "2026-03-01 14:22 UTC"), if any.
    pub indexed_at: Option<String>,
}

/// Query index statistics from the database.
pub fn index_stats(conn: &Connection) -> Result<IndexStats> {
    let file_count: usize = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunk_count: usize = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let embedding_count: usize =
        conn.query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))?;
    let model = get_meta(conn, "embed_model")?;
    let indexed_at = get_meta(conn, "last_indexed_at")?;
    Ok(IndexStats {
        file_count,
        chunk_count,
        embedding_count,
        model,
        indexed_at,
    })
}

/// Per-source statistics for the embedding index.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStats {
    pub file_count: usize,
    pub chunk_count: usize,
}

/// Query index statistics grouped by source (e.g. "project", "lib:serde").
pub fn index_stats_by_source(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, SourceStats>> {
    let mut stmt = conn.prepare(
        "SELECT source, COUNT(DISTINCT file_path), COUNT(*) FROM chunks GROUP BY source",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, usize>(1)?,
            row.get::<_, usize>(2)?,
        ))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (source, file_count, chunk_count) = row?;
        map.insert(
            source,
            SourceStats {
                file_count,
                chunk_count,
            },
        );
    }
    Ok(map)
}

/// Read a value from the `meta` key-value table.
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Return an error if the index was built with a different embedding model.
///
/// Call this at the start of `build_index` before processing any files.
/// Returns `Ok(())` when:
///   - no model has been stored yet (first run), OR
///   - the stored model matches `configured`
pub fn check_model_mismatch(conn: &Connection, configured: &str) -> Result<()> {
    match get_meta(conn, "embed_model")? {
        None => Ok(()), // first run
        Some(stored) if stored == configured => Ok(()),
        Some(stored) => anyhow::bail!(
            "Index was built with model '{stored}'.\n\
             Configured model is '{configured}'.\n\
             Delete .codescout/embeddings.db and re-run `index` to rebuild."
        ),
    }
}

/// Write (insert or replace) a value in the `meta` key-value table.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// Returns the current UTC time as a human-readable string like "2026-03-01 14:22 UTC".
/// Uses only `std::time` to avoid pulling in a chrono dependency.
fn utc_now_display() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mins = (secs / 60) % 60;
    let hours = (secs / 3600) % 24;
    let days_since_epoch = secs / 86400;
    let (year, month, day) = days_to_ymd(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{mins:02} UTC")
}

/// Convert days since the Unix epoch (1970-01-01) to (year, month, day).
/// Algorithm: http://howardhinnant.github.io/date_algorithms.html (civil_from_days)
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}

/// Get the SHA of the last commit that was fully indexed.
pub fn get_last_indexed_commit(conn: &Connection) -> Result<Option<String>> {
    get_meta(conn, "last_indexed_commit")
}

/// Record the SHA of the last commit that was fully indexed.
pub fn set_last_indexed_commit(conn: &Connection, sha: &str) -> Result<()> {
    set_meta(conn, "last_indexed_commit", sha)
}

#[derive(Debug, serde::Serialize)]
pub struct Staleness {
    pub stale: bool,
    pub behind_commits: usize,
}

/// Check if the index is behind HEAD.
/// Returns Ok with stale=false if up to date, stale=true with commit count if behind.
/// If no git repo or HEAD doesn't exist, returns stale=true with behind_commits=0.
pub fn check_index_staleness(conn: &Connection, project_root: &Path) -> Result<Staleness> {
    let repo = match crate::git::open_repo(project_root) {
        Ok(r) => r,
        Err(_) => {
            return Ok(Staleness {
                stale: true,
                behind_commits: 0,
            })
        }
    };
    let head_oid = match repo.head() {
        Ok(h) => match h.peel_to_commit() {
            Ok(c) => c.id().to_string(),
            Err(_) => {
                return Ok(Staleness {
                    stale: true,
                    behind_commits: 0,
                })
            }
        },
        Err(_) => {
            return Ok(Staleness {
                stale: true,
                behind_commits: 0,
            })
        }
    };

    let last_indexed = get_last_indexed_commit(conn)?;
    match last_indexed {
        None => Ok(Staleness {
            stale: true,
            behind_commits: 0,
        }),
        Some(ref stored) if stored == &head_oid => Ok(Staleness {
            stale: false,
            behind_commits: 0,
        }),
        Some(ref stored) => {
            let behind = count_commits_between(&repo, stored, &head_oid);
            Ok(Staleness {
                stale: true,
                behind_commits: behind,
            })
        }
    }
}

fn count_commits_between(repo: &git2::Repository, from: &str, to: &str) -> usize {
    let Ok(to_oid) = git2::Oid::from_str(to) else {
        return 0;
    };
    let Ok(from_oid) = git2::Oid::from_str(from) else {
        return 0;
    };
    let Ok(mut revwalk) = repo.revwalk() else {
        return 0;
    };
    if revwalk.push(to_oid).is_err() {
        return 0;
    }
    if revwalk.hide(from_oid).is_err() {
        return 0;
    }
    revwalk.count()
}

/// A row from the drift_report table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DriftReportRow {
    pub file_path: String,
    pub avg_drift: f32,
    pub max_drift: f32,
    pub max_drift_chunk: Option<String>,
    pub chunks_added: usize,
    pub chunks_removed: usize,
    pub indexed_at: String,
}

/// Insert or update a drift report row for a file.
pub fn upsert_drift_report(
    conn: &Connection,
    file_path: &str,
    avg_drift: f32,
    max_drift: f32,
    max_drift_chunk: Option<&str>,
    chunks_added: usize,
    chunks_removed: usize,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    conn.execute(
        "INSERT INTO drift_report (file_path, avg_drift, max_drift, max_drift_chunk, chunks_added, chunks_removed, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(file_path) DO UPDATE SET
             avg_drift = excluded.avg_drift,
             max_drift = excluded.max_drift,
             max_drift_chunk = excluded.max_drift_chunk,
             chunks_added = excluded.chunks_added,
             chunks_removed = excluded.chunks_removed,
             indexed_at = excluded.indexed_at",
        params![file_path, avg_drift, max_drift, max_drift_chunk, chunks_added as i64, chunks_removed as i64, now],
    )?;
    Ok(())
}

/// Query drift report rows, optionally filtering by threshold and path glob.
///
/// - `threshold`: minimum `max_drift` value (default 0.0 when None, which means
///   only rows with max_drift > 0 are returned)
/// - `path_glob`: SQL LIKE pattern for file_path filtering
/// - Results sorted by `max_drift` DESC
pub fn query_drift_report(
    conn: &Connection,
    threshold: Option<f32>,
    path_glob: Option<&str>,
) -> Result<Vec<DriftReportRow>> {
    let threshold = threshold.unwrap_or(0.0);
    let sql = if path_glob.is_some() {
        "SELECT file_path, avg_drift, max_drift, max_drift_chunk, chunks_added, chunks_removed, indexed_at
         FROM drift_report
         WHERE avg_drift > ?1 AND file_path LIKE ?2
         ORDER BY max_drift DESC"
    } else {
        "SELECT file_path, avg_drift, max_drift, max_drift_chunk, chunks_added, chunks_removed, indexed_at
         FROM drift_report
         WHERE avg_drift > ?1
         ORDER BY max_drift DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(glob) = path_glob {
        stmt.query_map(params![threshold, glob], map_drift_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![threshold], map_drift_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

fn map_drift_row(row: &rusqlite::Row) -> rusqlite::Result<DriftReportRow> {
    Ok(DriftReportRow {
        file_path: row.get(0)?,
        avg_drift: row.get(1)?,
        max_drift: row.get(2)?,
        max_drift_chunk: row.get(3)?,
        chunks_added: row.get::<_, i64>(4)? as usize,
        chunks_removed: row.get::<_, i64>(5)? as usize,
        indexed_at: row.get(6)?,
    })
}

/// Delete all rows from the drift_report table.
pub fn clear_drift_report(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM drift_report", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::schema::CodeChunk;
    use rusqlite::OptionalExtension;
    use tempfile::tempdir;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        (dir, conn)
    }

    fn open_test_db_vec0(dims: usize) -> (tempfile::TempDir, Connection) {
        let (dir, conn) = open_test_db();
        set_meta(&conn, "embedding_dims", &dims.to_string()).unwrap();
        maybe_migrate_to_vec0(&conn).unwrap();
        (dir, conn)
    }

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
        assert!(
            !sql.contains("USING vec0"),
            "expected plain table, got: {sql}"
        );
    }

    #[test]
    fn vec0_migration_upgrades_plain_table() {
        let (_dir, conn) = open_test_db();
        // Insert a chunk so there is data to migrate
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[0.1_f32, 0.2_f32],
        )
        .unwrap();
        set_meta(&conn, "embedding_dims", "2").unwrap();

        maybe_migrate_to_vec0(&conn).unwrap();

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='chunk_embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("USING vec0"),
            "expected vec0 virtual table, got: {sql}"
        );

        // Data must survive the migration
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn vec0_migration_is_idempotent() {
        let (_dir, conn) = open_test_db();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[0.1_f32, 0.2_f32],
        )
        .unwrap();
        set_meta(&conn, "embedding_dims", "2").unwrap();

        maybe_migrate_to_vec0(&conn).unwrap();
        // Second call must not error
        maybe_migrate_to_vec0(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn vec0_migration_is_transactional() {
        let (_dir, conn) = open_test_db();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[0.1_f32, 0.2_f32],
        )
        .unwrap();
        set_meta(&conn, "embedding_dims", "2").unwrap();
        // Try wrapping the four migration steps in an explicit transaction
        conn.execute_batch("BEGIN").unwrap();
        conn.execute_batch("ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1")
            .unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[2])")
            .unwrap();
        conn.execute_batch(
            "INSERT INTO chunk_embeddings(rowid, embedding) \
             SELECT rowid, embedding FROM chunk_embeddings_v1",
        )
        .unwrap();
        conn.execute_batch("DROP TABLE chunk_embeddings_v1")
            .unwrap();
        conn.execute_batch("COMMIT").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    fn dummy_chunk(file_path: &str, content: &str) -> CodeChunk {
        CodeChunk {
            id: None,
            file_path: file_path.to_string(),
            language: "rust".to_string(),
            content: content.to_string(),
            start_line: 1,
            end_line: 3,
            file_hash: "testhash".to_string(),
            source: "project".into(),
        }
    }

    fn dummy_chunk_with_source(file_path: &str, content: &str, source: &str) -> CodeChunk {
        CodeChunk {
            id: None,
            file_path: file_path.to_string(),
            language: "rust".to_string(),
            content: content.to_string(),
            start_line: 1,
            end_line: 3,
            file_hash: "testhash".to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn open_db_creates_tables() {
        let (_dir, conn) = open_test_db();
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        let chunks: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0);
        assert_eq!(chunks, 0);
    }

    #[test]
    fn open_db_auto_migrates_when_dims_present() {
        let dir = tempfile::tempdir().unwrap();
        let conn1 = open_db(dir.path()).unwrap();
        // Insert a chunk and store dims (simulates a post-indexing state)
        insert_chunk(
            &conn1,
            &dummy_chunk("x.rs", "fn x() {}"),
            &[1.0_f32, 0.0_f32],
        )
        .unwrap();
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
        assert!(
            sql.contains("USING vec0"),
            "expected vec0 after reopen, got: {sql}"
        );
    }

    /// Verify that vec0 shadow tables ARE properly rolled back on transaction ROLLBACK.
    /// If this test FAILS, it means shadow tables survive ROLLBACK, which is the root cause
    /// of the "chunk_embeddings_info already exists" error on subsequent open_db calls.
    #[test]
    fn vec0_create_virtual_table_rolls_back_shadow_tables_on_rollback() {
        let (_dir, conn) = open_test_db();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[0.1_f32, 0.2_f32],
        )
        .unwrap();
        set_meta(&conn, "embedding_dims", "2").unwrap();

        // Manually replay the migration steps, then ROLLBACK instead of COMMIT
        conn.execute_batch("BEGIN").unwrap();
        conn.execute_batch("ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1")
            .unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[2])")
            .unwrap();
        // Rollback WITHOUT committing — simulates a failed migration
        conn.execute_batch("ROLLBACK").unwrap();

        // After ROLLBACK: chunk_embeddings must be the plain table again
        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunk_embeddings'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(
            sql.as_deref()
                .map(|s| !s.contains("USING vec0"))
                .unwrap_or(false),
            "chunk_embeddings should be plain table after ROLLBACK, got: {sql:?}"
        );

        // The shadow tables must NOT persist after ROLLBACK
        let info_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='chunk_embeddings_info'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            > 0;
        assert!(
            !info_exists,
            "chunk_embeddings_info must not exist after ROLLBACK of CREATE VIRTUAL TABLE"
        );

        // A subsequent migration must succeed (no leftover shadow tables to conflict)
        maybe_migrate_to_vec0(&conn).unwrap();
    }

    /// Regression test: calling open_db on a DB that was already migrated to vec0
    /// must not fail with "table chunk_embeddings_info already exists".
    /// The execute_batch DDL in open_db runs `CREATE TABLE IF NOT EXISTS chunk_embeddings`
    /// every time — it must be a true no-op when chunk_embeddings is already a vec0 VT.
    #[test]
    fn open_db_is_idempotent_after_vec0_migration() {
        let dir = tempfile::tempdir().unwrap();

        // Conn1: plain table setup + dims set (simulates post-index_project state)
        let conn1 = open_db(dir.path()).unwrap();
        insert_chunk(
            &conn1,
            &dummy_chunk("x.rs", "fn x() {}"),
            &[1.0_f32, 0.0_f32],
        )
        .unwrap();
        set_meta(&conn1, "embedding_dims", "2").unwrap();
        drop(conn1);

        // Conn2: open_db triggers migration → chunk_embeddings becomes vec0
        let conn2 = open_db(dir.path()).unwrap();
        drop(conn2);

        // Conn3: open_db on already-migrated DB must NOT error with
        // "Could not create '_info' shadow table: table chunk_embeddings_info already exists"
        let conn3 = open_db(dir.path())
            .expect("third open_db must succeed when chunk_embeddings is already vec0");
        let sql: String = conn3
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='chunk_embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("USING vec0"),
            "expected vec0 to persist across opens, got: {sql}"
        );

        // Print all sqlite_master entries to understand vec0 table structure
        let mut stmt = conn3
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        eprintln!("sqlite_master after 3rd open:");
        for (t, n, s) in &rows {
            eprintln!("  type={t:?} name={n:?} sql={s:?}");
        }
    }

    /// Documents the SQLite behavior that caused the TOCTOU race:
    /// renaming a vec0 virtual table succeeds but does NOT rename its shadow
    /// tables, so a subsequent CREATE VIRTUAL TABLE with the original name
    /// fails because the shadow tables (e.g. chunk_embeddings_info) still
    /// exist.  This test is deterministic and always passes — it serves as a
    /// pinned understanding of the underlying SQLite quirk.
    #[test]
    fn migration_race_loser_exposes_shadow_table_conflict() {
        let dir = tempfile::tempdir().unwrap();

        // Set up: plain table + dims + data (post-index_project state)
        let conn = open_db(dir.path()).unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[0.1_f32, 0.2_f32],
        )
        .unwrap();
        set_meta(&conn, "embedding_dims", "2").unwrap();
        drop(conn);

        // First connection migrates successfully (plain → vec0)
        let conn_a = open_db(dir.path()).unwrap();
        drop(conn_a);

        // Simulate what the losing connection would do WITHOUT BEGIN IMMEDIATE:
        // it observed "plain table" before the lock, then acquired the write
        // lock after conn_a committed.  At that point chunk_embeddings is
        // already a vec0 VT.  ALTERing it succeeds (SQLite allows renaming VTs
        // since 3.26.0) but shadow tables are NOT renamed.
        let conn_b = open_db(dir.path()).unwrap();
        conn_b
            .execute_batch("ALTER TABLE chunk_embeddings RENAME TO chunk_embeddings_v1")
            .expect("ALTER TABLE on vec0 VT should succeed");

        // The shadow table chunk_embeddings_info still exists under its original
        // name → CREATE VIRTUAL TABLE must fail with the shadow-table conflict.
        let result = conn_b.execute_batch(
            "CREATE VIRTUAL TABLE chunk_embeddings USING vec0(embedding float[2] distance_metric=cosine)",
        );
        assert!(
            result.is_err(),
            "expected shadow-table conflict but CREATE succeeded"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("already exists"),
            "expected 'already exists' error, got: {err}"
        );
    }

    /// Regression test: two threads racing through open_db on a migration-ready
    /// database must both succeed.  Before the BEGIN IMMEDIATE fix the second
    /// thread could corrupt the database and return an error.
    #[test]
    fn concurrent_open_db_migrations_do_not_corrupt() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().to_owned();

        // Set up: plain table + dims + data, then close the connection so the
        // DB is in the "ready to migrate" state (no open connections).
        {
            let conn = open_db(&db_path).unwrap();
            insert_chunk(
                &conn,
                &dummy_chunk("a.rs", "fn a() {}"),
                &[0.1_f32, 0.2_f32],
            )
            .unwrap();
            set_meta(&conn, "embedding_dims", "2").unwrap();
        }

        // Barrier ensures both threads call open_db at the same time, maximising
        // the probability of the TOCTOU race manifesting without the fix.
        let barrier = Arc::new(Barrier::new(2));

        let barrier_a = Arc::clone(&barrier);
        let path_a = db_path.clone();
        let handle_a = std::thread::spawn(move || -> anyhow::Result<()> {
            barrier_a.wait();
            open_db(&path_a).map(|_| ())
        });

        let barrier_b = Arc::clone(&barrier);
        let path_b = db_path.clone();
        let handle_b = std::thread::spawn(move || -> anyhow::Result<()> {
            barrier_b.wait();
            open_db(&path_b).map(|_| ())
        });

        let result_a = handle_a.join().expect("thread A panicked");
        let result_b = handle_b.join().expect("thread B panicked");

        assert!(
            result_a.is_ok(),
            "thread A open_db failed: {:?}",
            result_a.err()
        );
        assert!(
            result_b.is_ok(),
            "thread B open_db failed: {:?}",
            result_b.err()
        );

        // Verify the DB is in the correct final state: vec0 virtual table.
        let conn = open_db(&db_path).unwrap();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='chunk_embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("USING vec0"),
            "expected vec0 after concurrent migration, got: {sql}"
        );
    }

    #[test]
    fn insert_chunk_assigns_row_id() {
        let (_dir, conn) = open_test_db();
        let id = insert_chunk(&conn, &dummy_chunk("a.rs", "fn a() {}"), &[0.1, 0.2]).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn insert_multiple_chunks_for_same_file() {
        let (_dir, conn) = open_test_db();
        insert_chunk(&conn, &dummy_chunk("f.rs", "chunk 1"), &[0.1]).unwrap();
        insert_chunk(&conn, &dummy_chunk("f.rs", "chunk 2"), &[0.2]).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE file_path='f.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn file_hash_upsert_and_get() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "src/lib.rs", "aabbcc", None).unwrap();
        assert_eq!(
            get_file_hash(&conn, "src/lib.rs").unwrap(),
            Some("aabbcc".to_string())
        );
    }

    #[test]
    fn file_hash_upsert_updates_on_conflict() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "src/lib.rs", "hash1", None).unwrap();
        upsert_file_hash(&conn, "src/lib.rs", "hash2", None).unwrap();
        assert_eq!(
            get_file_hash(&conn, "src/lib.rs").unwrap(),
            Some("hash2".to_string())
        );
    }

    #[test]
    fn get_file_hash_missing_returns_none() {
        let (_dir, conn) = open_test_db();
        assert_eq!(get_file_hash(&conn, "nonexistent.rs").unwrap(), None);
    }

    #[test]
    fn delete_file_chunks_removes_chunks_and_hash() {
        let (_dir, conn) = open_test_db();
        insert_chunk(&conn, &dummy_chunk("del.rs", "fn x() {}"), &[0.5]).unwrap();
        upsert_file_hash(&conn, "del.rs", "abc", None).unwrap();

        delete_file_chunks(&conn, "del.rs").unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE file_path='del.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(get_file_hash(&conn, "del.rs").unwrap(), None);
    }

    #[test]
    fn delete_does_not_affect_other_files() {
        let (_dir, conn) = open_test_db();
        insert_chunk(&conn, &dummy_chunk("keep.rs", "fn keep() {}"), &[0.1]).unwrap();
        insert_chunk(&conn, &dummy_chunk("del.rs", "fn del() {}"), &[0.2]).unwrap();

        delete_file_chunks(&conn, "del.rs").unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE file_path='keep.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn cosine_search_returns_closest_vector() {
        let (_dir, conn) = open_test_db();
        // Two orthogonal 4-dim embeddings
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[1.0, 0.0, 0.0, 0.0],
        )
        .unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk("b.rs", "fn b() {}"),
            &[0.0, 1.0, 0.0, 0.0],
        )
        .unwrap();

        // Query aligned with a.rs
        let results = search(&conn, &[0.9, 0.1, 0.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "a.rs");
        assert!(results[0].score > 0.9, "score was {}", results[0].score);
    }

    #[test]
    fn cosine_search_respects_limit() {
        let (_dir, conn) = open_test_db();
        for i in 0..5 {
            insert_chunk(
                &conn,
                &dummy_chunk(&format!("{}.rs", i), "fn f() {}"),
                &[i as f32, 0.0],
            )
            .unwrap();
        }
        let results = search(&conn, &[1.0, 0.0], 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn hash_file_produces_64_char_hex() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, b"fn main() {}").unwrap();
        let hash = hash_file(&file).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_file_differs_for_different_content() {
        let dir = tempdir().unwrap();
        let f1 = dir.path().join("a.rs");
        let f2 = dir.path().join("b.rs");
        std::fs::write(&f1, b"fn a() {}").unwrap();
        std::fs::write(&f2, b"fn b() {}").unwrap();
        assert_ne!(hash_file(&f1).unwrap(), hash_file(&f2).unwrap());
    }

    #[test]
    fn open_db_creates_meta_table() {
        let (_dir, conn) = open_test_db();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn meta_get_missing_key_returns_none() {
        let (_dir, conn) = open_test_db();
        let val = get_meta(&conn, "embed_model").unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn meta_set_then_get_roundtrip() {
        let (_dir, conn) = open_test_db();
        set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
        let val = get_meta(&conn, "embed_model").unwrap();
        assert_eq!(val.as_deref(), Some("ollama:mxbai-embed-large"));
    }

    #[test]
    fn meta_set_overwrites_existing_value() {
        let (_dir, conn) = open_test_db();
        set_meta(&conn, "embed_model", "old-model").unwrap();
        set_meta(&conn, "embed_model", "new-model").unwrap();
        let val = get_meta(&conn, "embed_model").unwrap();
        assert_eq!(val.as_deref(), Some("new-model"));
    }

    #[test]
    fn check_model_mismatch_first_run_is_ok() {
        let (_dir, conn) = open_test_db();
        // No meta entry yet — first run should succeed
        assert!(check_model_mismatch(&conn, "ollama:mxbai-embed-large").is_ok());
    }

    #[test]
    fn check_model_mismatch_same_model_is_ok() {
        let (_dir, conn) = open_test_db();
        set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
        assert!(check_model_mismatch(&conn, "ollama:mxbai-embed-large").is_ok());
    }

    #[test]
    fn check_model_mismatch_different_model_is_err() {
        let (_dir, conn) = open_test_db();
        set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
        let err = check_model_mismatch(&conn, "local:JinaEmbeddingsV2BaseCode")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("ollama:mxbai-embed-large"),
            "error should name stored model"
        );
        assert!(
            err.contains("local:JinaEmbeddingsV2BaseCode"),
            "error should name new model"
        );
        assert!(
            err.contains("embeddings.db"),
            "error should hint at DB deletion"
        );
    }

    #[test]
    fn index_stats_returns_stored_model() {
        let (_dir, conn) = open_test_db();
        set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
        let stats = index_stats(&conn).unwrap();
        assert_eq!(stats.model.as_deref(), Some("ollama:mxbai-embed-large"));
    }

    #[test]
    fn index_stats_model_is_none_when_unset() {
        let (_dir, conn) = open_test_db();
        let stats = index_stats(&conn).unwrap();
        assert!(stats.model.is_none());
    }

    #[test]
    fn normalize_rel_path_uses_forward_slashes() {
        use std::path::PathBuf;
        // Simulate what build_index does: strip prefix + to_string_lossy
        let root = PathBuf::from(if cfg!(windows) {
            "C:\\project"
        } else {
            "/project"
        });
        let file = root.join("src").join("tools").join("file.rs");
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(rel, "src/tools/file.rs");
    }

    #[test]
    fn insert_chunk_stores_source() {
        let (_dir, conn) = open_test_db();
        let chunk = dummy_chunk_with_source("lib.rs", "fn x() {}", "lib:serde");
        insert_chunk(&conn, &chunk, &[0.1, 0.2]).unwrap();
        let stored: String = conn
            .query_row(
                "SELECT source FROM chunks WHERE file_path = 'lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "lib:serde");
    }

    #[test]
    fn search_returns_source() {
        let (_dir, conn) = open_test_db();
        insert_chunk(
            &conn,
            &dummy_chunk_with_source("a.rs", "fn a() {}", "lib:tokio"),
            &[1.0, 0.0],
        )
        .unwrap();
        let results = search(&conn, &[1.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "lib:tokio");
    }

    #[test]
    fn search_scoped_filters_by_source() {
        let (_dir, conn) = open_test_db();
        // Insert project chunk and library chunk with orthogonal embeddings
        insert_chunk(
            &conn,
            &dummy_chunk("proj.rs", "fn proj() {}"),
            &[1.0, 0.0, 0.0],
        )
        .unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk_with_source("serde.rs", "fn serde() {}", "lib:serde"),
            &[0.0, 1.0, 0.0],
        )
        .unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk_with_source("tokio.rs", "fn tokio() {}", "lib:tokio"),
            &[0.0, 0.0, 1.0],
        )
        .unwrap();

        // No filter → all 3
        let all = search_scoped(&conn, &[1.0, 1.0, 1.0], 10, None).unwrap();
        assert_eq!(all.len(), 3);

        // Project only
        let proj = search_scoped(&conn, &[1.0, 1.0, 1.0], 10, Some("project")).unwrap();
        assert_eq!(proj.len(), 1);
        assert_eq!(proj[0].source, "project");

        // Libraries (all non-project)
        let libs = search_scoped(&conn, &[1.0, 1.0, 1.0], 10, Some("libraries")).unwrap();
        assert_eq!(libs.len(), 2);
        assert!(libs.iter().all(|r| r.source != "project"));

        // Specific library
        let serde_only = search_scoped(&conn, &[1.0, 1.0, 1.0], 10, Some("lib:serde")).unwrap();
        assert_eq!(serde_only.len(), 1);
        assert_eq!(serde_only[0].source, "lib:serde");
    }

    #[test]
    fn upsert_file_hash_stores_mtime() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "a.rs", "abc123", Some(1700000000)).unwrap();
        let mtime: Option<i64> = conn
            .query_row("SELECT mtime FROM files WHERE path = 'a.rs'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(mtime, Some(1700000000));
    }

    #[test]
    fn upsert_file_hash_updates_mtime() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "a.rs", "abc", Some(1000)).unwrap();
        upsert_file_hash(&conn, "a.rs", "def", Some(2000)).unwrap();
        let mtime: Option<i64> = conn
            .query_row("SELECT mtime FROM files WHERE path = 'a.rs'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(mtime, Some(2000));
    }

    #[test]
    fn get_file_mtime_returns_stored_value() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "a.rs", "abc", Some(1700000000)).unwrap();
        assert_eq!(get_file_mtime(&conn, "a.rs").unwrap(), Some(1700000000));
    }

    #[test]
    fn get_file_mtime_returns_none_for_missing() {
        let (_dir, conn) = open_test_db();
        assert_eq!(get_file_mtime(&conn, "missing.rs").unwrap(), None);
    }

    #[test]
    fn index_stats_by_source_groups() {
        let (_dir, conn) = open_test_db();
        insert_chunk(&conn, &dummy_chunk("a.rs", "fn a() {}"), &[0.1]).unwrap();
        insert_chunk(&conn, &dummy_chunk("b.rs", "fn b() {}"), &[0.2]).unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk_with_source("serde.rs", "fn serde() {}", "lib:serde"),
            &[0.3],
        )
        .unwrap();

        let by_source = index_stats_by_source(&conn).unwrap();
        assert_eq!(by_source.len(), 2);

        let project = by_source.get("project").unwrap();
        assert_eq!(project.file_count, 2);
        assert_eq!(project.chunk_count, 2);

        let serde = by_source.get("lib:serde").unwrap();
        assert_eq!(serde.file_count, 1);
        assert_eq!(serde.chunk_count, 1);
    }

    #[test]
    fn last_indexed_commit_roundtrip() {
        let (_dir, conn) = open_test_db();
        assert_eq!(get_last_indexed_commit(&conn).unwrap(), None);
        set_last_indexed_commit(&conn, "abc123def456").unwrap();
        assert_eq!(
            get_last_indexed_commit(&conn).unwrap(),
            Some("abc123def456".to_string())
        );
    }

    #[test]
    fn last_indexed_commit_updates() {
        let (_dir, conn) = open_test_db();
        set_last_indexed_commit(&conn, "aaa").unwrap();
        set_last_indexed_commit(&conn, "bbb").unwrap();
        assert_eq!(
            get_last_indexed_commit(&conn).unwrap(),
            Some("bbb".to_string())
        );
    }

    #[test]
    fn file_mtime_returns_epoch_seconds() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, b"fn main() {}").unwrap();
        let mtime = file_mtime(&file).unwrap();
        // Should be a reasonable epoch timestamp (after 2020)
        assert!(mtime > 1_577_836_800); // 2020-01-01
        assert!(mtime < 2_000_000_000); // ~2033
    }

    #[test]
    fn purge_missing_files_removes_deleted() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        // Insert entries for two files, but only create one on disk
        let existing = dir.path().join("exists.rs");
        std::fs::write(&existing, "fn a() {}").unwrap();
        insert_chunk(&conn, &dummy_chunk("exists.rs", "fn a() {}"), &[0.5]).unwrap();
        upsert_file_hash(&conn, "exists.rs", "aaa", Some(1000)).unwrap();
        insert_chunk(&conn, &dummy_chunk("gone.rs", "fn b() {}"), &[0.5]).unwrap();
        upsert_file_hash(&conn, "gone.rs", "bbb", Some(1000)).unwrap();

        let purged = purge_missing_files(&conn, dir.path()).unwrap();
        assert_eq!(purged, 1);

        // gone.rs should be removed from files table
        assert_eq!(get_file_hash(&conn, "gone.rs").unwrap(), None);
        // exists.rs should remain
        assert!(get_file_hash(&conn, "exists.rs").unwrap().is_some());
    }

    #[test]
    fn purge_missing_files_returns_zero_when_all_exist() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}").unwrap();
        upsert_file_hash(&conn, "a.rs", "aaa", Some(1000)).unwrap();
        let purged = purge_missing_files(&conn, dir.path()).unwrap();
        assert_eq!(purged, 0);
    }

    #[test]
    fn staleness_no_commit_stored_is_stale() {
        let dir = tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();
        // Create an initial commit
        let mut index = repo.index().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let conn = open_db(dir.path()).unwrap();
        let staleness = check_index_staleness(&conn, dir.path()).unwrap();
        assert!(staleness.stale);
    }

    #[test]
    fn staleness_matching_commit_is_fresh() {
        let dir = tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();
        let mut index = repo.index().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let conn = open_db(dir.path()).unwrap();
        set_last_indexed_commit(&conn, &oid.to_string()).unwrap();
        let staleness = check_index_staleness(&conn, dir.path()).unwrap();
        assert!(!staleness.stale);
        assert_eq!(staleness.behind_commits, 0);
    }

    #[test]
    fn find_changed_files_detects_new_file() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        // Create a file that's not in the index
        let file = dir.path().join("new.rs");
        std::fs::write(&file, "fn new() {}").unwrap();

        let candidates = find_changed_files(&conn, dir.path(), false).unwrap();
        assert_eq!(candidates.changed.len(), 1);
        assert_eq!(candidates.changed[0], "new.rs");
    }

    #[test]
    fn find_changed_files_skips_unchanged_mtime() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        // Create a file and index it with matching mtime + hash
        let file = dir.path().join("same.rs");
        std::fs::write(&file, "fn same() {}").unwrap();
        let mtime = file_mtime(&file).unwrap();
        let hash = hash_file(&file).unwrap();
        upsert_file_hash(&conn, "same.rs", &hash, Some(mtime)).unwrap();

        let candidates = find_changed_files(&conn, dir.path(), false).unwrap();
        assert!(candidates.changed.is_empty());
    }

    #[test]
    fn find_changed_files_mtime_match_skips_hash_check() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        let file = dir.path().join("mod.rs");
        std::fs::write(&file, "fn a() {}").unwrap();
        let mtime = file_mtime(&file).unwrap();
        // Store a different hash but matching mtime — mtime pre-filter should
        // short-circuit and treat the file as unchanged (by design: mtime is
        // the cheap gate, hash is only checked when mtime differs)
        upsert_file_hash(&conn, "mod.rs", "oldhash", Some(mtime)).unwrap();

        let candidates = find_changed_files(&conn, dir.path(), false).unwrap();
        assert!(
            candidates.changed.is_empty(),
            "mtime match should skip hash check"
        );
    }

    #[test]
    fn find_changed_files_detects_mtime_change() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        let file = dir.path().join("mod.rs");
        std::fs::write(&file, "fn a() {}").unwrap();
        let hash = hash_file(&file).unwrap();
        // Store correct hash but stale mtime — mtime differs so hash is checked,
        // hash matches so file should NOT be reported as changed
        upsert_file_hash(&conn, "mod.rs", &hash, Some(0)).unwrap();

        let candidates = find_changed_files(&conn, dir.path(), false).unwrap();
        assert!(
            candidates.changed.is_empty(),
            "hash match should prevent re-index even if mtime differs"
        );
    }

    #[test]
    fn open_db_creates_drift_report_table() {
        let (_dir, conn) = open_test_db();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM drift_report", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn find_changed_files_force_returns_all() {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();

        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}").unwrap();
        let mtime = file_mtime(&file).unwrap();
        let hash = hash_file(&file).unwrap();
        upsert_file_hash(&conn, "a.rs", &hash, Some(mtime)).unwrap();

        let candidates = find_changed_files(&conn, dir.path(), true).unwrap();
        assert_eq!(candidates.changed.len(), 1); // force includes even unchanged
    }

    #[test]
    fn read_file_embeddings_returns_content_and_vectors() {
        let (_dir, conn) = open_test_db();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn hello() {}"),
            &[1.0, 0.0, 0.0],
        )
        .unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn world() {}"),
            &[0.0, 1.0, 0.0],
        )
        .unwrap();

        let old = read_file_embeddings(&conn, "a.rs").unwrap();
        assert_eq!(old.len(), 2);
        assert_eq!(old[0].content, "fn hello() {}");
        assert_eq!(old[0].embedding, vec![1.0, 0.0, 0.0]);
        assert_eq!(old[1].content, "fn world() {}");
        assert_eq!(old[1].embedding, vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn read_file_embeddings_returns_empty_for_missing_file() {
        let (_dir, conn) = open_test_db();
        let old = read_file_embeddings(&conn, "missing.rs").unwrap();
        assert!(old.is_empty());
    }

    #[test]
    fn upsert_drift_report_inserts_and_queries() {
        let (_dir, conn) = open_test_db();
        upsert_drift_report(&conn, "a.rs", 0.25, 0.8, Some("fn changed() {}"), 1, 0).unwrap();

        let reports = query_drift_report(&conn, None, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].file_path, "a.rs");
        assert!((reports[0].avg_drift - 0.25).abs() < 0.01);
        assert!((reports[0].max_drift - 0.8).abs() < 0.01);
        assert_eq!(
            reports[0].max_drift_chunk.as_deref(),
            Some("fn changed() {}")
        );
        assert_eq!(reports[0].chunks_added, 1);
        assert_eq!(reports[0].chunks_removed, 0);
    }

    #[test]
    fn upsert_drift_report_overwrites() {
        let (_dir, conn) = open_test_db();
        upsert_drift_report(&conn, "a.rs", 0.1, 0.2, None, 0, 0).unwrap();
        upsert_drift_report(&conn, "a.rs", 0.9, 0.95, Some("new"), 2, 1).unwrap();
        let reports = query_drift_report(&conn, None, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert!((reports[0].avg_drift - 0.9).abs() < 0.01);
    }

    #[test]
    fn query_drift_report_filters_by_threshold() {
        let (_dir, conn) = open_test_db();
        upsert_drift_report(&conn, "low.rs", 0.05, 0.1, None, 0, 0).unwrap();
        upsert_drift_report(&conn, "high.rs", 0.5, 0.9, None, 1, 0).unwrap();
        let reports = query_drift_report(&conn, Some(0.1), None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].file_path, "high.rs");
    }

    #[test]
    fn query_drift_report_filters_by_path_glob() {
        let (_dir, conn) = open_test_db();
        upsert_drift_report(&conn, "src/tools/a.rs", 0.5, 0.5, None, 0, 0).unwrap();
        upsert_drift_report(&conn, "src/embed/b.rs", 0.5, 0.5, None, 0, 0).unwrap();
        let reports = query_drift_report(&conn, None, Some("src/tools/%")).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].file_path, "src/tools/a.rs");
    }

    #[test]
    fn clear_drift_report_removes_all_rows() {
        let (_dir, conn) = open_test_db();
        upsert_drift_report(&conn, "a.rs", 0.5, 0.5, None, 0, 0).unwrap();
        clear_drift_report(&conn).unwrap();
        let reports = query_drift_report(&conn, None, None).unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn vec0_search_returns_closest_vector() {
        let (_dir, conn) = open_test_db_vec0(4);
        insert_chunk(
            &conn,
            &dummy_chunk("a.rs", "fn a() {}"),
            &[1.0_f32, 0.0, 0.0, 0.0],
        )
        .unwrap();
        insert_chunk(
            &conn,
            &dummy_chunk("b.rs", "fn b() {}"),
            &[0.0_f32, 1.0, 0.0, 0.0],
        )
        .unwrap();

        let results = search(&conn, &[0.9_f32, 0.1, 0.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "a.rs");
        assert!(results[0].score > 0.9, "score={}", results[0].score);
    }

    #[test]
    fn vec0_search_respects_limit() {
        let (_dir, conn) = open_test_db_vec0(2);
        for i in 1..=5u8 {
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

    #[test]
    fn open_db_creates_memories_table() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_db_creates_vec_memories_table() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "384").unwrap();
        ensure_vec_memories(&conn).unwrap();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='vec_memories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("vec0"),
            "expected vec0 virtual table, got: {sql}"
        );
    }

    #[test]
    fn insert_and_search_memory() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        let embedding = vec![0.1_f32, 0.2, 0.3];
        let id = insert_memory(
            &conn,
            "code",
            "test title",
            "test content about patterns",
            &embedding,
        )
        .unwrap();
        assert!(id > 0);

        let results = search_memories(&conn, &embedding, None, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "test title");
        assert_eq!(results[0].bucket, "code");
    }

    #[test]
    fn delete_memory_removes_from_both_tables() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        let embedding = vec![0.1_f32, 0.2, 0.3];
        let id = insert_memory(&conn, "code", "to delete", "content", &embedding).unwrap();
        delete_memory(&conn, id).unwrap();

        let results = search_memories(&conn, &embedding, None, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_memories_filters_by_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        let e1 = vec![0.1_f32, 0.2, 0.3];
        let e2 = vec![0.3_f32, 0.2, 0.1];
        insert_memory(&conn, "code", "code mem", "patterns", &e1).unwrap();
        insert_memory(&conn, "system", "sys mem", "build stuff", &e2).unwrap();

        let code_only = search_memories(&conn, &e1, Some("code"), 5).unwrap();
        assert_eq!(code_only.len(), 1);
        assert_eq!(code_only[0].bucket, "code");
    }

    #[test]
    fn upsert_memory_updates_existing() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        let e1 = vec![0.1_f32, 0.2, 0.3];
        let id1 = insert_memory(&conn, "code", "my-topic", "old content", &e1).unwrap();

        let e2 = vec![0.4_f32, 0.5, 0.6];
        let id2 = upsert_memory_by_title(&conn, "code", "my-topic", "new content", &e2).unwrap();

        assert_eq!(id1, id2, "should update same row, not insert new");

        // Verify content updated
        let content: String = conn
            .query_row(
                "SELECT content FROM memories WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "new content");
    }

    #[test]
    fn build_index_does_not_clear_memories() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        // Insert a memory
        insert_memory(
            &conn,
            "code",
            "keep me",
            "important knowledge",
            &[0.1, 0.2, 0.3],
        )
        .unwrap();

        // Also insert a code chunk so we have data in the chunks/files tables
        insert_chunk(
            &conn,
            &dummy_chunk("src/main.rs", "fn main() {}"),
            &[0.4_f32, 0.5, 0.6],
        )
        .unwrap();

        // Simulate what build_index does to code chunks (it clears and rebuilds)
        conn.execute("DELETE FROM chunks", []).unwrap();
        conn.execute("DELETE FROM files", []).unwrap();

        // Memories should survive — they live in separate tables
        let results = search_memories(&conn, &[0.1, 0.2, 0.3], None, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "keep me");
        assert_eq!(results[0].content, "important knowledge");
    }

    #[test]
    fn search_memories_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        let results = search_memories(&conn, &[0.1, 0.2, 0.3], None, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn insert_memory_timestamps_are_set() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        insert_memory(&conn, "code", "test", "content", &[0.1, 0.2, 0.3]).unwrap();

        let (created, updated): (String, String) = conn
            .query_row(
                "SELECT created_at, updated_at FROM memories WHERE title = 'test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert!(!created.is_empty());
        assert!(!updated.is_empty());
        assert_eq!(created, updated); // Same on first insert
    }

    #[test]
    fn upsert_memory_updates_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        insert_memory(&conn, "code", "ts-test", "v1", &[0.1, 0.2, 0.3]).unwrap();

        let created1: String = conn
            .query_row(
                "SELECT created_at FROM memories WHERE title = 'ts-test'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Upsert with new content
        upsert_memory_by_title(&conn, "code", "ts-test", "v2", &[0.4, 0.5, 0.6]).unwrap();

        let (created2, updated2): (String, String) = conn
            .query_row(
                "SELECT created_at, updated_at FROM memories WHERE title = 'ts-test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert_eq!(created1, created2, "created_at should not change on upsert");
        // updated_at may or may not differ (depends on timing) but should exist
        assert!(!updated2.is_empty());
    }

    #[test]
    fn delete_nonexistent_memory_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        set_meta(&conn, "embedding_dims", "3").unwrap();
        ensure_vec_memories(&conn).unwrap();

        // Deleting a memory that doesn't exist should return a RecoverableError
        let result = delete_memory(&conn, 99999);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found"),
            "expected 'not found' in error: {err_msg}"
        );
    }
}
