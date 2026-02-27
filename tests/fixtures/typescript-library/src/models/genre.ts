/** Genre categories for library books. */
export enum Genre {
    Fiction = 'fiction',
    NonFiction = 'non_fiction',
    Science = 'science',
    History = 'history',
    Biography = 'biography',
}

/** Human-readable label for a genre. */
export function genreLabel(genre: Genre): string {
    return genre.replace('_', ' ');
}
