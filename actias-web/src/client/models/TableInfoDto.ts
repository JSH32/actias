/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { ColumnInfoDto } from './ColumnInfoDto';

export type TableInfoDto = {
    name: string;
    rows: number;
    columns: Array<ColumnInfoDto>;
};

