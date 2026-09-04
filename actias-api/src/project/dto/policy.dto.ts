import { IsArray, IsInt, IsString, Matches, Min } from 'class-validator';
import { script_service } from 'src/protobufs/script_service';

/**
 * A project's runtime policy: what its scripts may spend on a node and
 * where they may reach. Zero rates mean the platform default (unbounded);
 * an empty allow list admits every host the deny list and the node's own
 * policy do not refuse.
 */
export class ProjectPolicyDto {
  /**
   * Requests a node admits for the project per second, burst of the
   * same size; 0 is unbounded.
   */
  @IsInt()
  @Min(0)
  requestsPerSec!: number;

  /**
   * Work units a node lets the project spend per second; 0 is unbounded.
   */
  @IsInt()
  @Min(0)
  workUnitsPerSec!: number;

  /**
   * Hosts outbound requests and dials may reach; a leading dot matches
   * subdomains. Empty admits everything not denied.
   */
  @IsArray()
  @IsString({ each: true })
  egressAllow!: string[];

  /**
   * Hosts refused before the allow list is consulted.
   */
  @IsArray()
  @IsString({ each: true })
  egressDeny!: string[];

  static fromProto(policy: script_service.ProjectPolicy): ProjectPolicyDto {
    return Object.assign(new ProjectPolicyDto(), {
      requestsPerSec: Number(policy.requestsPerSec ?? 0),
      workUnitsPerSec: Number(policy.workUnitsPerSec ?? 0),
      egressAllow: policy.egressAllow ?? [],
      egressDeny: policy.egressDeny ?? [],
    });
  }
}

/**
 * The project's runtime policy as read: the editable fields plus where
 * the project lives, which is set on its own.
 */
export class ProjectPolicyViewDto extends ProjectPolicyDto {
  /**
   * The project's home region: where its named objects are born and its
   * directory lives.
   */
  region!: string;

  /** Whether the project is between homes; calls are refused, retryably, until it settles. */
  moving!: boolean;

  static fromProto(policy: script_service.ProjectPolicy): ProjectPolicyViewDto {
    return Object.assign(new ProjectPolicyViewDto(), {
      ...ProjectPolicyDto.fromProto(policy),
      region: policy.region ?? '',
      moving: policy.moving ?? false,
    });
  }
}

/**
 * A project's latest move between homes, as the console follows it.
 */
export class ProjectMoveDto {
  fromRegion!: string;

  toRegion!: string;

  /** marking, draining, copying, flipping, done, failed; empty when the project never moved. */
  step!: string;

  objectsTotal!: number;

  objectsCopied!: number;

  /** Set when the step is failed; the move may be started again. */
  error!: string;

  /** Unix milliseconds; 0 when the project never moved. */
  startedAt!: number;

  /** Unix milliseconds; 0 while the move runs. */
  finishedAt!: number;

  static fromProto(move: script_service.ProjectMove): ProjectMoveDto {
    return Object.assign(new ProjectMoveDto(), {
      fromRegion: move.fromRegion ?? '',
      toRegion: move.toRegion ?? '',
      step: move.step ?? '',
      objectsTotal: Number(move.objectsTotal ?? 0),
      objectsCopied: Number(move.objectsCopied ?? 0),
      error: move.error ?? '',
      startedAt: Number(move.startedMs ?? 0),
      finishedAt: Number(move.finishedMs ?? 0),
    });
  }
}

/** Sets the project's home region. */
export class SetProjectRegionDto {
  /** A region token: one to sixteen of a-z, 0-9 and '-', not starting with '-'. */
  @IsString()
  @Matches(/^[a-z0-9][a-z0-9-]{0,15}$/)
  region!: string;
}
