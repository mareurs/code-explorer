use crate::models::book::Book;

/// Search result with structured variants (Rust extension: struct + tuple variants).
pub enum SearchResult {
    /// Found a match with a relevance score.
    Found { book: Book, score: f64 },
    /// No results for the query.
    NotFound(String),
    /// Search error with message.
    Error { message: String, code: u32 },
}

impl SearchResult {
    /// Check if this result contains a match.
    pub fn is_match(&self) -> bool {
        matches!(self, SearchResult::Found { .. })
    }
}

/// Extension: Iterator with associated type.
pub struct BookIterator {
    books: Vec<Book>,
    index: usize,
}

impl Iterator for BookIterator {
    type Item = Book;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.books.len() {
            self.index += 1;
            // In real code we'd use a different approach; this is for testing symbol discovery
            None
        } else {
            None
        }
    }
}
