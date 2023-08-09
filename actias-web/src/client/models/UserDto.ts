/* istanbul ignore file */
/* tslint:disable */

export type UserDto = {
    /**
     * Users ID.
     */
    id: string;
    /**
     * When the user was created.
     */
    created: string;
    /**
     * If the user is a system admin.
     */
    admin: boolean;
    /**
     * Users email.
     */
    email: string;
    /**
     * Users username.
     */
    username: string;
};

