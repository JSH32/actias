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

  /**
   * Object classes declared with `object "Class" { ... }`.
   */
  objects: string[];

  /**
   * Databases declared with `database "name"`.
   */
  databases: string[];

  /**
   * Queues declared with `queue "name"`.
   */
  queues: string[];

  /**
   * Workflow definitions declared with `workflow "name"`.
   */
  workflows: string[];

  /**
   * Step literals found at publish: the declared-possible superset.
   */
  workflowSteps: string[];

  /**
   * Topics each class publishes: "Class:topic", with "=policy" suffixed
   * for built-in policies ("self").
   */
  publishes: string[];

  /**
   * Class lifecycle declarations: "Class:expire=30d" for a declared
   * lifespan, "Class:admit" for a creation gate.
   */
  lifecycle: string[];

  /**
   * Connection classes declared with `connection "Class" { ... }`.
   */
  connections: string[];
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
    const config = revision.scriptConfig as ScriptConfigDto;
    this.scriptConfig = {
      ...config,
      includes: config.includes ?? [],
      ignore: config.ignore ?? [],
      capabilities: config.capabilities && {
        kv: config.capabilities.kv ?? [],
        events: config.capabilities.events ?? [],
        secrets: config.capabilities.secrets ?? [],
        objects: config.capabilities.objects ?? [],
        databases: config.capabilities.databases ?? [],
        queues: config.capabilities.queues ?? [],
        workflows: config.capabilities.workflows ?? [],
        workflowSteps: config.capabilities.workflowSteps ?? [],
        publishes: config.capabilities.publishes ?? [],
        lifecycle: config.capabilities.lifecycle ?? [],
        connections: config.capabilities.connections ?? [],
      },
    };
    this.bundle =
      revision.bundle && BundleDto.fromServiceBundle(revision.bundle);
  }
}

export class RevisionDataDto extends OmitType(RevisionFullDto, [
  'bundle',
] as const) {}
