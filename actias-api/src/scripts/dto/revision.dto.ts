import { OmitType } from '@nestjs/swagger';
import { script_service } from 'src/protobufs/script_service';
import { BundleDto } from './bundle.dto';

/**
 * What a script declared at its top level, derived from its code by
 * `actias publish`; the code is the manifest.
 */
export class CapabilitiesDto {
  /**
   * Namespaces declared with `kv "name"`.
   */
  kv: string[];
  /**
   * Events declared with `on "event"`.
   */
  events: string[];
  /**
   * Secrets declared with `secret "name"`.
   */
  secrets: string[];
}

export class ScriptConfigDto {
  id: string;
  entryPoint: string;
  ignore: string[];
  includes: string[];
  /**
   * Derived from the code at publish, never hand-written.
   */
  capabilities?: CapabilitiesDto;
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

  constructor(revision: script_service.Revision) {
    this.id = revision.id;
    this.created = new Date(revision.created);
    this.scriptId = revision.scriptId;
    // The grpc layer omits empty repeated fields, but the openapi contract
    // promises the arrays, so clients decoding strictly need them present.
    this.scriptConfig = {
      ...(revision.scriptConfig as ScriptConfigDto),
      includes: revision.scriptConfig.includes ?? [],
      ignore: revision.scriptConfig.ignore ?? [],
    };
    this.bundle =
      revision.bundle && BundleDto.fromServiceBundle(revision.bundle);
  }
}

export class RevisionDataDto extends OmitType(RevisionFullDto, [
  'bundle',
] as const) {}
