/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { QueueStatsDto } from '../models/QueueStatsDto';
import type { ResourceInstanceDto } from '../models/ResourceInstanceDto';
import type { SqlQueryDto } from '../models/SqlQueryDto';
import type { SqlRowsDto } from '../models/SqlRowsDto';
import type { TableInfoDto } from '../models/TableInfoDto';

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
     * @param script
     * @param name
     * @returns QueueStatsDto
     * @throws ApiError
     */
    public queueStats(
        project: string,
        script: string,
        name: string,
    ): CancelablePromise<QueueStatsDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/queues/{script}/{name}/stats',
            path: {
                'project': project,
                'script': script,
                'name': name,
            },
        });
    }

    /**
     * @param project
     * @param script
     * @param name
     * @returns TableInfoDto
     * @throws ApiError
     */
    public databaseTables(
        project: string,
        script: string,
        name: string,
    ): CancelablePromise<Array<TableInfoDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/resources/databases/{script}/{name}/tables',
            path: {
                'project': project,
                'script': script,
                'name': name,
            },
        });
    }

    /**
     * Runs a read-only query from the nearest copy, bounded staleness by
     * design; the script-guard authorizer applies exactly as it does to
     * script sql.
     * @param project
     * @param script
     * @param name
     * @param requestBody
     * @returns SqlRowsDto
     * @throws ApiError
     */
    public query(
        project: string,
        script: string,
        name: string,
        requestBody: SqlQueryDto,
    ): CancelablePromise<SqlRowsDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/resources/databases/{script}/{name}/query',
            path: {
                'project': project,
                'script': script,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Executes a statement through the owner, transactional, single-writer.
     * @param project
     * @param script
     * @param name
     * @param requestBody
     * @returns SqlRowsDto
     * @throws ApiError
     */
    public execute(
        project: string,
        script: string,
        name: string,
        requestBody: SqlQueryDto,
    ): CancelablePromise<SqlRowsDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/resources/databases/{script}/{name}/execute',
            path: {
                'project': project,
                'script': script,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
