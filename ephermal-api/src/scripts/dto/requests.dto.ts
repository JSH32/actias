import { script_service } from 'src/protobufs/script_service';
import { BundleDto } from './bundle.dto';

export enum RevisionRequestTypeDto {
  NONE = 'none',
  LATEST = 'latest',
  ALL = 'all',
}

export const toRevisionNum = (request: RevisionRequestTypeDto) =>
  ({
    [RevisionRequestTypeDto.ALL]:
      script_service.FindScriptRequest.RevisionRequestType.ALL,
    [RevisionRequestTypeDto.LATEST]:
      script_service.FindScriptRequest.RevisionRequestType.LATEST,
    [RevisionRequestTypeDto.NONE]:
      script_service.FindScriptRequest.RevisionRequestType.NONE,
  }[request]);

export class CreateScriptDto {
  /**
   * Public identifier of the script.
   * This will be the globally unique identifier of your script.
   */
  publicIdentifier: string;
}

export class CreateRevisionDto {
  /**
   * The bundle which will be used.
   */
  bundle: BundleDto;

  /**
   * A valid project configuration.
   */
  projectConfig: object;
}
