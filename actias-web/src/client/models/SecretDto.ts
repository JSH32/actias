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
    /**
     * Public identifier of the live script declaring this name; null is the orphan state, set but reachable by no live revision.
     */
    declaredBy: string | null;
};

