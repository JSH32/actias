import { script_service } from 'src/protobufs/script_service';

export class ScriptDto {
  id: string;

  /**
   * Public identifier of the script.
   * This must be globally unique
   */
  publicIdentifier: string;

  /**
   * Date that the script (or revisions) were last updated.
   */
  lastUpdated: Date;

  /**
   * ID of the currently deployed revision.
   */
  currentRevisionId: string;

  constructor(bundle: script_service.Script) {
    this.id = bundle.id;
    this.currentRevisionId = bundle.currentRevisionId;
    this.publicIdentifier = bundle.publicIdentifier;
    this.lastUpdated = new Date(bundle.lastUpdated);
  }
}
