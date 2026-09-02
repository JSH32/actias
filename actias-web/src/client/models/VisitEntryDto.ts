/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { DirectoryEntryDto } from './DirectoryEntryDto';

export type VisitEntryDto = {
    entry: DirectoryEntryDto;
    /**
     * The row could not be checked against its object and is served anyway, saying so: dropping it would invent the false negative the directory refuses.
     */
    unverified: boolean;
    /**
     * Why, when unverified.
     */
    reason?: string;
};

