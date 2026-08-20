/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type QueueEventDto = {
    seq: number;
    at: number;
    kind: string;
    /**
     * Structured event detail: message id, payload preview, producer, per-attempt error.
     */
    detail: any;
};

