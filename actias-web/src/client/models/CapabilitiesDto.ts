/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type CapabilitiesDto = {
    /**
     * Namespaces declared with `kv "name"`.
     */
    kv: Array<string>;
    /**
     * Events declared with `on "event"`.
     */
    events: Array<string>;
    /**
     * Secrets declared with `secret "name"`.
     */
    secrets: Array<string>;
};

