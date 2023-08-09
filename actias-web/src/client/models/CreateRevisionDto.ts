/* istanbul ignore file */
/* tslint:disable */

import type { BundleDto } from './BundleDto';

export type CreateRevisionDto = {
    /**
     * The bundle which will be used.
     */
    bundle: BundleDto;
    /**
     * A valid project configuration.
     */
    projectConfig: any;
};

