/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type CreateServiceTokenDto = {
    /**
     * Access field names granted to the token (see the acl list shape).
     * Omitted, the token can manage scripts and kv but not permissions.
     */
    access?: Array<'SCRIPT_READ' | 'SCRIPT_WRITE' | 'SCRIPT_RESOURCE' | 'PERMISSIONS_READ' | 'PERMISSIONS_WRITE' | 'PERMISSIONS_RESOURCE' | 'KV_READ' | 'KV_WRITE' | 'KV_RESOURCE' | 'FULL'>;
    /**
     * Human label for the token list, e.g. "github deploy".
     */
    name: string;
};

