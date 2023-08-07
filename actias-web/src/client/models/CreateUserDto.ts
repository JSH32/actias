/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type CreateUserDto = {
    /**
     * Username, must be unique.
     */
    username: string;
    email: string;
    /**
     * Password between 8 and 64 characters.
     */
    password: string;
};

