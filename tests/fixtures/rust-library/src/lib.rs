pub mod models;
pub mod traits;
pub mod services;
pub mod extensions;

// Core: re-export for convenience
pub use models::book::Book;
pub use models::genre::Genre;
pub use traits::searchable::Searchable;
pub use services::catalog::Catalog;
