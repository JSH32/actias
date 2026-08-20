/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ResourceInstanceDto = {
    name: string;
    /**
     * Public identifier of a script declaring it; empty when only the directory remembers it.
     */
    declaredBy: string;
    /**
     * Data exists but no live revision declares it; the platform keeps it until it is deleted explicitly.
     */
    orphaned: boolean;
};

