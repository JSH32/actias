/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { PairType } from './PairType';

export type PairDto = {
    type: PairType;
    projectId: string;
    namespace: string;
    ttl: number;
    key: string;
    value: string;
};

