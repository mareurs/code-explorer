package library.extensions

import library.models.Book

/** Extension: inline/value class. */
@JvmInline
value class ISBN(val value: String)

/** Extension: delegated property. */
class LazyBook(title: String) {
    val formattedTitle: String by lazy {
        title.uppercase()
    }
}

/** Extension: scope functions with receiver. */
fun createBookWithDefaults(): Book =
    Book(
        title = "Default",
        isbn = "000-0",
        genre = library.models.Genre.FICTION
    ).let { book ->
        // Using scope function
        book.copy(copiesAvailable = 5)
    }
