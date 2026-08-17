/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type FileDto = {
    /**
     * How the platform treats the file; modules are lua source, assets are
     * served as-is. Defaults to module.
     */
    kind?: FileDto.kind;
    /**
     * Path of the file relative to the bundle root; its identity within the
     * bundle.
     */
    filePath: string;
    /**
     * Content of the file, base64 encoded.
     */
    content: string;
    /**
     * Mime type served for assets; informative for modules.
     */
    contentType?: string;
    /**
     * blake3 of the content, computed by the store; ignored on upload.
     */
    hash?: string;
    /**
     * Content size in bytes, computed by the store; ignored on upload.
     */
    size?: number;
};

export namespace FileDto {

    /**
     * How the platform treats the file; modules are lua source, assets are
     * served as-is. Defaults to module.
     */
    export enum kind {
        MODULE = 'module',
        ASSET = 'asset',
    }


}

