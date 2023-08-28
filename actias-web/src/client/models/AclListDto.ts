/* istanbul ignore file */
/* tslint:disable */

import type { UserDto } from './UserDto';

export type AclListDto = {
    /**
     * User id that the permissions apply to.
     */
    user: UserDto;
    /**
     * List of permissions
     */
    permissions: any;
};

