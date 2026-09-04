/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ProjectPolicyViewDto = {
    /**
     * The project's home region: where its named objects are born and its
     * directory lives.
     */
    region: string;
    /**
     * Whether the project is between homes; calls are refused, retryably, until it settles.
     */
    moving: boolean;
    /**
     * Requests a node admits for the project per second, burst of the
     * same size; 0 is unbounded.
     */
    requestsPerSec: number;
    /**
     * Work units a node lets the project spend per second; 0 is unbounded.
     */
    workUnitsPerSec: number;
    /**
     * Hosts outbound requests and dials may reach; a leading dot matches
     * subdomains. Empty admits everything not denied.
     */
    egressAllow: Array<string>;
    /**
     * Hosts refused before the allow list is consulted.
     */
    egressDeny: Array<string>;
};

