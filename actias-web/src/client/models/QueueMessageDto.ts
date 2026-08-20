/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type QueueMessageDto = {
    id: number;
    /**
     * pending, in-flight or dead.
     */
    state: string;
    attempts: number;
    /**
     * Payload prefix.
     */
    preview: string;
    /**
     * Payload size in bytes.
     */
    size: number;
    enqueuedMs: number;
    nextMs?: number | null;
    diedMs?: number | null;
};

