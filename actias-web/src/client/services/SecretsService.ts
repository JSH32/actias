/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { SecretDto } from '../models/SecretDto';
import type { SetSecretDto } from '../models/SetSecretDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class SecretsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * List a project's live secrets: names and metadata, never values.
     * @param project
     * @returns SecretDto
     * @throws ApiError
     */
    public listSecrets(
        project: string,
    ): CancelablePromise<Array<SecretDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/secrets',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Set or rotate a secret. Every write is a new immutable version; there
     * is no way to read a value back.
     * @param project
     * @param name
     * @param requestBody
     * @returns SecretDto
     * @throws ApiError
     */
    public putSecret(
        project: string,
        name: string,
        requestBody: SetSecretDto,
    ): CancelablePromise<SecretDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/project/{project}/secrets/{name}',
            path: {
                'project': project,
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Delete a secret. The name disappears and scripts stop resolving it;
     * workflow runs that pinned a version keep the credentials they
     * started with.
     * @param project
     * @param name
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteSecret(
        project: string,
        name: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/secrets/{name}',
            path: {
                'project': project,
                'name': name,
            },
        });
    }

}
