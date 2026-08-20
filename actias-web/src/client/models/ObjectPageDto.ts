/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { ObjectInstanceDto } from './ObjectInstanceDto';

export type ObjectPageDto = {
    items: Array<ObjectInstanceDto>;
    /**
     * Instances matching the filter across every page.
     */
    total: number;
};

