package library.models

/** Genre categories for library books. */
enum class Genre {
    FICTION,
    NON_FICTION,
    SCIENCE,
    HISTORY,
    BIOGRAPHY;

    /** Human-readable label for display. */
    fun label(): String = name.replace("_", " ").lowercase()
        .replaceFirstChar { it.uppercase() }
}
