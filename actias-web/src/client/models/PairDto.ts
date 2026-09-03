/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { PairType } from './PairType';

export type PairDto = {
    /**
     * The stored value is always text; this names how to read it back.
     */
    type: PairType;
    /**
     * Seconds until the pair expires; absent for one that never does.
     * Declared optional so a generated client does not demand a field
     * the service omits.
     */
    ttl?: number;
    projectId: string;
    namespace: string;
    key: string;
    value: string;
};

