/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateUserDto } from '../models/CreateUserDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PublicUserDto } from '../models/PublicUserDto';
import type { RegistrationConfigDto } from '../models/RegistrationConfigDto';
import type { UpdatePasswordDto } from '../models/UpdatePasswordDto';
import type { UpdateUserDto } from '../models/UpdateUserDto';
import type { UserDto } from '../models/UserDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class UsersService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Create a new user using standard username/password sign up.
     * @param requestBody
     * @returns UserDto
     * @throws ApiError
     */
    public createUser(
        requestBody: CreateUserDto,
    ): CancelablePromise<UserDto> {
        return this.httpRequest.request({
            method: 'POST',
            url: '/api/users',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * @returns RegistrationConfigDto
     * @throws ApiError
     */
    public registrationConfig(): CancelablePromise<RegistrationConfigDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/users/registrationConfig',
        });
    }

    /**
     * Get the currently logged in user's details.
     * @returns UserDto
     * @throws ApiError
     */
    public me(): CancelablePromise<UserDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/users/@me',
        });
    }

    /**
     * Update user details.
     * @param requestBody
     * @returns UserDto
     * @throws ApiError
     */
    public update(
        requestBody: UpdateUserDto,
    ): CancelablePromise<UserDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/users/@me',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Update user details.
     * @param requestBody
     * @returns MessageResponseDto
     * @throws ApiError
     */
    public updatePassword(
        requestBody: UpdatePasswordDto,
    ): CancelablePromise<MessageResponseDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/users/@me/password',
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Resolve one account by its exact email or username, for adding a
     * member to a project. Nothing here lists or matches loosely: browsing
     * the user table is an admin capability, at `GET /admin/users`.
     * @param identifier
     * @returns PublicUserDto
     * @throws ApiError
     */
    public lookupUser(
        identifier: string,
    ): CancelablePromise<PublicUserDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/users/lookup',
            query: {
                'identifier': identifier,
            },
        });
    }

}
