/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RunCancelDto } from '../models/RunCancelDto';
import type { RunSignalDto } from '../models/RunSignalDto';
import type { RunStartDto } from '../models/RunStartDto';
import type { WorkflowDefinitionDto } from '../models/WorkflowDefinitionDto';
import type { WorkflowRunDetailDto } from '../models/WorkflowRunDetailDto';
import type { WorkflowRunDto } from '../models/WorkflowRunDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class WorkflowsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Every workflow definition a live contract declares, with the
     * declared-possible step names the skeleton renders.
     * @param project
     * @returns WorkflowDefinitionDto
     * @throws ApiError
     */
    public listDefinitions(
        project: string,
    ): CancelablePromise<Array<WorkflowDefinitionDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/workflows',
            path: {
                'project': project,
            },
        });
    }

    /**
     * The definition's runs, newest first, each with its journal-derived
     * status; the directory names them, the files answer for them.
     * @param project
     * @param definition
     * @returns WorkflowRunDto
     * @throws ApiError
     */
    public listRuns(
        project: string,
        definition: string,
    ): CancelablePromise<Array<WorkflowRunDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/workflows/{definition}/runs',
            path: {
                'project': project,
                'definition': definition,
            },
        });
    }

    /**
     * Starts (or joins) a run; the id is the idempotency key, minted
     * here when the caller has none.
     * @param project
     * @param definition
     * @param requestBody
     * @returns any
     * @throws ApiError
     */
    public startRun(
        project: string,
        definition: string,
        requestBody: RunStartDto,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/workflows/{definition}/runs',
            path: {
                'project': project,
                'definition': definition,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * One run, whole: status plus the journal the CI view folds.
     * @param project
     * @param definition
     * @param id
     * @returns WorkflowRunDetailDto
     * @throws ApiError
     */
    public runDetail(
        project: string,
        definition: string,
        id: string,
    ): CancelablePromise<WorkflowRunDetailDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/workflows/{definition}/runs/{id}',
            path: {
                'project': project,
                'definition': definition,
                'id': id,
            },
        });
    }

    /**
     * Delivers a named signal into the run; a parked await resumes.
     * @param project
     * @param definition
     * @param id
     * @param requestBody
     * @returns any
     * @throws ApiError
     */
    public signal(
        project: string,
        definition: string,
        id: string,
        requestBody: RunSignalDto,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/workflows/{definition}/runs/{id}/signal',
            path: {
                'project': project,
                'definition': definition,
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Cancels the run; children and late signals stay refused.
     * @param project
     * @param definition
     * @param id
     * @param requestBody
     * @returns any
     * @throws ApiError
     */
    public cancel(
        project: string,
        definition: string,
        id: string,
        requestBody: RunCancelDto,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/project/{project}/workflows/{definition}/runs/{id}/cancel',
            path: {
                'project': project,
                'definition': definition,
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
