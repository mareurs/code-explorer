/// Genre categories for library books.
#[derive(Debug, Clone, PartialEq)]
pub enum Genre {
    Fiction,
    NonFiction,
    Science,
    History,
    Biography,
}

impl Genre {
    /// Human-readable label for display.
    pub fn label(&self) -> &str {
        match self {
            Genre::Fiction => "Fiction",
            Genre::NonFiction => "Non-Fiction",
            Genre::Science => "Science",
            Genre::History => "History",
            Genre::Biography => "Biography",
        }
    }
}
