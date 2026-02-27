from abc import ABC, abstractmethod
from typing import Protocol, runtime_checkable


class Searchable(ABC):
    """Interface for anything that can be searched in the catalog."""

    @abstractmethod
    def search_text(self) -> str:
        """Return a search-friendly text representation."""
        ...

    def relevance(self) -> float:
        """Default relevance score — override for custom ranking."""
        return 0.0


@runtime_checkable
class HasISBN(Protocol):
    """Extension: structural typing via Protocol."""

    @property
    def isbn(self) -> str: ...
