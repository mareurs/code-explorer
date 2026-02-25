//! sqlite-vec based embedding index with incremental updates.
//!
//! Inspired by cocoindex-code's SQLite + sqlite-vec approach:
//! zero external services, embedded in the project directory.
//!
//! Schema:
//!   files(path TEXT, hash TEXT)            — tracks indexed file hashes
//!   chunks(id, file_path, language,         — code chunks
//!          content, start_line, end_line,
//!          file_hash)
//!   chunk_embeddings(rowid, embedding)      — sqlite-vec virtual table
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
            hash  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS chunks (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path  TEXT NOT NULL,
            language   TEXT NOT NULL,
            content    TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line   INTEGER NOT NULL,
            file_hash  TEXT NOT NULL
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

    Ok(conn)
}

/// Hash the content of a file for change detection.
pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    Ok(hex::encode(digest))
}

/// Insert a chunk and its embedding into the database.
pub fn insert_chunk(conn: &Connection, chunk: &CodeChunk, embedding: &[f32]) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (file_path, language, content, start_line, end_line, file_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            chunk.file_path,
            chunk.language,
            chunk.content,
            chunk.start_line,
            chunk.end_line,
            chunk.file_hash,
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

/// Get the stored hash for a file (for incremental indexing).
pub fn get_file_hash(conn: &Connection, file_path: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT hash FROM files WHERE path = ?1")?;
    let mut rows = stmt.query(params![file_path])?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

/// Update or insert the file hash record.
pub fn upsert_file_hash(conn: &Connection, file_path: &str, hash: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO files (path, hash) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET hash = excluded.hash",
        params![file_path, hash],
    )?;
    Ok(())
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
    let mut stmt = conn.prepare(
        "SELECT c.file_path, c.language, c.content, c.start_line, c.end_line, ce.embedding
         FROM chunks c JOIN chunk_embeddings ce ON c.id = ce.rowid",
    )?;

    let qnorm = l2_norm(query_embedding);
    let mut scored: Vec<(f32, SearchResult)> = stmt
        .query_map([], |row| {
            let blob: Vec<u8> = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, usize>(3)?,
                row.get::<_, usize>(4)?,
                blob,
            ))
        })?
        .filter_map(|r| r.ok())
        .map(|(fp, lang, content, sl, el, blob)| {
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

/// Build or incrementally update the embedding index for a project.
///
/// Three-phase pipeline for maximum throughput:
///   1. Walk + hash + chunk  (sequential, CPU-bound)
///   2. Embed concurrently   (up to 4 in-flight HTTP requests at once)
///   3. DB writes in a single transaction  (eliminates per-chunk commit overhead)
pub async fn build_index(project_root: &Path, force: bool) -> Result<()> {
    use crate::ast::detect_language;
    use crate::config::ProjectConfig;
    use crate::embed::{create_embedder, Embedding};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let config = ProjectConfig::load_or_default(project_root)?;
    let conn = open_db(project_root)?;
    let embedder: Arc<dyn crate::embed::Embedder> =
        Arc::from(create_embedder(&config.embeddings.model).await?);
    check_model_mismatch(&conn, &config.embeddings.model)?;

    // ── Phase 1: Walk, hash, chunk ────────────────────────────────────────────
    struct FileWork {
        rel: String,
        hash: String,
        lang: String,
        chunks: Vec<super::chunker::RawChunk>,
    }

    let ignored = config.ignored_paths.patterns.clone();
    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(move |entry| {
            let name = entry.file_name().to_string_lossy();
            !ignored.iter().any(|p| p.as_str() == name.as_ref())
        })
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

        let rel = path
            .strip_prefix(project_root)?
            .to_string_lossy()
            .to_string();
        let hash = hash_file(path)?;

        if !force {
            if let Some(stored) = get_file_hash(&conn, &rel)? {
                if stored == hash {
                    skipped += 1;
                    continue;
                }
            }
        }

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let chunks = super::ast_chunker::split_file(
            &source,
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
            };
            insert_chunk(&conn, &chunk, emb)?;
        }
        upsert_file_hash(&conn, &result.rel, &result.hash)?;
        tracing::debug!("indexed {} ({} chunks)", result.rel, result.chunks.len());
    }
    conn.execute_batch("COMMIT")?;

    set_meta(&conn, "embed_model", &config.embeddings.model)?;
    tracing::info!(
        "Index complete: {} files indexed, {} unchanged",
        indexed,
        skipped
    );
    Ok(())
}

/// Statistics about the embedding index.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStats {
    pub file_count: usize,
    pub chunk_count: usize,
    pub embedding_count: usize,
}

/// Query index statistics from the database.
pub fn index_stats(conn: &Connection) -> Result<IndexStats> {
    let file_count: usize = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunk_count: usize = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let embedding_count: usize =
        conn.query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))?;
    Ok(IndexStats {
        file_count,
        chunk_count,
        embedding_count,
    })
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
        upsert_file_hash(&conn, "src/lib.rs", "aabbcc").unwrap();
        assert_eq!(
            get_file_hash(&conn, "src/lib.rs").unwrap(),
            Some("aabbcc".to_string())
        );
    }

    #[test]
    fn file_hash_upsert_updates_on_conflict() {
        let (_dir, conn) = open_test_db();
        upsert_file_hash(&conn, "src/lib.rs", "hash1").unwrap();
        upsert_file_hash(&conn, "src/lib.rs", "hash2").unwrap();
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
        upsert_file_hash(&conn, "del.rs", "abc").unwrap();

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
}
