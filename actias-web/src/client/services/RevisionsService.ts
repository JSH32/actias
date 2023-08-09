/* istanbul ignore file */
/* tslint:disable */
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
    public getRevision(
        id: string,
        withBundle: boolean,
    ): CancelablePromise<RevisionFullDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/revisions/{id}',
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
    public deleteRevision(
        id: string,
    ): CancelablePromise<NewRevisionResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/revisions/{id}',
            path: {
                'id': id,
            },
        });
    }

}
