/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { RegistrationCodeDto } from './RegistrationCodeDto';

export type InviteResponseDto = {
    /**
     * The one-use code backing the invite.
     */
    code: RegistrationCodeDto;
    /**
     * The register link the code rides in.
     */
    link: string;
    /**
     * Whether the invite was mailed; false means copy the link instead.
     */
    emailed: boolean;
};

