import { script_service } from 'src/protobufs/script_service';
import { BundleDto } from './bundle.dto';

export class RevisionDto {
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
  projectConfig: object;

  /**
   * Content bundle of all files.
   * This is only present in some responses.
   */
  bundle?: BundleDto;

  constructor(revision: script_service.Revision) {
    this.id = revision.id;
    this.created = new Date(revision.created);
    this.scriptId = revision.scriptId;
    // This is fine because script service does validation on JSON.
    this.projectConfig = JSON.parse(revision.projectConfig);
    this.bundle = revision.bundle && new BundleDto(revision.bundle);
  }
}
