package library.extensions;

import library.models.Book;

/** Extension: sealed interface hierarchy. */
public sealed interface SearchResult permits SearchResult.Found, SearchResult.NotFound, SearchResult.Error {

    /** Found a match with a relevance score. */
    record Found(Book book, double score) implements SearchResult {}

    /** No results for the query. */
    record NotFound(String query) implements SearchResult {}

    /** Search error with message. */
    record Error(String message, int code) implements SearchResult {}

    /** Check if this result contains a match — extension: pattern matching. */
    default boolean isMatch() {
        return this instanceof Found;
    }
}
