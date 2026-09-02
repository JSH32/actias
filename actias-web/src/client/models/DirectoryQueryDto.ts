/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { DirectoryOrderDto } from './DirectoryOrderDto';
import type { DirectoryWhereDto } from './DirectoryWhereDto';

export type DirectoryQueryDto = {
    where?: DirectoryWhereDto;
    order?: Array<DirectoryOrderDto>;
    /**
     * Rows this page may carry; the worker caps it.
     */
    limit?: number;
    /**
     * From a previous page.
     */
    cursor?: string;
};

