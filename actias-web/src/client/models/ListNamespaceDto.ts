/* istanbul ignore file */
/* tslint:disable */

import type { PairDto } from './PairDto';

export type ListNamespaceDto = {
    pageSize: number;
    /**
     * Token used to fetch next page.
     * Not provided on last page.
     */
    token?: string;
    pairs: Array<PairDto>;
};

