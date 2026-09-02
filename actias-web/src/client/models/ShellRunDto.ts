/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ShellRunDto = {
    /**
     * The chunk to run: any Luau, as typed or pasted. Its return value is the result.
     */
    source: string;
    /**
     * Wall budget in seconds; the node caps it.
     */
    wallSecs?: number;
    /**
     * Whether the session is in write mode. Off, the chunk still runs, and the vm refuses kv set/delete, database exec and method calls inside it.
     */
    write?: boolean;
};

