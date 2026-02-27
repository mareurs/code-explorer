import { Searchable } from '../interfaces/searchable';

/** Statistics about the catalog. */
export class CatalogStats {
    constructor(
        public totalItems: number,
        public name: string
    ) {}
}

/** A catalog that holds searchable items. */
export class Catalog<T extends Searchable> {
    private items: T[] = [];

    constructor(private name: string) {}

    /** Add an item to the catalog. */
    add(item: T): void {
        this.items.push(item);
    }

    /** Search for items matching a query. */
    search(query: string): T[] {
        return this.items.filter(item => item.searchText().includes(query));
    }

    /** Get catalog statistics. */
    stats(): CatalogStats {
        return new CatalogStats(this.items.length, this.name);
    }
}

/** Free function: create a default catalog. */
export function createDefaultCatalog(): Catalog<any> {
    return new Catalog('Main Library');
}
