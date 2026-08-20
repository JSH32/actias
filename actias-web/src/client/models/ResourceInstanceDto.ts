/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ResourceInstanceDto = {
    name: string;
    scriptId: string;
    /**
     * Public identifier of the owning script.
     */
    scriptIdentifier: string;
    /**
     * Data exists but no live revision declares it; the platform keeps it until it is deleted explicitly.
     */
    orphaned: boolean;
};

