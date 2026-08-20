/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { TableInfoDto } from './TableInfoDto';

export type DatabaseOverviewDto = {
    /**
     * Database file size in bytes.
     */
    sizeBytes: number;
    tables: Array<TableInfoDto>;
};

