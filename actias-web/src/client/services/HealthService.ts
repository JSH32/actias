/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class HealthService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Service and database liveness.
     * @returns any
     * @throws ApiError
     */
    public health(): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/health',
        });
    }

}
