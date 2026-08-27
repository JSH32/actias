/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type AdminProjectDto = {
    id: string;
    name: string;
    /**
     * The owning user's name; ownership means full access.
     */
    ownerUsername: string;
    createdAt: string;
};

