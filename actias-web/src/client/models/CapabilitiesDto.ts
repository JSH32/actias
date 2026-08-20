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
    /**
     * Object classes declared with `object "Class" { ... }`.
     */
    objects: Array<string>;
    /**
     * Databases declared with `database "name"`.
     */
    databases: Array<string>;
    /**
     * Queues declared with `queue "name"`.
     */
    queues: Array<string>;
    /**
     * Workflow definitions declared with `workflow "name"`.
     */
    workflows: Array<string>;
    /**
     * Step literals found at publish: the declared-possible superset.
     */
    workflowSteps: Array<string>;
};

