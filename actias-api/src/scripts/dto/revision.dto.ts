import { OmitType } from '@nestjs/swagger';
import { script_service } from 'src/protobufs/script_service';
import { BundleDto } from './bundle.dto';

export class ScriptConfigDto {
  id: string;
  entryPoint: string;
  includes: string[];
}

export class RevisionFullDto {
  id: string;

  /**
   * Date that revision was published.
   */
  created: Date;

  /**
   * ID of the script this revision is attached to.
   */
  scriptId: string;

  /**
   * Config that the project was uploaded with.
   * This is metadata and is mostly included for CLI to restore revisions intact.
   */
  scriptConfig: ScriptConfigDto;

  /**
   * Content bundle of all files.
   * This is only present in some responses.
   */
  bundle?: BundleDto;

  constructor(scriptId: string, revision: script_service.Revision) {
    this.id = revision.id;
    this.created = new Date(revision.created);
    this.scriptId = scriptId;
    // This is fine because script service does validation on JSON.
    this.scriptConfig = JSON.parse(revision.scriptConfig);
    this.bundle =
      revision.bundle && BundleDto.fromServiceBundle(revision.bundle);
  }
}

export class RevisionDataDto extends OmitType(RevisionFullDto, [
  'bundle',
] as const) {}
