/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AuthTokenDto } from '../models/AuthTokenDto';
import type { LoginDto } from '../models/LoginDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class AuthService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Login with username/password and recieve an authentication token.
     * @param requestBody
     * @returns AuthTokenDto
     * @throws ApiError
     */
    public authControllerLogin(
        requestBody: LoginDto,
    ): CancelablePromise<AuthTokenDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/auth/login',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
