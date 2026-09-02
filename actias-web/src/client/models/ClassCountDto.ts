/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { DirectoryFieldDto } from './DirectoryFieldDto';

export type ClassCountDto = {
    class: string;
    count: number;
    /**
     * Whether the class's current revision declares a directory, so the console offers a search only where one exists.
     */
    hasDirectory: boolean;
    /**
     * The fields that directory declares, so a filter can be typed against the same set the worker enforces. Empty for a class with no directory, and for one published before fields were declared.
     */
    directoryFields: Array<DirectoryFieldDto>;
    /**
     * The methods the class's current revision declares, by name, so a shell can offer what an instance answers to. Which of them write is not declarable; a read-only session refuses at the call.
     */
    methods: Array<string>;
};

