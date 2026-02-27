package library.models;

/** Genre categories for library books. */
public enum Genre {
    FICTION,
    NON_FICTION,
    SCIENCE,
    HISTORY,
    BIOGRAPHY;

    /** Human-readable label for display. */
    public String label() {
        return name().replace("_", " ").substring(0, 1)
            + name().replace("_", " ").substring(1).toLowerCase();
    }
}
