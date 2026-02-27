package library.extensions;

import library.interfaces.Searchable;
import library.models.Book;

import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.util.List;

/** Extension: custom annotation. */
@Retention(RetentionPolicy.RUNTIME)
public @interface Indexed {
    String value() default "";
}

/** Extension: class with annotations, anonymous class, and wildcards. */
class BookProcessor {

    @Indexed("isbn")
    public void process(Book book) {
        // annotated method
    }

    /** Extension: anonymous class implementing an interface. */
    public Searchable createAnonymousSearchable() {
        return new Searchable() {
            @Override
            public String searchText() {
                return "anonymous";
            }
        };
    }

    /** Extension: generics with wildcards. */
    public void processAll(List<? extends Searchable> items) {
        for (Searchable item : items) {
            item.searchText();
        }
    }

    /** Extension: static inner class vs non-static. */
    static class BatchResult {
        int processed;
        int failed;
    }

    class ProcessingContext {
        String currentBook;
    }
}
