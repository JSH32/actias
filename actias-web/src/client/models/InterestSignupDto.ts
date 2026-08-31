/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type InterestSignupDto = {
    /**
     * Signup id.
     */
    id: string;
    /**
     * Address that asked to be kept posted.
     */
    email: string;
    /**
     * Surface the address came from.
     */
    source: string;
    /**
     * When the address was first recorded.
     */
    createdAt: string;
};

