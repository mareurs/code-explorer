from __future__ import annotations
from typing import Generic, TypeVar
from library.interfaces.searchable import Searchable

T = TypeVar("T", bound=Searchable)


class Catalog(Generic[T]):
    """A catalog that holds searchable items."""

    class Stats:
        """Nested class: statistics about the catalog."""

        def __init__(self, total_items: int, name: str) -> None:
            self.total_items = total_items
            self.name = name

    def __init__(self, name: str) -> None:
        self._items: list[T] = []
        self._name = name

    def add(self, item: T) -> None:
        """Add an item to the catalog."""
        self._items.append(item)

    def search(self, query: str) -> list[T]:
        """Search for items matching a query."""
        return [item for item in self._items if query in item.search_text()]

    def stats(self) -> Stats:
        """Get catalog statistics."""
        return self.Stats(total_items=len(self._items), name=self._name)


def create_default_catalog() -> Catalog:
    """Free function: create a default catalog for books."""
    return Catalog(name="Main Library")
