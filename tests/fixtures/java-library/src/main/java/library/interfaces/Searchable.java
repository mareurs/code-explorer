package library.interfaces;

/** Interface for anything that can be searched in the catalog. */
public interface Searchable {
    /** Return a search-friendly text representation. */
    String searchText();

    /** Extension: default method — override for custom ranking. */
    default double relevance() {
        return 0.0;
    }
}
