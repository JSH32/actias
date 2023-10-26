/* istanbul ignore file */
/* tslint:disable */

export type LoginDto = {
    /**
     * Either username or email.
     */
    auth: string;
    password: string;
    /**
     * Should the generated token be remembered?
     * This changes expiration to 60 days from 1 day.
     */
    rememberMe?: boolean;
};

