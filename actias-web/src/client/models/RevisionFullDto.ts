/* istanbul ignore file */
/* tslint:disable */

import type { BundleDto } from './BundleDto';
import type { ScriptConfigDto } from './ScriptConfigDto';

export type RevisionFullDto = {
    id: string;
    /**
     * Date that revision was published.
     */
    created: string;
    /**
     * ID of the script this revision is attached to.
     */
    scriptId: string;
    /**
     * Config that the project was uploaded with.
     * This is metadata and is mostly included for CLI to restore revisions intact.
     */
    scriptConfig: ScriptConfigDto;
    /**
     * Content bundle of all files.
     * This is only present in some responses.
     */
    bundle?: BundleDto;
};

