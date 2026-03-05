//! File-level drift detection: compare old and new chunks to quantify change.
//!
//! Receives old chunks (from `read_file_embeddings`) and new chunks (from the
//! embedding phase), and computes drift scores using content hashing and
//! cosine similarity via sqlite-vec's `vec_distance_cosine`.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::index::OldChunk;

/// Minimum cosine similarity to consider two chunks semantically related.
const SEMANTIC_MATCH_THRESHOLD: f32 = 0.3;

/// Maximum length for the `max_drift_chunk` content snippet.
const SNIPPET_MAX_LEN: usize = 200;

/// Per-file drift result after comparing old vs new chunks.
#[derive(Debug)]
pub struct FileDrift {
    pub file_path: String,
    pub avg_drift: f32,
    pub max_drift: f32,
    pub max_drift_chunk: Option<String>,
    pub chunks_added: usize,
    pub chunks_removed: usize,
}

/// A new chunk with content and its embedding vector.
#[derive(Debug, Clone)]
pub struct NewChunk {
    pub content: String,
    pub embedding: Vec<f32>,
}

/// Compare old and new chunks for a single file and compute drift scores.
///
/// Algorithm:
/// 1. Content-hash exact matching (fast path) — identical content gets drift 0.0
/// 2. Greedy best-cosine pairing on remainder — semantic matching
/// 3. Classify unmatched as added/removed (drift 1.0 each)
/// 4. Aggregate into avg_drift, max_drift, max_drift_chunk
pub fn compute_file_drift(
    conn: &Connection,
    file_path: &str,
    old_chunks: &[OldChunk],
    new_chunks: &[NewChunk],
) -> Result<FileDrift> {
    // Both empty → zero drift
    if old_chunks.is_empty() && new_chunks.is_empty() {
        return Ok(FileDrift {
            file_path: file_path.to_string(),
            avg_drift: 0.0,
            max_drift: 0.0,
            max_drift_chunk: None,
            chunks_added: 0,
            chunks_removed: 0,
        });
    }

    // Track all individual drift values and their associated content
    let mut drifts: Vec<(f32, Option<String>)> = Vec::new();
    let mut chunks_added: usize = 0;
    let mut chunks_removed: usize = 0;

    // Step 1: Content-hash exact matching (fast path)
    // Track which old/new chunks are still unmatched
    let mut old_matched = vec![false; old_chunks.len()];
    let mut new_matched = vec![false; new_chunks.len()];

    for (oi, old) in old_chunks.iter().enumerate() {
        for (ni, new) in new_chunks.iter().enumerate() {
            if !new_matched[ni] && old.content == new.content {
                old_matched[oi] = true;
                new_matched[ni] = true;
                drifts.push((0.0, None));
                break;
            }
        }
    }

    // Collect unmatched indices
    let unmatched_old: Vec<usize> = old_matched
        .iter()
        .enumerate()
        .filter(|(_, matched)| !**matched)
        .map(|(i, _)| i)
        .collect();

    let unmatched_new: Vec<usize> = new_matched
        .iter()
        .enumerate()
        .filter(|(_, matched)| !**matched)
        .map(|(i, _)| i)
        .collect();

    // Step 2: Greedy best-cosine pairing via sqlite-vec vec_distance_cosine.
    if !unmatched_old.is_empty() && !unmatched_new.is_empty() {
        // Load unmatched old embeddings into a temp table so each new-chunk
        // query can scan all old embeddings in one SQL round-trip.
        conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _drift_old (oi INTEGER, embedding BLOB); \
             DELETE FROM _drift_old;",
        )?;

        // SG-7: Wrap the query loop in a closure so _drift_old is dropped even
        // if an early `?` fires. Without this, the temp table leaks on error.
        let pairs_result = (|| -> Result<Vec<(usize, usize, f32)>> {
            for &oi in &unmatched_old {
                let blob: Vec<u8> = old_chunks[oi]
                    .embedding
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                conn.execute(
                    "INSERT INTO _drift_old (oi, embedding) VALUES (?1, ?2)",
                    params![oi as i64, blob],
                )?;
            }

            // One query per new chunk returns distances to all old chunks.
            // vec_distance_cosine returns cosine distance ∈ [0,1] (0 = identical);
            // COALESCE maps NULL (degenerate zero-vector) to 1.0 (maximum distance).
            let mut pairs: Vec<(usize, usize, f32)> = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT oi, \
                     COALESCE(vec_distance_cosine(vec_f32(embedding), vec_f32(?1)), 1.0) \
                     FROM _drift_old",
                )?;
                for &ni in &unmatched_new {
                    let b_blob: Vec<u8> = new_chunks[ni]
                        .embedding
                        .iter()
                        .flat_map(|f| f.to_le_bytes())
                        .collect();
                    let rows = stmt
                        .query_map(params![b_blob], |r| {
                            let oi: i64 = r.get(0)?;
                            let dist: f64 = r.get(1)?;
                            Ok((oi as usize, (1.0_f32 - dist as f32).clamp(0.0, 1.0)))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (oi, sim) in rows {
                        pairs.push((oi, ni, sim));
                    }
                }
            }
            Ok(pairs)
        })();
        // Always clean up the temp table, even if the query loop failed.
        conn.execute_batch("DROP TABLE IF EXISTS _drift_old")?;
        let mut pairs = pairs_result?;

        // Sort by similarity descending
        pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // Greedy assignment
        let mut old_assigned = vec![false; old_chunks.len()];
        let mut new_assigned = vec![false; new_chunks.len()];

        for (oi, ni, sim) in &pairs {
            if old_assigned[*oi] || new_assigned[*ni] {
                continue;
            }
            if *sim < SEMANTIC_MATCH_THRESHOLD {
                // Below threshold — stop, remaining are unmatched
                break;
            }
            old_assigned[*oi] = true;
            new_assigned[*ni] = true;
            let drift_val = 1.0 - sim;
            let snippet = snippet(&new_chunks[*ni].content);
            drifts.push((drift_val, Some(snippet)));
        }

        // Step 3: Classify unmatched
        for &oi in &unmatched_old {
            if !old_assigned[oi] {
                chunks_removed += 1;
                let snippet = snippet(&old_chunks[oi].content);
                drifts.push((1.0, Some(snippet)));
            }
        }
        for &ni in &unmatched_new {
            if !new_assigned[ni] {
                chunks_added += 1;
                let snippet = snippet(&new_chunks[ni].content);
                drifts.push((1.0, Some(snippet)));
            }
        }
    } else {
        // One side is empty after content matching
        for &oi in &unmatched_old {
            chunks_removed += 1;
            let snippet = snippet(&old_chunks[oi].content);
            drifts.push((1.0, Some(snippet)));
        }
        for &ni in &unmatched_new {
            chunks_added += 1;
            let snippet = snippet(&new_chunks[ni].content);
            drifts.push((1.0, Some(snippet)));
        }
    }

    // Step 4: Aggregate
    let total = drifts.len();
    let avg_drift = if total > 0 {
        drifts.iter().map(|(d, _)| d).sum::<f32>() / total as f32
    } else {
        0.0
    };

    let (max_drift, max_drift_chunk) = drifts
        .iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(d, s)| (*d, s.clone()))
        .unwrap_or((0.0, None));

    Ok(FileDrift {
        file_path: file_path.to_string(),
        avg_drift,
        max_drift,
        max_drift_chunk,
        chunks_added,
        chunks_removed,
    })
}

