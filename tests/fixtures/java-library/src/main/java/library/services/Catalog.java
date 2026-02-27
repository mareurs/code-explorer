package library.services;

import library.interfaces.Searchable;
import library.models.Book;
import library.models.Genre;

import java.util.ArrayList;
import java.util.List;

/** A catalog that holds searchable items. */
public class Catalog<T extends Searchable> {

    private final List<T> items = new ArrayList<>();
    private final String name;

    /** Static nested class: statistics about the catalog. */
    public static class CatalogStats {
        public final int totalItems;
        public final String name;

        public CatalogStats(int totalItems, String name) {
            this.totalItems = totalItems;
            this.name = name;
        }
    }

    public Catalog(String name) {
        this.name = name;
    }

    /** Add an item to the catalog. */
    public void add(T item) {
        items.add(item);
    }

    /** Search for items matching a query. */
    public List<T> search(String query) {
        return items.stream()
            .filter(item -> item.searchText().contains(query))
            .toList();
    }

    /** Get catalog statistics. */
    public CatalogStats stats() {
        return new CatalogStats(items.size(), name);
    }

    /** Static factory: create a default catalog. */
    public static Catalog<Book> createDefault() {
        return new Catalog<>("Main Library");
    }
}
