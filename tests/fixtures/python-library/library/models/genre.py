from enum import Enum


class Genre(Enum):
    """Genre categories for library books."""

    FICTION = "fiction"
    NON_FICTION = "non_fiction"
    SCIENCE = "science"
    HISTORY = "history"
    BIOGRAPHY = "biography"

    def label(self) -> str:
        """Human-readable label for display."""
        return self.value.replace("_", " ").title()
