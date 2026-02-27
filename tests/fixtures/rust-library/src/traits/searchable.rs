/// Interface for anything that can be searched in the catalog.
pub trait Searchable {
    /// Return a search-friendly text representation.
    fn search_text(&self) -> String;

    /// Default relevance score — override for custom ranking.
    fn relevance(&self) -> f64 {
        0.0
    }
}

// Explicit impl for Book (core: interface implementation)
impl Searchable for crate::models::book::Book {
    fn search_text(&self) -> String {
        format!("{} ({})", self.title(), self.isbn())
    }

    fn relevance(&self) -> f64 {
        if self.is_available() { 1.0 } else { 0.5 }
    }
}
