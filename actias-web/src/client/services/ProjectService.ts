/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
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
    public projectControllerCreateProject(
        requestBody: CreateProjectDto,
    ): CancelablePromise<ProjectDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/project',
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
    public projectControllerGet(
        project: string,
    ): CancelablePromise<ProjectDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/project/{project}',
            path: {
                'project': project,
            },
        });
    }

}
