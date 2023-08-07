/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { BundleDto } from './BundleDto';

export type CreateRevisionDto = {
    /**
     * The bundle which will be used.
     */
    bundle: BundleDto;
    /**
     * A valid project configuration.
     */
    projectConfig: Record<string, any>;
};

