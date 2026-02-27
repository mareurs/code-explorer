package library.models;

/** A book in the library catalog. */
public record Book(
    String title,
    String isbn,
    Genre genre,
    int copiesAvailable
) {
    /** Maximum number of search results to return. */
    public static final int MAX_RESULTS = 100;

    /** Compact constructor with default copies. */
    public Book(String title, String isbn, Genre genre) {
        this(title, isbn, genre, 1);
    }

    /** Check if the book is available for borrowing. */
    public boolean isAvailable() {
        return copiesAvailable > 0;
    }
}
