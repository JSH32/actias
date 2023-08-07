/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { ProjectConfigDto } from './ProjectConfigDto';

export type RevisionDataDto = {
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
};

