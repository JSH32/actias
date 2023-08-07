/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { NewRevisionResponseDto } from '../models/NewRevisionResponseDto';
import type { RevisionFullDto } from '../models/RevisionFullDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class RevisionsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Get a revision by ID.
     * @param id
     * @param withBundle
     * @returns RevisionFullDto
     * @throws ApiError
     */
    public revisionsControllerGetRevision(
        id: string,
        withBundle: boolean,
    ): CancelablePromise<RevisionFullDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/revisions/{id}',
            path: {
                'id': id,
            },
            query: {
                'withBundle': withBundle,
            },
        });
    }

    /**
     * Delete a revision by ID.
     * @param id
     * @returns NewRevisionResponseDto
     * @throws ApiError
     */
    public revisionsControllerDeleteRevision(
        id: string,
    ): CancelablePromise<NewRevisionResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/revisions/{id}',
            path: {
                'id': id,
            },
        });
    }

}
