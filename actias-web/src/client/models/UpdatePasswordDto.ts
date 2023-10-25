/* istanbul ignore file */
/* tslint:disable */

export type UpdatePasswordDto = {
    /**
     * This is only needed if a password is set.
     * OAuth only accounts or accounts with alternative methods
     * do not need this.
     */
    currentPassword?: string;
    /**
     * Password between 8 and 64 characters.
     */
    password: string;
};

