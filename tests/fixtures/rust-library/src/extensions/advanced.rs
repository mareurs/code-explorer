use crate::models::book::Book;
use crate::traits::searchable::Searchable;

/// Extension: derive macros generate code (Debug, Clone).
#[derive(Debug, Clone, PartialEq)]
pub struct BookRef {
    pub title: String,
    pub available: bool,
}

/// Extension: lifetime annotations.
pub fn borrow_title<'a>(book: &'a Book) -> &'a str {
    book.title()
}

/// Extension: impl Trait return type.
pub fn available_titles(books: &[Book]) -> impl Iterator<Item = &str> {
    books.iter().filter(|b| b.is_available()).map(|b| b.title())
}

/// Extension: re-export (pub use).
pub use crate::models::genre::Genre as BookGenre;