/// Truncate content to a snippet of at most `SNIPPET_MAX_LEN` characters.
fn snippet(content: &str) -> String {
    if content.len() <= SNIPPET_MAX_LEN {
        content.to_string()
    } else {
        let truncated: String = content.chars().take(SNIPPET_MAX_LEN).collect();
        format!("{}...", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn conn() -> Connection {
        crate::embed::index::init_sqlite_vec();
        Connection::open_in_memory().unwrap()
    }

    fn old(content: &str, emb: &[f32]) -> OldChunk {
        OldChunk {
            content: content.to_string(),
            embedding: emb.to_vec(),
        }
    }

    fn new(content: &str, emb: &[f32]) -> NewChunk {
        NewChunk {
            content: content.to_string(),
            embedding: emb.to_vec(),
        }
    }

    #[test]
    fn identical_chunks_have_zero_drift() {
        let olds = vec![old("fn a() {}", &[1.0, 0.0, 0.0])];
        let news = vec![new("fn a() {}", &[1.0, 0.0, 0.0])];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert_eq!(drift.avg_drift, 0.0);
        assert_eq!(drift.max_drift, 0.0);
        assert_eq!(drift.chunks_added, 0);
        assert_eq!(drift.chunks_removed, 0);
    }

    #[test]
    fn completely_different_chunks_have_high_drift() {
        let olds = vec![old("fn a() {}", &[1.0, 0.0, 0.0])];
        let news = vec![new("fn b() { completely_different() }", &[0.0, 1.0, 0.0])];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert!(drift.avg_drift > 0.9);
        assert!(drift.max_drift > 0.9);
    }

    #[test]
    fn added_chunks_count_as_full_drift() {
        let olds = vec![];
        let news = vec![new("fn new_func() {}", &[1.0, 0.0, 0.0])];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert_eq!(drift.avg_drift, 1.0);
        assert_eq!(drift.max_drift, 1.0);
        assert_eq!(drift.chunks_added, 1);
        assert_eq!(drift.chunks_removed, 0);
    }

    #[test]
    fn removed_chunks_count_as_full_drift() {
        let olds = vec![old("fn old_func() {}", &[1.0, 0.0, 0.0])];
        let news = vec![];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert_eq!(drift.avg_drift, 1.0);
        assert_eq!(drift.max_drift, 1.0);
        assert_eq!(drift.chunks_added, 0);
        assert_eq!(drift.chunks_removed, 1);
    }

    #[test]
    fn content_hash_match_skips_semantic_comparison() {
        // Same content, different embeddings -> content match wins, drift = 0.0
        let olds = vec![old("fn a() {}", &[1.0, 0.0, 0.0])];
        let news = vec![new("fn a() {}", &[0.0, 1.0, 0.0])];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert_eq!(drift.avg_drift, 0.0);
        assert_eq!(drift.max_drift, 0.0);
    }

    #[test]
    fn mixed_matched_and_added() {
        let olds = vec![
            old("fn unchanged() {}", &[1.0, 0.0, 0.0]),
            old("fn tweaked() { v1 }", &[0.0, 1.0, 0.0]),
        ];
        let news = vec![
            new("fn unchanged() {}", &[1.0, 0.0, 0.0]),
            new("fn tweaked() { v2 }", &[0.1, 0.9, 0.0]),
            new("fn brand_new() {}", &[0.0, 0.0, 1.0]),
        ];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert_eq!(drift.chunks_added, 1);
        assert_eq!(drift.chunks_removed, 0);
        assert!(drift.avg_drift > 0.3);
        assert_eq!(drift.max_drift, 1.0);
    }

    #[test]
    fn max_drift_chunk_is_most_drifted_content() {
        let olds = vec![
            old("fn stable() {}", &[1.0, 0.0, 0.0]),
            old("fn volatile() { old_impl }", &[0.0, 1.0, 0.0]),
        ];
        let news = vec![
            new("fn stable() {}", &[1.0, 0.0, 0.0]),
            new("fn volatile() { new_impl }", &[0.0, 0.0, 1.0]),
        ];
        let drift = compute_file_drift(&conn(), "a.rs", &olds, &news).unwrap();
        assert!(drift.max_drift_chunk.is_some());
        let snippet = drift.max_drift_chunk.unwrap();
        assert!(snippet.contains("volatile"));
    }

    #[test]
    fn both_empty_means_zero_drift() {
        let drift = compute_file_drift(&conn(), "a.rs", &[], &[]).unwrap();
        assert_eq!(drift.avg_drift, 0.0);
        assert_eq!(drift.max_drift, 0.0);
    }
}
