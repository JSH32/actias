/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { QueueEventDto } from '../models/QueueEventDto';
import type { QueueMessageDto } from '../models/QueueMessageDto';
import type { QueueStatsDto } from '../models/QueueStatsDto';
import type { ResourceInstanceDto } from '../models/ResourceInstanceDto';
import type { RetriedDto } from '../models/RetriedDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class QueuesService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * @param project
     * @returns ResourceInstanceDto
     * @throws ApiError
     */
    public listQueues(
        project: string,
    ): CancelablePromise<Array<ResourceInstanceDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/queues',
            path: {
                'project': project,
            },
        });
    }

    /**
     * @param project
     * @param name
     * @returns QueueStatsDto
     * @throws ApiError
     */
    public queueStats(
        project: string,
        name: string,
    ): CancelablePromise<QueueStatsDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/queues/{name}/stats',
            path: {
                'project': project,
                'name': name,
            },
        });
    }

    /**
     * Live and dead message rows, newest first; delivered messages are in
     * the journal.
     * @param project
     * @param name
     * @returns QueueMessageDto
     * @throws ApiError
     */
    public queueMessages(
        project: string,
        name: string,
    ): CancelablePromise<Array<QueueMessageDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/queues/{name}/messages',
            path: {
                'project': project,
                'name': name,
            },
        });
    }

    /**
     * The queue's journal after `since`: enqueued, delivered, retried and
     * dead-lettered, oldest first.
     * @param project
     * @param name
     * @param since
     * @returns QueueEventDto
     * @throws ApiError
     */
    public queueEvents(
        project: string,
        name: string,
        since?: number,
    ): CancelablePromise<Array<QueueEventDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/queues/{name}/events',
            path: {
                'project': project,
                'name': name,
            },
            query: {
                'since': since,
            },
        });
    }

    /**
     * Requeues every dead letter; they start their attempts over.
     * @param project
     * @param name
     * @returns RetriedDto
     * @throws ApiError
     */
    public retryDead(
        project: string,
        name: string,
    ): CancelablePromise<RetriedDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/queues/{name}/retry-dead',
            path: {
                'project': project,
                'name': name,
            },
        });
    }

    /**
     * Requeues one dead letter by id.
     * @param project
     * @param name
     * @param id
     * @returns RetriedDto
     * @throws ApiError
     */
    public retryMessage(
        project: string,
        name: string,
        id: string,
    ): CancelablePromise<RetriedDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/queues/{name}/messages/{id}/retry',
            path: {
                'project': project,
                'name': name,
                'id': id,
            },
        });
    }

    /**
     * Discards one message, live or dead.
     * @param project
     * @param name
     * @param id
     * @returns any
     * @throws ApiError
     */
    public dropMessage(
        project: string,
        name: string,
        id: string,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/queues/{name}/messages/{id}/drop',
            path: {
                'project': project,
                'name': name,
                'id': id,
            },
        });
    }

}
