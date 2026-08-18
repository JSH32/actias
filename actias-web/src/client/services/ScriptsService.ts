/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AliasDto } from '../models/AliasDto';
import type { CreateRevisionDto } from '../models/CreateRevisionDto';
import type { CreateScriptDto } from '../models/CreateScriptDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { MissingBlobsDto } from '../models/MissingBlobsDto';
import type { MissingBlobsResponseDto } from '../models/MissingBlobsResponseDto';
import type { NewRevisionResponseDto } from '../models/NewRevisionResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
import type { RevisionDataDto } from '../models/RevisionDataDto';
import type { ScriptDto } from '../models/ScriptDto';
import type { SetAliasDto } from '../models/SetAliasDto';

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
    public revisionList(
        id: string,
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/script/{id}/revisions',
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
    public setRevision(
        id: string,
        revisionId: string,
    ): CancelablePromise<NewRevisionResponseDto> {
        return this.httpRequest.request({
            method: 'PATCH',
            url: '/api/script/{id}/revisions',
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
    public createRevision(
        id: string,
        requestBody: CreateRevisionDto,
    ): CancelablePromise<RevisionDataDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/script/{id}/revisions',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * List a script's environment aliases.
     * @param id
     * @returns AliasDto
     * @throws ApiError
     */
    public listAliases(
        id: string,
    ): CancelablePromise<Array<AliasDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/script/{id}/aliases',
            path: {
                'id': id,
            },
        });
    }

    /**
     * Point an environment alias at a revision. Creating and moving an alias
     * are the same call; rollback is moving it back.
     * @param id
     * @param requestBody
     * @returns AliasDto
     * @throws ApiError
     */
    public setAlias(
        id: string,
        requestBody: SetAliasDto,
    ): CancelablePromise<AliasDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/script/{id}/aliases',
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
    public getScript(
        id: string,
    ): CancelablePromise<ScriptDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/script/{id}',
            path: {
                'id': id,
            },
        });
    }

    /**
     * @param id
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteScript(
        id: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/script/{id}',
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
    public listScripts(
        project: string,
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/scripts',
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
    public createScript(
        project: string,
        requestBody: CreateScriptDto,
    ): CancelablePromise<ScriptDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/scripts',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Which of these content hashes the blob store does not hold. A publish
     * may reference stored hashes without resending their content.
     * @param project
     * @param requestBody
     * @returns MissingBlobsResponseDto
     * @throws ApiError
     */
    public missingBlobs(
        project: string,
        requestBody: MissingBlobsDto,
    ): CancelablePromise<MissingBlobsResponseDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/scripts/missing-blobs',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
