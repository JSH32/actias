/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type DirectoryConditionDto = {
    /**
     * Field name as the class's directory function returned it; nested tables arrive flattened, so "location.region" is one field.
     */
    field: string;
    /**
     * eq, ne, lt, lte, gt, gte, one_of, starts_with, contains or exists. one_of rather than in, because in is a Lua keyword.
     */
    op: string;
    /**
     * The operand, json-encoded: a scalar, an array for one_of, or a boolean for exists.
     */
    valueJson: string;
};

