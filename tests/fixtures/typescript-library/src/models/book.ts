import { Genre } from './genre';

/** Maximum number of search results to return. */
export const MAX_RESULTS = 100;

/** A book in the library catalog. */
export class Book {
    constructor(
        private _title: string,
        private _isbn: string,
        private _genre: Genre,
        private _copiesAvailable: number = 1
    ) {}

    /** Get the book title. */
    title(): string {
        return this._title;
    }

    /** Get the ISBN. */
    isbn(): string {
        return this._isbn;
    }

    /** Check if the book is available for borrowing. */
    isAvailable(): boolean {
        return this._copiesAvailable > 0;
    }

    /** Get the genre. */
    genre(): Genre {
        return this._genre;
    }
}
