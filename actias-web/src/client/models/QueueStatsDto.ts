/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type QueueStatsDto = {
    /**
     * Every message still queued.
     */
    depth: number;
    /**
     * Messages due now, in delivery.
     */
    inFlight: number;
    oldestPending?: number | null;
    deadLetters: number;
};

