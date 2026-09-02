/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ShellOutcomeDto = {
    /**
     * The chunk's return value as json text; "null" for nothing.
     */
    valueJson: string;
    /**
     * Every print and log line, in order.
     */
    output: Array<string>;
    /**
     * The chunk's own error, when it failed.
     */
    error?: string;
    /**
     * Work units the run consumed.
     */
    work: number;
    /**
     * Milliseconds the run took on the node.
     */
    wallMs: number;
};

