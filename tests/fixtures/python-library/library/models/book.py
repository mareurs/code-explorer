from __future__ import annotations
from dataclasses import dataclass, field
from library.models.genre import Genre


MAX_RESULTS: int = 100
"""Maximum number of search results to return."""


@dataclass
class Book:
    """A book in the library catalog."""

    title: str
    isbn: str
    genre: Genre
    copies_available: int = 1

    @property
    def is_available(self) -> bool:
        """Check if the book is available for borrowing."""
        return self.copies_available > 0

    def __repr__(self) -> str:
        return f"Book(title={self.title!r}, isbn={self.isbn!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Book):
            return NotImplemented
        return self.isbn == other.isbn

    def __hash__(self) -> int:
        return hash(self.isbn)
