/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ProjectPolicyDto = {
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

