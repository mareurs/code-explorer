//! Data types for the embedding index.

use serde::{Deserialize, Serialize};

/// A chunk of source code stored in the embedding index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    /// Unique row ID (auto-assigned by SQLite)
    pub id: Option<i64>,
    /// File path relative to project root
    pub file_path: String,
    /// Programming language (e.g. "rust", "python")
    pub language: String,
    /// Source text of this chunk
    pub content: String,
    /// 1-indexed start line
    pub start_line: usize,
    /// 1-indexed end line (inclusive)
    pub end_line: usize,
    /// SHA-256 hash of the file content at index time (for incremental updates)
    pub file_hash: String,
    /// Source identifier: "project" for project files, "lib:<name>" for libraries
    pub source: String,
}

/// A ranked result from a semantic search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub file_path: String,
    pub language: String,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    /// Cosine similarity score in [0, 1]; higher is more relevant.
    pub score: f32,
    /// Source identifier: "project" or "lib:<name>"
    pub source: String,
}
