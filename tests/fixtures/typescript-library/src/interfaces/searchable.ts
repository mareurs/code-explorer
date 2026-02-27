/** Interface for anything that can be searched in the catalog. */
export interface Searchable {
    /** Return a search-friendly text representation. */
    searchText(): string;

    /** Optional relevance score — default is 0. */
    relevance?(): number;
}
