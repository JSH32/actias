/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateProjectDto } from '../models/CreateProjectDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
import type { ProjectDto } from '../models/ProjectDto';
import type { ProjectMoveDto } from '../models/ProjectMoveDto';
import type { ProjectPolicyDto } from '../models/ProjectPolicyDto';
import type { ProjectPolicyViewDto } from '../models/ProjectPolicyViewDto';
import type { SetProjectRegionDto } from '../models/SetProjectRegionDto';

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
     * Get projects that a user has access to.
     * @param page
     * @returns any
     * @throws ApiError
     */
    public listProjects(
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project',
            query: {
                'page': page,
            },
        });
    }

    /**
     * Get a project by its ID.
     * @param project
     * @returns ProjectDto
     * @throws ApiError
     */
    public getProject(
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

    /**
     * @param project
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteProject(
        project: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Delete a project by its ID.
     * The project's runtime policy: rates and egress lists, the defaults
     * when none was set. Lives in the script service, which is what the
     * workers read it from.
     * @param project
     * @returns ProjectPolicyViewDto
     * @throws ApiError
     */
    public getPolicy(
        project: string,
    ): CancelablePromise<ProjectPolicyViewDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/policy',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Replaces the project's runtime policy. Every field is set: a rate of
     * 0 is unbounded, an empty allow list admits everything not denied.
     * @param project
     * @param requestBody
     * @returns ProjectPolicyViewDto
     * @throws ApiError
     */
    public setPolicy(
        project: string,
        requestBody: ProjectPolicyDto,
    ): CancelablePromise<ProjectPolicyViewDto> {
        return this.httpRequest.request({
            method: 'PATCH',
            url: '/api/project/{project}/policy',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Moves the project to another home: marks it moving, drains, copies
     * its objects between the regions' buckets, flips the home (FLEET.md
     * 6.3). Answers at once with the move to follow; both regions must be
     * registered.
     * @param project
     * @param requestBody
     * @returns ProjectMoveDto
     * @throws ApiError
     */
    public moveProject(
        project: string,
        requestBody: SetProjectRegionDto,
    ): CancelablePromise<ProjectMoveDto> {
        return this.httpRequest.request({
            method: 'PATCH',
            url: '/api/project/{project}/region',
            path: {
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * The project's latest move between homes; an empty step means it
     * never moved.
     * @param project
     * @returns ProjectMoveDto
     * @throws ApiError
     */
    public getMove(
        project: string,
    ): CancelablePromise<ProjectMoveDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/move',
            path: {
                'project': project,
            },
        });
    }

}
