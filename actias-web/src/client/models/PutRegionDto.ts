/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type PutRegionDto = {
    dataPlaneAddr: string;
    bucket: string;
    placementAddr?: string;
    /**
     * The region's own S3 endpoint with its scheme; omit for the control plane's.
     */
    s3Endpoint?: string;
    s3AccessKey?: string;
    /**
     * Write-only; never read back.
     */
    s3SecretKey?: string;
};

