/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type FollowerEdgeDto = {
    /**
     * 'object' (durable) or 'connection'.
     */
    kind: string;
    /**
     * The follower's identity, 'Class/name'.
     */
    follower: string;
    /**
     * Connection edges only: the endpoint connection id.
     */
    connection?: string | null;
    /**
     * The topic this edge listens on.
     */
    topic: string;
    /**
     * Equality filter on event data fields, when set.
     */
    filter?: any;
    /**
     * Last event sequence this edge passed.
     */
    cursor: number;
    /**
     * Undelivered events behind the log head; durable edges only.
     */
    lag?: number | null;
    /**
     * Consecutive failed deliveries so far.
     */
    attempts: number;
    /**
     * When delivery retries next (unix ms); 0 when not due.
     */
    nextAt: number;
};

