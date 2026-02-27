package library.extensions

import library.models.Book

/** Extension: sealed class with data class, object, and class subclasses. */
sealed class SearchResult {
    /** Found a match with a relevance score. */
    data class Found(val book: Book, val score: Double) : SearchResult()

    /** No results for the query. */
    object NotFound : SearchResult()

    /** Search error with message. */
    data class Error(val message: String, val code: Int) : SearchResult()

    /** Check if this result contains a match. */
    fun isMatch(): Boolean = this is Found
}

/** Extension: object declaration (singleton). */
object BookRegistry {
    private val books = mutableMapOf<String, Book>()

    fun register(book: Book) {
        books[book.isbn] = book
    }

    fun lookup(isbn: String): Book? = books[isbn]
}
