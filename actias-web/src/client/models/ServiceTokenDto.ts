/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ServiceTokenDto = {
    id: string;
    name: string;
    /**
     * First characters of the secret, to match a held token to its row.
     */
    tokenPrefix: string;
    /**
     * Access field names to whether the token holds them.
     */
    access: any;
    createdAt: string;
    lastUsed?: string;
};

