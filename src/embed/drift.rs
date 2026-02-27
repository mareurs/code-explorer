//! File-level drift detection: compare old and new chunks to quantify change.
//!
//! Pure functions with no database access. Receives old chunks (from
//! `read_file_embeddings`) and new chunks (from the embedding phase),
//! and computes drift scores using content hashing and cosine similarity.

use super::index::{cosine_sim, l2_norm, OldChunk};

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
    file_path: &str,
    old_chunks: &[OldChunk],
    new_chunks: &[NewChunk],
) -> FileDrift {
    // Both empty → zero drift
    if old_chunks.is_empty() && new_chunks.is_empty() {
        return FileDrift {
            file_path: file_path.to_string(),
            avg_drift: 0.0,
            max_drift: 0.0,
            max_drift_chunk: None,
            chunks_added: 0,
            chunks_removed: 0,
        };
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

    // Step 2: Greedy best-cosine pairing on remainder
    if !unmatched_old.is_empty() && !unmatched_new.is_empty() {
        // Compute all pairwise similarities
        let mut pairs: Vec<(usize, usize, f32)> = Vec::new();
        for &oi in &unmatched_old {
            let a_norm = l2_norm(&old_chunks[oi].embedding);
            for &ni in &unmatched_new {
                let sim = cosine_sim(&old_chunks[oi].embedding, &new_chunks[ni].embedding, a_norm);
                pairs.push((oi, ni, sim));
            }
        }

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

    FileDrift {
        file_path: file_path.to_string(),
        avg_drift,
        max_drift,
        max_drift_chunk,
        chunks_added,
        chunks_removed,
    }
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
        let drift = compute_file_drift("a.rs", &olds, &news);
        assert_eq!(drift.avg_drift, 0.0);
        assert_eq!(drift.max_drift, 0.0);
        assert_eq!(drift.chunks_added, 0);
        assert_eq!(drift.chunks_removed, 0);
    }

    #[test]
    fn completely_different_chunks_have_high_drift() {
        let olds = vec![old("fn a() {}", &[1.0, 0.0, 0.0])];
        let news = vec![new("fn b() { completely_different() }", &[0.0, 1.0, 0.0])];
        let drift = compute_file_drift("a.rs", &olds, &news);
        assert!(drift.avg_drift > 0.9);
        assert!(drift.max_drift > 0.9);
    }

    #[test]
    fn added_chunks_count_as_full_drift() {
        let olds = vec![];
        let news = vec![new("fn new_func() {}", &[1.0, 0.0, 0.0])];
        let drift = compute_file_drift("a.rs", &olds, &news);
        assert_eq!(drift.avg_drift, 1.0);
        assert_eq!(drift.max_drift, 1.0);
        assert_eq!(drift.chunks_added, 1);
        assert_eq!(drift.chunks_removed, 0);
    }

    #[test]
    fn removed_chunks_count_as_full_drift() {
        let olds = vec![old("fn old_func() {}", &[1.0, 0.0, 0.0])];
        let news = vec![];
        let drift = compute_file_drift("a.rs", &olds, &news);
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
        let drift = compute_file_drift("a.rs", &olds, &news);
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
        let drift = compute_file_drift("a.rs", &olds, &news);
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
        let drift = compute_file_drift("a.rs", &olds, &news);
        assert!(drift.max_drift_chunk.is_some());
        let snippet = drift.max_drift_chunk.unwrap();
        assert!(snippet.contains("volatile"));
    }

    #[test]
    fn both_empty_means_zero_drift() {
        let drift = compute_file_drift("a.rs", &[], &[]);
        assert_eq!(drift.avg_drift, 0.0);
        assert_eq!(drift.max_drift, 0.0);
    }
}
