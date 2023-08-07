/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type FileDto = {
    /**
     * ID of the revision this file resides in.
     * This is empty when creating a revision or uploading a bundle.
     */
    revisionId?: string;
    /**
     * Name of the file
     */
    fileName: string;
    /**
     * Path of file relative from the root path
     */
    filePath: string;
    /**
     * Content of the file, base64 encoded
     */
    content: string;
};

