/* istanbul ignore file */
/* tslint:disable */

export type PairDto = {
    type: PairDto.type;
    projectId: string;
    namespace: string;
    ttl: number;
    key: string;
    value: string;
};

export namespace PairDto {

    export enum type {
        '_0' = 0,
        '_1' = 1,
        '_2' = 2,
        '_3' = 3,
        '_4' = 4,
    }


}

