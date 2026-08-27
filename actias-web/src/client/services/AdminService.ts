/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InviteRequestDto } from '../models/InviteRequestDto';
import type { InviteResponseDto } from '../models/InviteResponseDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
import type { RegistrationCodeDto } from '../models/RegistrationCodeDto';
import type { RegistrationSettingsDto } from '../models/RegistrationSettingsDto';
import type { SetAdminDto } from '../models/SetAdminDto';
import type { UserDto } from '../models/UserDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class AdminService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * What the admin surface adapts to: invite-only and mail capability.
     * @returns RegistrationSettingsDto
     * @throws ApiError
     */
    public registrationSettings(): CancelablePromise<RegistrationSettingsDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/admin/registration/settings',
        });
    }

    /**
     * Invite one person: a one-use code wrapped in a register link,
     * mailed when SMTP is configured, returned for copying either way.
     * @param requestBody
     * @returns InviteResponseDto
     * @throws ApiError
     */
    public createInvite(
        requestBody: InviteRequestDto,
    ): CancelablePromise<InviteResponseDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/admin/registration/invite',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Create a new registration code.
     * @param uses
     * @returns RegistrationCodeDto
     * @throws ApiError
     */
    public newRegistrationCode(
        uses: number,
    ): CancelablePromise<RegistrationCodeDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/admin/registration',
            query: {
                'uses': uses,
            },
        });
    }

    /**
     * List created registration codes.
     * @param page
     * @returns any
     * @throws ApiError
     */
    public listRegistrationCodes(
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/admin/registration',
            query: {
                'page': page,
            },
        });
    }

    /**
     * @param code
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteRegistrationCode(
        code: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/admin/registration/{code}',
            path: {
                'code': code,
            },
        });
    }

    /**
     * Every user on the instance, newest first; search matches username
     * or email.
     * @param page
     * @param search
     * @returns any
     * @throws ApiError
     */
    public listUsers(
        page: number,
        search: string,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/admin/users',
            query: {
                'page': page,
                'search': search,
            },
        });
    }

    /**
     * Grants or revokes the admin flag; your own is another admin's to
     * change.
     * @param user
     * @param requestBody
     * @returns UserDto
     * @throws ApiError
     */
    public setUserAdmin(
        user: string,
        requestBody: SetAdminDto,
    ): CancelablePromise<UserDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/admin/users/{user}/admin',
            path: {
                'user': user,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Deletes a user and every project they own, through the same
     * teardown an owner's own delete uses.
     * @param user
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteUser(
        user: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/admin/users/{user}',
            path: {
                'user': user,
            },
        });
    }

    /**
     * Every project on the instance, newest first; search matches the
     * name.
     * @param page
     * @param search
     * @returns any
     * @throws ApiError
     */
    public listAllProjects(
        page: number,
        search: string,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/admin/projects',
            query: {
                'page': page,
                'search': search,
            },
        });
    }

    /**
     * Deletes any project, through the same teardown its owner would
     * trigger.
     * @param project
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public deleteAnyProject(
        project: string,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/admin/projects/{project}',
            path: {
                'project': project,
            },
        });
    }

}
