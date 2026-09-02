/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type DirectoryRebuiltDto = {
    /**
     * Identities the placement store still lists as live.
     */
    live: number;
    /**
     * Rows recovered from manifests.
     */
    rows: number;
    /**
     * Live objects whose manifest carries no row yet. Not an error: nothing has settled to copy, and a backfill is what covers them.
     */
    withoutRow: number;
    /**
     * Rows retired because the object no longer exists.
     */
    tombstones: number;
    /**
     * Whether a node did the work. False means another node holds the class and is rebuilding it already.
     */
    held: boolean;
};

