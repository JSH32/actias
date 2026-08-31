/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateInterestDto } from '../models/CreateInterestDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class InterestService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Ask to be kept posted. Public, because the landing page is where
     * this is asked and nobody has an account yet.
     *
     * The reply is the same whether the address was new or already on the
     * list, so the endpoint cannot be used to test whether someone signed
     * up.
     * @param requestBody
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public keepMePosted(
        requestBody: CreateInterestDto,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/interest',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Read the list.
     * @param page
     * @returns any
     * @throws ApiError
     */
    public listInterest(
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/interest',
            query: {
                'page': page,
            },
        });
    }

}
