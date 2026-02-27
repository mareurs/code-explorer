import { Book } from '../models/book';

/** Extension: function overload signatures. */
export function findBook(isbn: string): Book | undefined;
export function findBook(title: string, author: string): Book[];
export function findBook(first: string, second?: string): Book | Book[] | undefined {
    return undefined;
}

/** Extension: decorator (experimental). */
function logged(target: any, propertyKey: string, descriptor: PropertyDescriptor) {
    return descriptor;
}

export class BookService {
    @logged
    process(book: Book): void {
        // decorated method
    }
}

/** Extension: namespace merging (declaration merging). */
export interface BookMetadata {
    title: string;
    pages: number;
}

export namespace BookMetadata {
    export function create(title: string, pages: number): BookMetadata {
        return { title, pages };
    }
}

/** Extension: default export. */
export default class DefaultCatalog {
    readonly name = 'default';
}
