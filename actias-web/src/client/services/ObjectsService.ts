/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ClassCountDto } from '../models/ClassCountDto';
import type { DatabaseOverviewDto } from '../models/DatabaseOverviewDto';
import type { DeleteOutcomeDto } from '../models/DeleteOutcomeDto';
import type { DirectoryPageDto } from '../models/DirectoryPageDto';
import type { DirectoryQueryDto } from '../models/DirectoryQueryDto';
import type { DirectoryRebuiltDto } from '../models/DirectoryRebuiltDto';
import type { FollowersDto } from '../models/FollowersDto';
import type { ObjectCallDto } from '../models/ObjectCallDto';
import type { ObjectCallResultDto } from '../models/ObjectCallResultDto';
import type { ObjectPageDto } from '../models/ObjectPageDto';
import type { SqlQueryDto } from '../models/SqlQueryDto';
import type { SqlRowsDto } from '../models/SqlRowsDto';
import type { StateDto } from '../models/StateDto';
import type { VisitPageDto } from '../models/VisitPageDto';

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
     * Deletion is forget: storage, snapshot and edges are reclaimed,
     * the name may be recreated later and starts fresh, and there is no
     * undo. This tombstones; the janitor finishes within a sweep.
     * @param project
     * @param _class
     * @param name
     * @returns DeleteOutcomeDto
     * @throws ApiError
     */
    public deleteObject(
        project: string,
        _class: string,
        name: string,
    ): CancelablePromise<DeleteOutcomeDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/objects/{class}/{name}',
            path: {
                'project': project,
                'class': _class,
                'name': name,
            },
        });
    }

    /**
     * Every instance of one class, for dev cleanup; pages through the
     * directory and tombstones each row.
     * @param project
     * @param _class
     * @returns DeleteOutcomeDto
     * @throws ApiError
     */
    public deleteClass(
        project: string,
        _class: string,
    ): CancelablePromise<DeleteOutcomeDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/objects/{class}',
            path: {
                'project': project,
                'class': _class,
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
     * One method call on one instance, as a script would make it: through
     * the object's own lane, serialized with every other call, directory
     * derivation and alarms running as they always do. Never a side
     * channel into the file.
     *
     * This is the shell's write mode, and a person touching live data,
     * so every call is logged against the account that made it. Naming
     * an instance that does not exist creates it, admission permitting,
     * exactly as in a script.
     * @param project
     * @param _class
     * @param name
     * @param requestBody
     * @returns ObjectCallResultDto
     * @throws ApiError
     */
    public objectCall(
        project: string,
        _class: string,
        name: string,
        requestBody: ObjectCallDto,
    ): CancelablePromise<ObjectCallResultDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/objects/{class}/{name}/call',
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
     * The object's key-value state pairs, in key order. The reserved
     * table is denied to SQL from every direction, so this typed read is
     * the console's only window on the store face. Read-only: writes go
     * through the object's methods, like everything else it keeps.
     * @param project
     * @param _class
     * @param name
     * @returns StateDto
     * @throws ApiError
     */
    public objectState(
        project: string,
        _class: string,
        name: string,
    ): CancelablePromise<StateDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/objects/{class}/{name}/state',
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

    /**
     * One page of the class's directory: the row every object in it
     * contributes, answered without waking any of them.
     *
     * A POST because the predicate is a tree, not a query string. The
     * rows are each object's last saved write, so a listing decides
     * which objects to call and never substitutes for calling one.
     * @param project
     * @param _class
     * @param requestBody
     * @returns DirectoryPageDto
     * @throws ApiError
     */
    public objectDirectory(
        project: string,
        _class: string,
        requestBody: DirectoryQueryDto,
    ): CancelablePromise<DirectoryPageDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/objects/{class}/directory',
            path: {
                'project': project,
                'class': _class,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Rebuilds the class's index from what still exists: the placement
     * store's live identities, and each object's shipping manifest.
     *
     * The operator's path for damage the background pass cannot reach.
     * That pass finds classes by listing the blob store, so a class
     * whose prefix is gone entirely is invisible to it; a name can always
     * be asked for. Nothing is woken and no object file is opened, so
     * the cost is one small read per live object.
     * @param project
     * @param _class
     * @returns DirectoryRebuiltDto
     * @throws ApiError
     */
    public objectDirectoryRebuild(
        project: string,
        _class: string,
    ): CancelablePromise<DirectoryRebuiltDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/objects/{class}/directory/rebuild',
            path: {
                'project': project,
                'class': _class,
            },
        });
    }

    /**
     * The verified read over a class's directory. Same query as the
     * listing; every candidate is checked against its object's settled
     * state before it is served, so stale rows drop, fresher rows arrive
     * fresh, and the uncheckable come back flagged rather than missing.
     * @param project
     * @param _class
     * @param requestBody
     * @returns VisitPageDto
     * @throws ApiError
     */
    public objectDirectoryVisit(
        project: string,
        _class: string,
        requestBody: DirectoryQueryDto,
    ): CancelablePromise<VisitPageDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/objects/{class}/directory/visit',
            path: {
                'project': project,
                'class': _class,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
