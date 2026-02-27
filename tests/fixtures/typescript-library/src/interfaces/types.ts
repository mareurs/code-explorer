import { Book } from '../models/book';

/** Extension: union type. */
export type SearchResult = FoundResult | NotFoundResult | ErrorResult;

export interface FoundResult {
    kind: 'found';
    book: Book;
    score: number;
}

export interface NotFoundResult {
    kind: 'not_found';
    query: string;
}

export interface ErrorResult {
    kind: 'error';
    message: string;
    code: number;
}

/** Extension: type guard function. */
export function isFound(result: SearchResult): result is FoundResult {
    return result.kind === 'found';
}

/** Extension: mapped type. */
export type ReadonlyBook = Readonly<Pick<Book, 'title' | 'isbn'>>;

/** Extension: conditional type. */
export type IsAvailable<T> = T extends { isAvailable(): boolean } ? true : false;

/** Extension: index signature. */
export interface BookIndex {
    [isbn: string]: Book;
}
