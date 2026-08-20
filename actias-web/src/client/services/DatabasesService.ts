/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DatabaseOverviewDto } from '../models/DatabaseOverviewDto';
import type { ResourceInstanceDto } from '../models/ResourceInstanceDto';
import type { SqlQueryDto } from '../models/SqlQueryDto';
import type { SqlRowsDto } from '../models/SqlRowsDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class DatabasesService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

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
            url: '/api/project/{project}/databases',
            path: {
                'project': project,
            },
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
            url: '/api/project/{project}/databases/{name}/overview',
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
            url: '/api/project/{project}/databases/{name}/query',
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
            url: '/api/project/{project}/databases/{name}/execute',
            path: {
                'project': project,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
