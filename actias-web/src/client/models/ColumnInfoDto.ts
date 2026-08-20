/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ColumnInfoDto = {
    name: string;
    /**
     * Declared SQLite type; empty when untyped.
     */
    type: string;
    notNull: boolean;
    primaryKey: boolean;
};

