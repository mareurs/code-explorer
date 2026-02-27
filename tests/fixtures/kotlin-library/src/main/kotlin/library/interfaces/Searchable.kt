package library.interfaces

/** Interface for anything that can be searched in the catalog. */
interface Searchable {
    /** Return a search-friendly text representation. */
    fun searchText(): String

    /** Default relevance score — override for custom ranking. */
    fun relevance(): Double = 0.0
}
