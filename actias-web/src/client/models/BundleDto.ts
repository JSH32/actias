/* istanbul ignore file */
/* tslint:disable */

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

