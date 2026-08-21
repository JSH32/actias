/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type SecretVersionDto = {
    version: number;
    /**
     * Unix milliseconds this version was written.
     */
    createdMs: number;
    /**
     * User id that wrote it.
     */
    createdBy: string;
    /**
     * Username behind createdBy, when the account still exists.
     */
    createdByName: string;
    /**
     * Unix milliseconds this version was tombstoned by a delete; 0 while live.
     */
    deletedMs: number;
};

