/* istanbul ignore file */
/* tslint:disable */
import type { CreateUserDto } from '../models/CreateUserDto';
import type { MessageResponseDto } from '../models/MessageResponseDto';
import type { PaginatedResponseDto } from '../models/PaginatedResponseDto';
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
     * @param name
     * @param page
     * @returns any
     * @throws ApiError
     */
    public searchUsers(
        name: string,
        page: number,
    ): CancelablePromise<PaginatedResponseDto> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/users',
            query: {
                'name': name,
                'page': page,
            },
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

}
