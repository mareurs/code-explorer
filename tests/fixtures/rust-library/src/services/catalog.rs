use crate::traits::searchable::Searchable;

/// A catalog that holds searchable items.
pub struct Catalog<T: Searchable> {
    items: Vec<T>,
    name: String,
}

/// Nested type: statistics about the catalog.
pub struct CatalogStats {
    pub total_items: usize,
    pub name: String,
}

impl<T: Searchable> Catalog<T> {
    /// Create a new empty catalog.
    pub fn new(name: String) -> Self {
        Self {
            items: Vec::new(),
            name,
        }
    }

    /// Add an item to the catalog.
    pub fn add(&mut self, item: T) {
        self.items.push(item);
    }

    /// Search for items matching a query.
    pub fn search(&self, query: &str) -> Vec<&T> {
        self.items
            .iter()
            .filter(|item| item.search_text().contains(query))
            .collect()
    }

    /// Get catalog statistics.
    pub fn stats(&self) -> CatalogStats {
        CatalogStats {
            total_items: self.items.len(),
            name: self.name.clone(),
        }
    }
}

/// Free function: create a default catalog for books.
pub fn create_default_catalog() -> Catalog<crate::models::book::Book> {
    Catalog::new("Main Library".to_string())
}
