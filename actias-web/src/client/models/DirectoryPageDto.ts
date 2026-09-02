/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { DirectoryEntryDto } from './DirectoryEntryDto';

export type DirectoryPageDto = {
    entries: Array<DirectoryEntryDto>;
    /**
     * Feeds the next page; absent on the last.
     */
    cursor?: string;
    /**
     * Fields the class has seen but not finished backfilling. A query naming one is refused; this reports the rest so progress is visible rather than a column mysteriously missing.
     */
    building: Array<string>;
};

