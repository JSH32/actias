/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { VisitEntryDto } from './VisitEntryDto';

export type VisitPageDto = {
    entries: Array<VisitEntryDto>;
    /**
     * Feeds the next page; absent on the last.
     */
    cursor?: string;
    /**
     * Fields still building; a query naming one is refused.
     */
    building: Array<string>;
};

