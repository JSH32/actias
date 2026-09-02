/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { DirectoryConditionDto } from './DirectoryConditionDto';

export type DirectoryWhereDto = {
    conditions?: Array<DirectoryConditionDto>;
    /**
     * OR over sub-wheres.
     */
    any?: Array<DirectoryWhereDto>;
    /**
     * Explicit AND, for grouping inside an any.
     */
    all?: Array<DirectoryWhereDto>;
    /**
     * NOT over sub-wheres.
     */
    none?: Array<DirectoryWhereDto>;
};

