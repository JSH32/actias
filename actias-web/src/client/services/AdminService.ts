/* istanbul ignore file */
/* tslint:disable */
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
import type { RegistrationCodeDto } from '../models/RegistrationCodeDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class AdminService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Create a new registration code.
     * @param uses
     * @returns RegistrationCodeDto
     * @throws ApiError
     */
    public newRegistrationCode(
        uses: number,
    ): CancelablePromise<RegistrationCodeDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/admin/registration',
            query: {
                'uses': uses,
            },
        });
    }

    /**
     * List created registration codes.
     * @param page
     * @returns any
     * @throws ApiError
     */
    public listRegistrationCodes(
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/admin/registration',
            query: {
                'page': page,
            },
        });
    }

    /**
     * @param code
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteRegistrationCode(
        code: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/admin/registration/{code}',
            path: {
                'code': code,
            },
        });
    }

}
