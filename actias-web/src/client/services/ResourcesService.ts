/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ClassCountDto } from '../models/ClassCountDto';
import type { DatabaseOverviewDto } from '../models/DatabaseOverviewDto';
import type { ObjectPageDto } from '../models/ObjectPageDto';
import type { QueueEventDto } from '../models/QueueEventDto';
import type { QueueMessageDto } from '../models/QueueMessageDto';
import type { QueueStatsDto } from '../models/QueueStatsDto';
import type { ResourceInstanceDto } from '../models/ResourceInstanceDto';
import type { RetriedDto } from '../models/RetriedDto';
import type { SqlQueryDto } from '../models/SqlQueryDto';
import type { SqlRowsDto } from '../models/SqlRowsDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ResourcesService {

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
            url: '/api/project/{project}/resources/queues',
            path: {
                'project': project,
            },
        });
    }

    /**
     * @param project
     * @returns ResourceInstanceDto
     * @throws ApiError
     */
    public listDatabases(
        project: string,
    ): CancelablePromise<Array<ResourceInstanceDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/databases',
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
            url: '/api/project/{project}/resources/queues/{name}/stats',
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
            url: '/api/project/{project}/resources/queues/{name}/messages',
            path: {
                'project': project,
                'name': name,
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
            url: '/api/project/{project}/resources/queues/{name}/retry-dead',
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
            url: '/api/project/{project}/resources/queues/{name}/messages/{id}/retry',
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
            url: '/api/project/{project}/resources/queues/{name}/messages/{id}/drop',
            path: {
                'project': project,
                'name': name,
                'id': id,
            },
        });
    }

    /**
     * Durable object instances the directory knows, user classes only;
     * filterable by class and name prefix, always paged, because a
     * per-user class holds one instance per user.
     * @param project
     * @param _class
     * @param prefix
     * @param page
     * @param pageSize
     * @returns ObjectPageDto
     * @throws ApiError
     */
    public listObjects(
        project: string,
        _class?: string,
        prefix?: string,
        page?: number,
        pageSize?: number,
    ): CancelablePromise<ObjectPageDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/objects',
            path: {
                'project': project,
            },
            query: {
                'class': _class,
                'prefix': prefix,
                'page': page,
                'pageSize': pageSize,
            },
        });
    }

    /**
     * How many instances each user class holds: what the rail renders
     * before anyone asks for names.
     * @param project
     * @returns ClassCountDto
     * @throws ApiError
     */
    public countObjects(
        project: string,
    ): CancelablePromise<Array<ClassCountDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/objects/counts',
            path: {
                'project': project,
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
            url: '/api/project/{project}/resources/queues/{name}/events',
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
     * Overview of one durable object's private storage; a user class's
     * file is a SQLite database like any other.
     * @param project
     * @param _class
     * @param name
     * @returns DatabaseOverviewDto
     * @throws ApiError
     */
    public objectOverview(
        project: string,
        _class: string,
        name: string,
    ): CancelablePromise<DatabaseOverviewDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/objects/{class}/{name}/overview',
            path: {
                'project': project,
                'class': _class,
                'name': name,
            },
        });
    }

    /**
     * A read-only query against one object's storage, from the nearest
     * copy; the script-guard authorizer applies, so reserved tables stay
     * out of reach. Writes only ever happen through the object's methods.
     * @param project
     * @param _class
     * @param name
     * @param requestBody
     * @returns SqlRowsDto
     * @throws ApiError
     */
    public objectQuery(
        project: string,
        _class: string,
        name: string,
        requestBody: SqlQueryDto,
    ): CancelablePromise<SqlRowsDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/resources/objects/{class}/{name}/query',
            path: {
                'project': project,
                'class': _class,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * @param project
     * @param name
     * @returns DatabaseOverviewDto
     * @throws ApiError
     */
    public databaseOverview(
        project: string,
        name: string,
    ): CancelablePromise<DatabaseOverviewDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/databases/{name}/overview',
            path: {
                'project': project,
                'name': name,
            },
        });
    }

    /**
     * Runs a read-only query from the nearest copy, bounded staleness by
     * design; the script-guard authorizer applies exactly as it does to
     * script sql.
     * @param project
     * @param name
     * @param requestBody
     * @returns SqlRowsDto
     * @throws ApiError
     */
    public query(
        project: string,
        name: string,
        requestBody: SqlQueryDto,
    ): CancelablePromise<SqlRowsDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/resources/databases/{name}/query',
            path: {
                'project': project,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Executes a statement through the owner, transactional, single-writer.
     * @param project
     * @param name
     * @param requestBody
     * @returns SqlRowsDto
     * @throws ApiError
     */
    public execute(
        project: string,
        name: string,
        requestBody: SqlQueryDto,
    ): CancelablePromise<SqlRowsDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/resources/databases/{name}/execute',
            path: {
                'project': project,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
