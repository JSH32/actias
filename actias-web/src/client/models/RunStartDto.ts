/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type RunStartDto = {
    payload?: any;
    /**
     * The run id; the idempotency key. Omitted, the console mints one.
     */
    id?: string;
};

