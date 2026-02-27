/// A book in the library catalog.
pub struct Book {
    title: String,
    isbn: String,
    genre: super::genre::Genre,
    copies_available: u32,
}

/// Maximum number of search results to return.
pub const MAX_RESULTS: usize = 100;

impl Book {
    /// Create a new book.
    pub fn new(title: String, isbn: String, genre: super::genre::Genre) -> Self {
        Self {
            title,
            isbn,
            genre,
            copies_available: 1,
        }
    }

    /// Get the book title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Get the ISBN.
    pub fn isbn(&self) -> &str {
        &self.isbn
    }

    /// Check if the book is available for borrowing.
    pub fn is_available(&self) -> bool {
        self.copies_available > 0
    }

    /// Get the genre.
    pub fn genre(&self) -> &super::genre::Genre {
        &self.genre
    }
}
