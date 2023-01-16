import { Type } from 'class-transformer';
import { ValidateNested } from 'class-validator';
import { BundleDto } from './bundle.dto';

export class CreateScriptDto {
  /**
   * Public identifier of the script.
   * This will be the globally unique identifier of your script.
   */
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
  projectConfig: object;
}
