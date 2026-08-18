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

/**
 * Content hashes a publish is about to reference.
 */
export class MissingBlobsDto {
  /**
   * blake3 hashes of the files, lowercase hex.
   */
  hashes: string[];
}

/**
 * The subset of asked-about hashes the store does not hold; a publish only
 * needs to carry content for these.
 */
export class MissingBlobsResponseDto {
  missing: string[];
}

/**
 * Points a named environment alias at a revision; creating and moving are
 * the same call.
 */
export class SetAliasDto {
  /**
   * Alias name: 1-64 lowercase letters, digits or single dashes. `live-`
   * and `r-` prefixes are reserved for routing.
   */
  name: string;

  /**
   * Revision the alias serves; must belong to the script.
   */
  revisionId: string;
}

/**
 * One named environment alias.
 */
export class AliasDto {
  scriptId: string;
  name: string;
  revisionId: string;

  constructor(alias: {
    scriptId?: string;
    name?: string;
    revisionId?: string;
  }) {
    this.scriptId = alias.scriptId ?? '';
    this.name = alias.name ?? '';
    this.revisionId = alias.revisionId ?? '';
  }
}
