/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type CreateInterestDto = {
    email: string;
    /**
     * Which surface this came from. Defaults to `landing`, which is the
     * only one that asks today.
     */
    source?: string;
};

