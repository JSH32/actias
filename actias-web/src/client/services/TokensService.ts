/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreatedServiceTokenDto } from '../models/CreatedServiceTokenDto';
import type { CreateServiceTokenDto } from '../models/CreateServiceTokenDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { ServiceTokenDto } from '../models/ServiceTokenDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class TokensService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Create a service token. The response is the only time the secret is
     * shown; only its hash is stored.
     * @param project
     * @param requestBody
     * @returns CreatedServiceTokenDto
     * @throws ApiError
     */
    public createToken(
        project: string,
        requestBody: CreateServiceTokenDto,
    ): CancelablePromise<CreatedServiceTokenDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/tokens',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * List the project's service tokens. Secrets are never listed.
     * @param project
     * @returns ServiceTokenDto
     * @throws ApiError
     */
    public listTokens(
        project: string,
    ): CancelablePromise<Array<ServiceTokenDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/tokens',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Revoke a token. Revocation is deletion: the hash is gone, so the held
     * secret can never authenticate again.
     * @param project
     * @param token
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public revokeToken(
        project: string,
        token: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/tokens/{token}',
            path: {
                'project': project,
                'token': token,
            },
        });
    }

}
