/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type StatePairDto = {
    key: string;
    /**
     * How 'value' parses: the kv typed-pair kind.
     */
    type: string;
    /**
     * The stored value, as encoded text.
     */
    value: string;
};

