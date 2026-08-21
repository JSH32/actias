/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type SecretDto = {
    name: string;
    /**
     * Head version; every rotation increments it.
     */
    version: number;
    /**
     * Unix milliseconds of the head version write.
     */
    createdMs: number;
    /**
     * User id that wrote the head version.
     */
    createdBy: string;
};

