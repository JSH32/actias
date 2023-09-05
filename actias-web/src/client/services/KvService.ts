/* istanbul ignore file */
/* tslint:disable */
import type { ListNamespaceDto } from '../models/ListNamespaceDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { NamespaceDto } from '../models/NamespaceDto';
import type { PairDto } from '../models/PairDto';
import type { SetKeyDto } from '../models/SetKeyDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class KvService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * @param project
     * @returns NamespaceDto
     * @throws ApiError
     */
    public listNamespaces(
        project: string,
    ): CancelablePromise<Array<NamespaceDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/kv',
            path: {
                'project': project,
            },
        });
    }

    /**
     * @param project
     * @param namespace
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteNamespace(
        project: string,
        namespace: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/kv/{namespace}',
            path: {
                'project': project,
                'namespace': namespace,
            },
        });
    }

    /**
     * List pairs in a namespace.
     * @param project
     * @param namespace
     * @param token Pagination token
     * @returns ListNamespaceDto
     * @throws ApiError
     */
    public listNamespace(
        project: string,
        namespace: string,
        token?: string,
    ): CancelablePromise<ListNamespaceDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/kv/{namespace}',
            path: {
                'project': project,
                'namespace': namespace,
            },
            query: {
                'token': token,
            },
        });
    }

    /**
     * @param project
     * @param namespace
     * @param key
     * @returns PairDto
     * @throws ApiError
     */
    public getKey(
        project: string,
        namespace: string,
        key: string,
    ): CancelablePromise<PairDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/kv/{namespace}/{key}',
            path: {
                'project': project,
                'namespace': namespace,
                'key': key,
            },
        });
    }

    /**
     * @param project
     * @param namespace
     * @param key
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteKey(
        project: string,
        namespace: string,
        key: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/project/{project}/kv/{namespace}/{key}',
            path: {
                'project': project,
                'namespace': namespace,
                'key': key,
            },
        });
    }

    /**
     * @param project
     * @param namespace
     * @param key
     * @param requestBody
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public setKey(
        project: string,
        namespace: string,
        key: string,
        requestBody: SetKeyDto,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/project/{project}/kv/{namespace}/{key}',
            path: {
                'project': project,
                'namespace': namespace,
                'key': key,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

}
