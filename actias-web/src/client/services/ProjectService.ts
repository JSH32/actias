/* istanbul ignore file */
/* tslint:disable */
import type { CreateProjectDto } from '../models/CreateProjectDto';
import type { ProjectDto } from '../models/ProjectDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class ProjectService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Create a project and return the data.
     * @param requestBody
     * @returns ProjectDto
     * @throws ApiError
     */
    public createProject(
        requestBody: CreateProjectDto,
    ): CancelablePromise<ProjectDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Get a project by its ID.
     * @param project
     * @returns ProjectDto
     * @throws ApiError
     */
    public get(
        project: string,
    ): CancelablePromise<ProjectDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}',
            path: {
                'project': project,
            },
        });
    }

}
