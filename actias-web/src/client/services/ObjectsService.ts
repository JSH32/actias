/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ClassCountDto } from '../models/ClassCountDto';
import type { DatabaseOverviewDto } from '../models/DatabaseOverviewDto';
import type { FollowersDto } from '../models/FollowersDto';
import type { ObjectPageDto } from '../models/ObjectPageDto';
import type { SqlQueryDto } from '../models/SqlQueryDto';
import type { SqlRowsDto } from '../models/SqlRowsDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ObjectsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Instances the directory knows, filterable by class and name
     * prefix, always paged, because a per-user class holds one instance
     * per user.
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
            url: '/api/project/{project}/objects',
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
            url: '/api/project/{project}/objects/counts',
            path: {
                'project': project,
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
            url: '/api/project/{project}/objects/{class}/{name}/overview',
            path: {
                'project': project,
                'class': _class,
                'name': name,
            },
        });
    }

    /**
     * The edges other things hold on this object: who follows it, on
     * which topic, with what filter, and how far behind the publisher's
     * event log each durable edge sits. Runtime state, never contract.
     * @param project
     * @param _class
     * @param name
     * @returns FollowersDto
     * @throws ApiError
     */
    public objectFollowers(
        project: string,
        _class: string,
        name: string,
    ): CancelablePromise<FollowersDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/objects/{class}/{name}/followers',
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
            url: '/api/project/{project}/objects/{class}/{name}/query',
            path: {
                'project': project,
                'class': _class,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
