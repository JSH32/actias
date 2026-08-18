/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type CreatedServiceTokenDto = {
    /**
     * The full token. Store it now; it cannot be retrieved again.
     */
    token: string;
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

