/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { CapabilitiesDto } from './CapabilitiesDto';

export type ScriptConfigDto = {
    id: string;
    entryPoint: string;
    ignore: Array<string>;
    includes: Array<string>;
    /**
     * Derived from the code at publish, never hand-written.
     */
    capabilities?: CapabilitiesDto;
};

