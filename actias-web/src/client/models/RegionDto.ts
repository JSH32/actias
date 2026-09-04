/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type RegionDto = {
    /**
     * A region token: one to sixteen of a-z, 0-9 and '-', not starting with '-'.
     */
    name: string;
    /**
     * The region's data-plane ingress, host:port, what other regions forward to.
     */
    dataPlaneAddr: string;
    /**
     * The region's object bucket.
     */
    bucket: string;
    /**
     * The region's placement service as the control plane reaches it; a
     * move lists the project's objects there. Empty for the control
     * plane's own region, which it reaches by its own setting.
     */
    placementAddr: string;
    /**
     * The region's own object storage endpoint, when it is not the
     * control plane's; empty means the control plane's S3 settings reach
     * the bucket. The access key is shown; the secret never is.
     */
    s3Endpoint: string;
    s3AccessKey: string;
};

