import {
  BadGatewayException,
  BadRequestException,
  Inject,
  Injectable,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ConfigService } from '@nestjs/config';
import { Metadata } from '@grpc/grpc-js';
import { lastValueFrom } from 'rxjs';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { node_registry } from 'src/protobufs/node_registry';
import { worker_data } from 'src/protobufs/worker_data';
import { DatabaseOverviewDto, ResourceInstanceDto } from './dto/resources.dto';

/** The platform class each resource kind rides on. */
export const CLASSES = { queues: '__queue', databases: '__database' } as const;

/** Rows one directory page may carry; larger asks clamp, never error. */
export function clampPageSize(requested?: number): number {
  if (!requested || requested <= 0 || Number.isNaN(requested)) return 100;
  return Math.min(Math.floor(requested), 500);
}

/**
 * The backplane's shared plumbing: the grpc clients and the worker
 * data-plane calls every resource family rides. Controllers stay thin
 * route surfaces; identity is project-scoped ((project, class, name)),
 * so reads and calls carry the project, never a script.
 */
@Injectable()
export class ResourcesService {
  scripts: script_service.ScriptService;
  registry: node_registry.NodeRegistryService;
  workers: worker_data.WorkerData;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly scriptClient: ClientGrpc,
    @Inject('NODE_REGISTRY') private readonly registryClient: ClientGrpc,
    @Inject('WORKER_DATA') private readonly workerClient: ClientGrpc,
    private readonly config: ConfigService,
  ) {}

  onModuleInit() {
    this.scripts =
      this.scriptClient.getService<script_service.ScriptService>(
        'ScriptService',
      );
    this.registry =
      this.registryClient.getService<node_registry.NodeRegistryService>(
        'NodeRegistryService',
      );
    this.workers =
      this.workerClient.getService<worker_data.WorkerData>('WorkerData');
  }

  /** Every data-plane call carries the cluster-internal secret. */
  internalMetadata(): Metadata {
    const metadata = new Metadata();
    metadata.set(
      'x-actias-internal',
      this.config.get<string>('worker.internalToken'),
    );
    return metadata;
  }

  /** The json a read answered with; an empty or `null` value reads as
   * "no observable state yet". */
  parseValue(
    json: string | undefined,
  ): Record<string, unknown> | unknown[] | null {
    if (!json) return null;
    try {
      return JSON.parse(json);
    } catch {
      throw new BadGatewayException('The worker answered garbage.');
    }
  }

  /** One typed platform read over the data plane, answered from the
   * freshest copy the worker can reach (its file, the holder's, the
   * replica). With `sql`, one read-only statement instead of the class
   * overview; with `messages`, the queue's message rows. */
  async workerRead(
    project: Projects,
    className: string,
    name: string,
    options: { sql?: string; messages?: boolean; followers?: boolean } = {},
  ): Promise<Record<string, unknown> | unknown[] | null> {
    const value = await lastValueFrom(
      this.workers
        .readStats(
          {
            scopeId: project.id,
            class: className,
            name,
            sql: options.sql,
            messages: options.messages ?? false,
            followers: options.followers ?? false,
            firstHop: true,
          },
          this.internalMetadata(),
        )
        .pipe(toHttpException()),
    );
    return this.parseValue(value.valueJson);
  }

  /** The queue's journal after a cursor, routed like any other read. */
  async readJournal(
    project: Projects,
    className: string,
    name: string,
    since: number,
  ): Promise<Record<string, unknown> | unknown[] | null> {
    const value = await lastValueFrom(
      this.workers
        .readJournal(
          {
            scopeId: project.id,
            class: className,
            name,
            since,
            firstHop: true,
          },
          this.internalMetadata(),
        )
        .pipe(toHttpException()),
    );
    return this.parseValue(value.valueJson);
  }

  /** One method call through the worker's data plane; lands on any node
   * and forwards once to the holder. A method failure is the object's
   * own user-safe error and surfaces as a 400. */
  async dispatchObject(
    project: Projects,
    className: string,
    name: string,
    method: string,
    args: unknown[],
  ): Promise<unknown> {
    // Internal platform verbs (stream delivery, hook invocation) are
    // worker-originated only; nothing the api serves may mint them.
    if (method.startsWith('__')) {
      throw new Error(`'${method}' is a platform verb.`);
    }
    const result = await lastValueFrom(
      this.workers
        .dispatch(
          {
            scopeId: project.id,
            class: className,
            name,
            method,
            argumentsJson: JSON.stringify(args),
            firstHop: true,
          },
          this.internalMetadata(),
        )
        .pipe(toHttpException()),
    );
    if (result.error) {
      throw new BadRequestException(result.error);
    }
    return this.parseValue(result.resultJson);
  }

  /** One overview read mapped onto the DTO, whatever class owns the file. */
  async overviewOf(
    project: Projects,
    className: string,
    name: string,
  ): Promise<DatabaseOverviewDto> {
    const overview = (await this.workerRead(project, className, name)) as {
      size_bytes?: number;
      tables?: {
        name: string;
        rows: number;
        columns?: {
          name: string;
          type: string;
          not_null: boolean;
          primary_key: boolean;
        }[];
      }[];
    } | null;
    return {
      sizeBytes: overview?.size_bytes ?? 0,
      tables: (overview?.tables ?? []).map((table) => ({
        name: table.name,
        rows: table.rows,
        columns: (table.columns ?? []).map((column) => ({
          name: column.name,
          type: column.type,
          notNull: column.not_null,
          primaryKey: column.primary_key,
        })),
      })),
    };
  }

  /** The union listing: declared by live contracts, present in the
   * directory, or both. */
  async listResources(
    project: Projects,
    kind: keyof typeof CLASSES,
  ): Promise<ResourceInstanceDto[]> {
    const page = await lastValueFrom(
      this.scripts
        .listScripts({ projectId: project.id, pageSize: 500, page: 1 })
        .pipe(toHttpException()),
    );
    const scripts = page.scripts || [];
    const identifiers = new Map(
      scripts.map((script) => [script.id, script.publicIdentifier]),
    );

    // Identity is the name alone; a name several scripts declare is one
    // resource, and the first declarer (stable script order) fills the
    // "declared by" chip.
    const declared = new Map<string, ResourceInstanceDto>();
    const contracts = await Promise.all(
      scripts
        .filter((script) => script.currentRevisionId)
        .map(async (script) => {
          const revision = await lastValueFrom(
            this.scripts
              .getRevision({
                id: script.currentRevisionId,
                withBundle: false,
                manifestOnly: false,
              })
              .pipe(toHttpException()),
          );
          const capabilities = revision.scriptConfig?.capabilities;
          return {
            script,
            names: capabilities?.[kind] ?? [],
            // A queue's consumer (`on "queue:<name>"`) outranks its
            // producers for the "declared by" chip, mirroring owner
            // resolution.
            consumed:
              kind === 'queues'
                ? (capabilities?.events ?? [])
                    .filter((event) => event.startsWith('queue:'))
                    .map((event) => event.slice('queue:'.length))
                : [],
          };
        }),
    );
    const sorted = contracts.sort((a, b) =>
      a.script.id.localeCompare(b.script.id),
    );
    for (const { script, names } of sorted) {
      for (const name of names) {
        if (!declared.has(name)) {
          declared.set(name, {
            name,
            declaredBy: script.publicIdentifier,
            orphaned: false,
          });
        }
      }
    }
    for (const { script, consumed } of sorted) {
      for (const name of consumed) {
        declared.set(name, {
          name,
          declaredBy: script.publicIdentifier,
          orphaned: false,
        });
      }
    }

    const directory = await lastValueFrom(
      this.registry
        .listInstances({
          projectIds: [project.id],
          class: CLASSES[kind],
          pageSize: 500,
        })
        .pipe(toHttpException()),
    );
    for (const instance of directory.instances || []) {
      if (instance.class !== CLASSES[kind]) continue;
      if (!declared.has(instance.name)) {
        declared.set(instance.name, {
          name: instance.name,
          declaredBy: identifiers.get(instance.scriptId) ?? '',
          orphaned: true,
        });
      }
    }

    return [...declared.values()].sort((a, b) => a.name.localeCompare(b.name));
  }
}
