/* istanbul ignore file */
/* tslint:disable */

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
    /**
     * Registration code (if needed).
     */
    registrationCode?: string;
};

