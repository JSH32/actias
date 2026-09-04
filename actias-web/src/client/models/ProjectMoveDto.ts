/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ProjectMoveDto = {
    fromRegion: string;
    toRegion: string;
    /**
     * marking, draining, copying, flipping, done, failed; empty when the project never moved.
     */
    step: string;
    objectsTotal: number;
    objectsCopied: number;
    /**
     * Set when the step is failed; the move may be started again.
     */
    error: string;
    /**
     * Unix milliseconds; 0 when the project never moved.
     */
    startedAt: number;
    /**
     * Unix milliseconds; 0 while the move runs.
     */
    finishedAt: number;
};

