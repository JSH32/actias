import { IsOptional, IsString, Matches, MinLength } from 'class-validator';
import { script_service } from 'src/protobufs/script_service';

/**
 * A region the control plane knows: a data plane that can be reached
 * and the bucket that holds its bytes. A single-region deployment
 * registers none; a region nobody registered routes to the project's
 * home.
 */
export class RegionDto {
  /** A region token: one to sixteen of a-z, 0-9 and '-', not starting with '-'. */
  name!: string;

  /** The region's data-plane ingress, host:port, what other regions forward to. */
  dataPlaneAddr!: string;

  /** The region's object bucket. */
  bucket!: string;

  /**
   * The region's placement service as the control plane reaches it; a
   * move lists the project's objects there. Empty for the control
   * plane's own region, which it reaches by its own setting.
   */
  placementAddr!: string;

  /**
   * The region's own object storage endpoint, when it is not the
   * control plane's; empty means the control plane's S3 settings reach
   * the bucket. The access key is shown; the secret never is.
   */
  s3Endpoint!: string;

  s3AccessKey!: string;

  static fromProto(region: script_service.Region): RegionDto {
    return Object.assign(new RegionDto(), {
      name: region.name,
      dataPlaneAddr: region.dataPlaneAddr,
      bucket: region.bucket,
      placementAddr: region.placementAddr ?? '',
      s3Endpoint: region.s3Endpoint ?? '',
      s3AccessKey: region.s3AccessKey ?? '',
    });
  }
}

/** Registers or updates a region; the name is in the path. */
export class PutRegionDto {
  @IsString()
  @MinLength(1)
  dataPlaneAddr!: string;

  @IsString()
  @MinLength(1)
  bucket!: string;

  @IsOptional()
  @IsString()
  placementAddr?: string;

  /** The region's own S3 endpoint with its scheme; omit for the control plane's. */
  @IsOptional()
  @IsString()
  s3Endpoint?: string;

  @IsOptional()
  @IsString()
  s3AccessKey?: string;

  /** Write-only; never read back. */
  @IsOptional()
  @IsString()
  s3SecretKey?: string;
}

export const REGION_TOKEN = /^[a-z0-9][a-z0-9-]{0,15}$/;

export class RegionNameParam {
  @Matches(REGION_TOKEN)
  name!: string;
}
