package library.services

import library.interfaces.Searchable
import library.models.Book

/** A catalog that holds searchable items. */
class Catalog<T : Searchable>(private val name: String) {

    private val items = mutableListOf<T>()

    /** Nested class: statistics about the catalog. */
    data class CatalogStats(val totalItems: Int, val name: String)

    /** Add an item to the catalog. */
    fun add(item: T) {
        items.add(item)
    }

    /** Search for items matching a query. */
    fun search(query: String): List<T> =
        items.filter { it.searchText().contains(query) }

    /** Get catalog statistics. */
    fun stats(): CatalogStats = CatalogStats(items.size, name)
}

/** Free function: create a default catalog for books. */
fun createDefaultCatalog(): Catalog<Book> = Catalog("Main Library")

/** Extension: suspend function (coroutine). */
suspend fun <T : Searchable> Catalog<T>.searchAsync(query: String): List<T> =
    search(query)

/** Extension: extension function on Book. */
fun Book.toSearchText(): String = "$title ($isbn)"
