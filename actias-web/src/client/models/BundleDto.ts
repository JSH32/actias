/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { FileDto } from './FileDto';

export type BundleDto = {
    /**
     * Path of the entrypoint file.
     * This is the first file which is executed by the runtime.
     */
    entryPoint: string;
    /**
     * All files within the bundle.
     */
    files: Array<FileDto>;
};

