/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ShellOutcomeDto } from '../models/ShellOutcomeDto';
import type { ShellRunDto } from '../models/ShellRunDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ShellService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * @param project
     * @param requestBody
     * @returns ShellOutcomeDto
     * @throws ApiError
     */
    public runShell(
        project: string,
        requestBody: ShellRunDto,
    ): CancelablePromise<ShellOutcomeDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/shell/run',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
