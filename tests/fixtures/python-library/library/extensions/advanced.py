from __future__ import annotations
from typing import Any
from library.models.book import Book
from library.interfaces.searchable import Searchable


# Extension: type alias
BookList = list[Book]


class Playable:
    """Mixin for items that can be played (audiobooks)."""

    def play(self) -> str:
        return "Playing..."

    def duration_minutes(self) -> int:
        return 0


class AudioBook(Book, Playable):
    """Extension: multiple inheritance with MRO."""

    narrator: str = ""

    def search_text(self) -> str:
        return f"{self.title} (narrated by {self.narrator})"


def search_books(*terms: str, **filters: Any) -> BookList:
    """Extension: *args and **kwargs in signature."""
    return []


def rank_results(books: BookList) -> BookList:
    """Extension: uses type alias in signature."""

    def _score(book: Book) -> float:
        """Extension: nested function / closure."""
        return 1.0 if book.is_available else 0.5

    return sorted(books, key=_score, reverse=True)
