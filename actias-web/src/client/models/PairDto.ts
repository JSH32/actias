/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { PairType } from './PairType';

export type PairDto = {
    type: PairType;
    ttl?: number;
    projectId: string;
    namespace: string;
    key: string;
    value: string;
};

