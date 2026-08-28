/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ObjectInstanceDto = {
    /**
     * The object class.
     */
    class: string;
    /**
     * The instance name.
     */
    name: string;
    /**
     * Public identifier of the script whose code it runs.
     */
    declaredBy: string;
    /**
     * Unix ms of the first claim.
     */
    createdMs: number;
    /**
     * When the platform deletes it if untouched; 0 = never.
     */
    expireAtMs: number;
    /**
     * Tombstone time; nonzero is a deletion in progress the janitor is finishing.
     */
    deletedAtMs: number;
    /**
     * The pending alarm's due time; 0 = none.
     */
    alarmDueMs: number;
    /**
     * The lease holder; empty = cold, next touch revives.
     */
    nodeId: string;
};

