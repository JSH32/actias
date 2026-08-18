/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type SetAliasDto = {
    /**
     * Alias name: 1-64 lowercase letters, digits or single dashes. `live-`
     * and `r-` prefixes are reserved for routing.
     */
    name: string;
    /**
     * Revision the alias serves; must belong to the script.
     */
    revisionId: string;
};

