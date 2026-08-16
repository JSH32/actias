/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { BundleDto } from './BundleDto';
import type { ScriptConfigDto } from './ScriptConfigDto';

export type CreateRevisionDto = {
    /**
     * The bundle which will be used.
     */
    bundle: BundleDto;
    /**
     * A valid project configuration.
     */
    scriptConfig: ScriptConfigDto;
};

