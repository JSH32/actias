/* istanbul ignore file */
/* tslint:disable */
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
    public login(
        requestBody: LoginDto,
    ): CancelablePromise<AuthTokenDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/auth/login',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
