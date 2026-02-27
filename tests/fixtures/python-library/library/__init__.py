"""Library management system."""
from library.models.book import Book
from library.models.genre import Genre
from library.interfaces.searchable import Searchable
from library.services.catalog import Catalog
