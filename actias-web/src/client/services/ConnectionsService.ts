/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ConnectionDto } from '../models/ConnectionDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ConnectionsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * @param project
     * @returns ConnectionDto
     * @throws ApiError
     */
    public listConnections(
        project: string,
    ): CancelablePromise<Array<ConnectionDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/connections',
            path: {
                'project': project,
            },
        });
    }

}
