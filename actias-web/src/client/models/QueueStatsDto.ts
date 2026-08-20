/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type QueueStatsDto = {
    depth: number;
    oldestPending?: number | null;
    deadLetters: number;
};

