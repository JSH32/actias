/* istanbul ignore file */
/* tslint:disable */
import type { AclListDto } from '../models/AclListDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class AclService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Get ACL list for the current authorized user.
     * This doesn't require the `PERMISSIONS_READ` permission.
     * @param project
     * @returns AclListDto
     * @throws ApiError
     */
    public getAclMe(
        project: string,
    ): CancelablePromise<AclListDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/acl/@me',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Get ACL list for a single user.
     * @param project
     * @param user
     * @returns AclListDto
     * @throws ApiError
     */
    public getAclSingle(
        project: string,
        user: string,
    ): CancelablePromise<AclListDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/acl/{user}',
            path: {
                'project': project,
                'user': user,
            },
        });
    }

    /**
     * Set ACL access for the project for the user.
     * This will implicitly add the user to the project.
     * If `permissions` is empty then the user will be removed from the project.
     * @param user
     * @param project
     * @param requestBody
     * @returns AclListDto
     * @throws ApiError
     */
    public putAcl(
        user: string,
        project: string,
        requestBody: Array<string>,
    ): CancelablePromise<AclListDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/project/{project}/acl/{user}',
            path: {
                'user': user,
                'project': project,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Get ACL list for all users.
     * @param project
     * @returns AclListDto
     * @throws ApiError
     */
    public getAcl(
        project: string,
    ): CancelablePromise<Array<AclListDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/project/{project}/acl',
            path: {
                'project': project,
            },
        });
    }

    /**
     * Get a list of all permissions.
     * @returns string
     * @throws ApiError
     */
    public getPermissions(): CancelablePromise<Array<string>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/acl/permissions',
        });
    }

}
