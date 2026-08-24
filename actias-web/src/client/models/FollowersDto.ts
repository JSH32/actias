/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { FollowerEdgeDto } from './FollowerEdgeDto';

export type FollowersDto = {
    /**
     * Newest event sequence in the publisher's log.
     */
    head: number;
    edges: Array<FollowerEdgeDto>;
};

