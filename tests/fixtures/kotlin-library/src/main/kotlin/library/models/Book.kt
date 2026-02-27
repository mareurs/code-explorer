package library.models

/** Maximum number of search results to return. */
const val MAX_RESULTS: Int = 100

/** A book in the library catalog. */
data class Book(
    val title: String,
    val isbn: String,
    val genre: Genre,
    val copiesAvailable: Int = 1
) {
    /** Check if the book is available for borrowing. */
    fun isAvailable(): Boolean = copiesAvailable > 0

    /** Extension: companion object with factory methods. */
    companion object {
        fun create(title: String, isbn: String): Book =
            Book(title, isbn, Genre.FICTION)

        fun fromJson(json: String): Book =
            Book("Parsed", "000-0", Genre.FICTION)
    }
}
