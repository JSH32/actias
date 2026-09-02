/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type DirectoryEntryDto = {
    /**
     * The instance name; what you call to reach the object.
     */
    name: string;
    objectId: string;
    /**
     * Field name to json-encoded value, for the fields this object has. A field the object lacks is simply absent.
     */
    fields: Record<string, string>;
};

