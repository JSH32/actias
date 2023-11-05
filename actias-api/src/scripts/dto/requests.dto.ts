import { Type } from 'class-transformer';
import { IsAlphanumeric, Length, ValidateNested } from 'class-validator';
import { BundleDto } from './bundle.dto';
import { ScriptConfigDto } from './revision.dto';

export class CreateScriptDto {
  /**
   * Public identifier of the script.
   * This will be the globally unique identifier of your script.
   */
  @IsAlphanumeric()
  @Length(3, 63) // Length of S3 containers.
  publicIdentifier: string;
}

export class NewRevisionResponseDto {
  scriptId: string;
  /**
   * New revision ID.
   * This may be null.
   */
  revisionId?: string;
}

export class CreateRevisionDto {
  /**
   * The bundle which will be used.
   */
  @ValidateNested()
  @Type(() => BundleDto)
  bundle: BundleDto;

  /**
   * A valid project configuration.
   */
  scriptConfig: ScriptConfigDto;
}
