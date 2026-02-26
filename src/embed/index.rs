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
//!   chunk_embeddings(rowid, embedding)         — sqlite-vec virtual table
//!   meta(key TEXT, value TEXT)                  — stores embed_model, last_indexed_commit
//!
//! Change detection fallback chain:
//!   1. git diff last_indexed_commit..HEAD (tracked files)
//!   2. mtime comparison (untracked files or git unavailable)
//!   3. SHA-256 hash (final arbiter)
//!
//! TODO: Load the sqlite-vec extension at connection time:
//!   conn.load_extension_enable()?;
//!   conn.load_extension("sqlite_vec", None)?;
//!   conn.load_extension_disable()?;

use anyhow::Result;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::schema::{CodeChunk, SearchResult};

/// Path to the embedding database within a project.
pub fn db_path(project_root: &Path) -> PathBuf {
    project_root.join(".code-explorer").join("embeddings.db")
}

/// Open (or create) the embedding database and apply the schema.
pub fn open_db(project_root: &Path) -> Result<Connection> {
    let path = db_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(&path)?;

    // TODO: load sqlite-vec extension here
    // conn.load_extension_enable()?;
    // conn.load_extension("sqlite_vec", None)?;

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

        -- TODO: replace with sqlite-vec virtual table once extension is loaded:
        -- CREATE VIRTUAL TABLE IF NOT EXISTS chunk_embeddings
        --   USING vec0(embedding float[768]);
        CREATE TABLE IF NOT EXISTS chunk_embeddings (
            rowid     INTEGER PRIMARY KEY,
            embedding BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;

    // Migrate: add mtime column if missing (safe no-op if already present)
    let has_mtime: bool = conn.prepare("SELECT mtime FROM files LIMIT 0").is_ok();
    if !has_mtime {
        conn.execute_batch("ALTER TABLE files ADD COLUMN mtime INTEGER")?;
    }

    Ok(conn)
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

/// Naive cosine similarity search (pure Rust fallback, no sqlite-vec).
///
/// TODO: Replace with sqlite-vec virtual table query for production:
///   SELECT c.*, vec_distance_cosine(ce.embedding, ?1) AS distance
///   FROM chunk_embeddings ce JOIN chunks c ON c.id = ce.rowid
///   ORDER BY distance LIMIT ?2
pub fn search(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    search_scoped(conn, query_embedding, limit, None)
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
    let (where_clause, filter_param): (&str, Option<String>) = match source_filter {
        None => ("", None),
        Some("project") => ("WHERE c.source = ?1", Some("project".to_string())),
        Some("libraries") => ("WHERE c.source != 'project'", None),
        Some(x) if x.starts_with("lib:") => ("WHERE c.source = ?1", Some(x.to_string())),
        Some(x) => ("WHERE c.source = ?1", Some(x.to_string())),
    };

    let sql = format!(
        "SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, c.source, ce.embedding
         FROM chunks c JOIN chunk_embeddings ce ON c.id = ce.rowid {where_clause}"
    );

    let mut stmt = conn.prepare(&sql)?;

    // Collect rows into a Vec to avoid closure type mismatch between if/else branches
    type Row = (String, String, String, usize, usize, String, Vec<u8>);
    let rows: Vec<Row> = if let Some(ref param) = filter_param {
        let mut rows_out = Vec::new();
        let mut query_rows = stmt.query(params![param])?;
        while let Some(row) = query_rows.next()? {
            let blob: Vec<u8> = row.get(6)?;
            rows_out.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, usize>(3)?,
                row.get::<_, usize>(4)?,
                row.get::<_, String>(5)?,
                blob,
            ));
        }
        rows_out
    } else {
        let mut rows_out = Vec::new();
        let mut query_rows = stmt.query([])?;
        while let Some(row) = query_rows.next()? {
            let blob: Vec<u8> = row.get(6)?;
            rows_out.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, usize>(3)?,
                row.get::<_, usize>(4)?,
                row.get::<_, String>(5)?,
                blob,
            ));
        }
        rows_out
    };

    let qnorm = l2_norm(query_embedding);
    let mut scored: Vec<(f32, SearchResult)> = rows
        .into_iter()
        .map(|(fp, lang, content, sl, el, source, blob)| {
            let emb = bytes_to_f32(&blob);
            let sim = cosine_sim(query_embedding, &emb, qnorm);
            (
                sim,
                SearchResult {
                    file_path: fp,
                    language: lang,
                    content,
                    start_line: sl,
                    end_line: el,
                    score: sim,
                    source,
                },
            )
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(limit).map(|(_, r)| r).collect())
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine_sim(a: &[f32], b: &[f32], a_norm: f32) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let b_norm = l2_norm(b);
    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }
    (dot / (a_norm * b_norm)).clamp(0.0, 1.0)
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
            config.embeddings.chunk_size,
            config.embeddings.chunk_overlap,
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
                source: "project".into(),
            };
            insert_chunk(&conn, &chunk, emb)?;
        }
        upsert_file_hash(&conn, &result.rel, &result.hash, Some(result.mtime))?;
        tracing::debug!("indexed {} ({} chunks)", result.rel, result.chunks.len());
    }
    set_meta(&conn, "embed_model", &config.embeddings.model)?;

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
    })
}

/// Build or incrementally update the embedding index for a library.
///
/// Similar to `build_index` but walks `library_path` instead of the project root,
/// and tags all chunks with the given `source` string (e.g. "lib:serde").
/// The DB is stored under `project_root/.code-explorer/embeddings.db` (shared with project).
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
            config.embeddings.chunk_size,
            config.embeddings.chunk_overlap,
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
}

/// Statistics about the embedding index.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStats {
    pub file_count: usize,
    pub chunk_count: usize,
    pub embedding_count: usize,
    /// Model string stored at index time, if any.
    pub model: Option<String>,
}

/// Query index statistics from the database.
pub fn index_stats(conn: &Connection) -> Result<IndexStats> {
    let file_count: usize = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunk_count: usize = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let embedding_count: usize =
        conn.query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))?;
    let model = get_meta(conn, "embed_model")?;
    Ok(IndexStats {
        file_count,
        chunk_count,
        embedding_count,
        model,
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
             Delete .code-explorer/embeddings.db and re-run `index` to rebuild."
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

/// Get the SHA of the last commit that was fully indexed.
pub fn get_last_indexed_commit(conn: &Connection) -> Result<Option<String>> {
    get_meta(conn, "last_indexed_commit")
}

/// Record the SHA of the last commit that was fully indexed.
pub fn set_last_indexed_commit(conn: &Connection, sha: &str) -> Result<()> {
    set_meta(conn, "last_indexed_commit", sha)
}

#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::schema::CodeChunk;
    use tempfile::tempdir;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let conn = open_db(dir.path()).unwrap();
        (dir, conn)
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
}
