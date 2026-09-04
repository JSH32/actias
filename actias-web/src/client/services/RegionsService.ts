/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PutRegionDto } from '../models/PutRegionDto';
import type { RegionDto } from '../models/RegionDto';

import type { CancelablePromise } from '../core/CancelablePromise';
import type { BaseHttpRequest } from '../core/BaseHttpRequest';

export class RegionsService {

    constructor(public readonly httpRequest: BaseHttpRequest) {}

    /**
     * Every registered region. Empty on a single-region deployment.
     * @returns RegionDto
     * @throws ApiError
     */
    public listRegions(): CancelablePromise<Array<RegionDto>> {
        return this.httpRequest.request({
            method: 'GET',
            url: '/api/regions',
        });
    }

    /**
     * Registers or updates a region: its data-plane address and bucket.
     * @param name
     * @param requestBody
     * @returns RegionDto
     * @throws ApiError
     */
    public putRegion(
        name: string,
        requestBody: PutRegionDto,
    ): CancelablePromise<RegionDto> {
        return this.httpRequest.request({
            method: 'PUT',
            url: '/api/regions/{name}',
            path: {
                'name': name,
            },
            body: requestBody,
            mediaType: 'application/json',
        });
    }

    /**
     * Forgets a region. Refused while any project calls it home.
     * @param name
     * @returns any
     * @throws ApiError
     */
    public deleteRegion(
        name: string,
    ): CancelablePromise<any> {
        return this.httpRequest.request({
            method: 'DELETE',
            url: '/api/regions/{name}',
            path: {
                'name': name,
            },
        });
    }

}
