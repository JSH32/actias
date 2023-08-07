/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { BundleDto } from './BundleDto';
import type { ProjectConfigDto } from './ProjectConfigDto';

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
    projectConfig: ProjectConfigDto;
    /**
     * Content bundle of all files.
     * This is only present in some responses.
     */
    bundle?: BundleDto;
};

