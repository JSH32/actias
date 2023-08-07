/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateRevisionDto } from '../models/CreateRevisionDto';
import type { CreateScriptDto } from '../models/CreateScriptDto';
import type { NewRevisionResponseDto } from '../models/NewRevisionResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
import type { RevisionDataDto } from '../models/RevisionDataDto';
import type { ScriptDto } from '../models/ScriptDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ScriptsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Get a list of revisions (bundle not included).
     * @param id
     * @param page
     * @returns any
     * @throws ApiError
     */
    public scriptsControllerRevisionList(
        id: string,
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/script/{id}/revisions',
            path: {
                'id': id,
            },
            query: {
                'page': page,
            },
        });
    }

    /**
     * Set the current active revision for a script.
     * @param id
     * @param revisionId
     * @returns NewRevisionResponseDto
     * @throws ApiError
     */
    public scriptsControllerSetRevision(
        id: string,
        revisionId: string,
    ): CancelablePromise<NewRevisionResponseDto> {
        return this.httpRequest.request({
            method: 'PATCH',
            url: '/script/{id}/revisions',
            path: {
                'id': id,
            },
            query: {
                'revisionId': revisionId,
            },
        });
    }

    /**
     * Create a new revision.
     * @param id
     * @param requestBody
     * @returns RevisionDataDto
     * @throws ApiError
     */
    public scriptsControllerCreateRevision(
        id: string,
        requestBody: CreateRevisionDto,
    ): CancelablePromise<RevisionDataDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/script/{id}/revisions',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Get a script by ID.
     * @param id
     * @returns ScriptDto
     * @throws ApiError
     */
    public scriptsControllerGetScript(
        id: string,
    ): CancelablePromise<ScriptDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/script/{id}',
            path: {
                'id': id,
            },
        });
    }

    /**
     * @param id
     * @returns any
     * @throws ApiError
     */
    public scriptsControllerDeleteScript(
        id: string,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/script/{id}',
            path: {
                'id': id,
            },
        });
    }

    /**
     * Get a paginated list of scripts.
     * @param project
     * @param page
     * @returns any
     * @throws ApiError
     */
    public projectScriptControllerListScripts(
        project: string,
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/project/{project}/scripts',
            path: {
                'project': project,
            },
            query: {
                'page': page,
            },
        });
    }

    /**
     * @param project
     * @param requestBody
     * @returns ScriptDto
     * @throws ApiError
     */
    public projectScriptControllerCreateScript(
        project: string,
        requestBody: CreateScriptDto,
    ): CancelablePromise<ScriptDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/project/{project}/scripts',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
