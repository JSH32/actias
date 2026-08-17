/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { SetSecretDto } from '../models/SetSecretDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class SecretsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * List the names of a project's secrets. Values are never returned.
     * @param project
     * @returns string
     * @throws ApiError
     */
    public listSecrets(
        project: string,
    ): CancelablePromise<Array<string>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/secrets',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Set a secret, encrypting it at rest. Overwrites an existing value.
     * @param project
     * @param name
     * @param requestBody
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public putSecret(
        project: string,
        name: string,
        requestBody: SetSecretDto,
    ): CancelablePromise<MessageResponseDto> {
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
     * Delete a secret.
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
