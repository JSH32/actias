import {
  BadGatewayException,
  BadRequestException,
  Body,
  Controller,
  Get,
  Inject,
  Param,
  Post,
  Query,
} from '@nestjs/common';
import { ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { ClientGrpc } from '@nestjs/microservices';
import { ConfigService } from '@nestjs/config';
import { Metadata } from '@grpc/grpc-js';
import { lastValueFrom } from 'rxjs';
import { AclByProject } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { node_registry } from 'src/protobufs/node_registry';
import { worker_data } from 'src/protobufs/worker_data';
import {
  ClassCountDto,
  DatabaseOverviewDto,
  ObjectPageDto,
  QueueEventDto,
  QueueMessageDto,
  QueueStatsDto,
  ResourceInstanceDto,
  RetriedDto,
  SqlQueryDto,
  SqlRowsDto,
} from './dto/resources.dto';

/** The platform class each resource kind rides on. */
const CLASSES = { queues: '__queue', databases: '__database' } as const;

/** Rows one directory page may carry; larger asks clamp, never error. */
export function clampPageSize(requested?: number): number {
  if (!requested || requested <= 0 || Number.isNaN(requested)) return 100;
  return Math.min(Math.floor(requested), 500);
}

/**
 * A project's object-backed resources: queues and sql databases. Identity
 * is project-scoped ((project, class, name)); which script's code an
 * object runs is the data plane's business, so reads and calls carry the
 * project, never a script. Listings are the union of what live contracts
 * declare and what the instance directory records, so data outlives the
 * revision that declared it.
 */
@ApiTags('resources')
@Controller('project/:project/resources')
export class ResourcesController {
  private scripts: script_service.ScriptService;
  private registry: node_registry.NodeRegistryService;
  private workers: worker_data.WorkerData;

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
  private internalMetadata(): Metadata {
    const metadata = new Metadata();
    metadata.set(
      'x-actias-internal',
      this.config.get<string>('worker.internalToken'),
    );
    return metadata;
  }

  @Get('queues')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listQueues(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ResourceInstanceDto[]> {
    return this.listResources(project, 'queues');
  }

  @Get('databases')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listDatabases(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ResourceInstanceDto[]> {
    return this.listResources(project, 'databases');
  }

  @Get('queues/:name/stats')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async queueStats(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<QueueStatsDto> {
    const stats = (await this.workerRead(project, CLASSES.queues, name)) as {
      depth?: number;
      in_flight?: number;
      oldest_pending?: number;
      dead_letters?: number;
    } | null;
    return {
      depth: stats?.depth ?? 0,
      inFlight: stats?.in_flight ?? 0,
      oldestPending: stats?.oldest_pending ?? undefined,
      deadLetters: stats?.dead_letters ?? 0,
    };
  }

  /** Live and dead message rows, newest first; delivered messages are in
   * the journal. */
  @Get('queues/:name/messages')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async queueMessages(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<QueueMessageDto[]> {
    const rows = (await this.workerRead(project, CLASSES.queues, name, {
      messages: true,
    })) as Record<string, unknown>[] | null;
    return (Array.isArray(rows) ? rows : []).map((row) => ({
      id: Number(row.id),
      state: String(row.state ?? ''),
      attempts: Number(row.attempts ?? 0),
      preview: String(row.preview ?? ''),
      size: Number(row.size ?? 0),
      enqueuedMs: Number(row.enqueued_ms ?? 0),
      nextMs: row.next_ms == null ? undefined : Number(row.next_ms),
      diedMs: row.died_ms == null ? undefined : Number(row.died_ms),
    }));
  }

  /** Requeues every dead letter; they start their attempts over. */
  @Post('queues/:name/retry-dead')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async retryDead(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<RetriedDto> {
    const count = await this.dispatchObject(
      project,
      CLASSES.queues,
      name,
      'retry_dead',
      [],
    );
    return { requeued: Number(count ?? 0) };
  }

  /** Requeues one dead letter by id. */
  @Post('queues/:name/messages/:id/retry')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async retryMessage(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Param('id') id: string,
  ): Promise<RetriedDto> {
    const count = await this.dispatchObject(
      project,
      CLASSES.queues,
      name,
      'retry_message',
      [Number(id)],
    );
    return { requeued: Number(count ?? 0) };
  }

  /** Discards one message, live or dead. */
  @Post('queues/:name/messages/:id/drop')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async dropMessage(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Param('id') id: string,
  ): Promise<void> {
    await this.dispatchObject(project, CLASSES.queues, name, 'drop_message', [
      Number(id),
    ]);
  }

  /** Durable object instances the directory knows, user classes only;
   * filterable by class and name prefix, always paged, because a
   * per-user class holds one instance per user. */
  @Get('objects')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiQuery({ name: 'class', required: false, type: String })
  @ApiQuery({ name: 'prefix', required: false, type: String })
  @ApiQuery({ name: 'page', required: false, type: Number })
  @ApiQuery({ name: 'pageSize', required: false, type: Number })
  async listObjects(
    @EntityParam('project', Projects) project: Projects,
    @Query('class') className?: string,
    @Query('prefix') prefix?: string,
    @Query('page') page?: string,
    @Query('pageSize') pageSize?: string,
  ): Promise<ObjectPageDto> {
    const scripts = await lastValueFrom(
      this.scripts
        .listScripts({ projectId: project.id, pageSize: 500, page: 1 })
        .pipe(toHttpException()),
    );
    const identifiers = new Map(
      (scripts.scripts || []).map((script) => [
        script.id,
        script.publicIdentifier,
      ]),
    );

    const directory = await lastValueFrom(
      this.registry
        .listInstances({
          projectIds: [project.id],
          class: className ?? '',
          namePrefix: prefix ?? '',
          pageSize: clampPageSize(Number(pageSize)),
          page: Math.max(0, Math.floor(Number(page) || 0)),
        })
        .pipe(toHttpException()),
    );
    return {
      items: (directory.instances || [])
        .filter((instance) => !instance.class.startsWith('__'))
        .map((instance) => ({
          class: instance.class,
          name: instance.name,
          declaredBy: identifiers.get(instance.scriptId) ?? '',
        })),
      total: Number(directory.total ?? 0),
    };
  }

  /** How many instances each user class holds: what the rail renders
   * before anyone asks for names. */
  @Get('objects/counts')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async countObjects(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ClassCountDto[]> {
    const counted = await lastValueFrom(
      this.registry
        .countInstances({ projectIds: [project.id] })
        .pipe(toHttpException()),
    );
    // Platform classes have their own sections; the rail's object group
    // is user classes alone.
    return (counted.counts || [])
      .filter((row) => !row.class.startsWith('__'))
      .map((row) => ({ class: row.class, count: Number(row.count ?? 0) }));
  }

  /** The queue's journal after `since`: enqueued, delivered, retried and
   * dead-lettered, oldest first. */
  @Get('queues/:name/events')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiQuery({ name: 'since', required: false, type: Number })
  async queueEvents(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Query('since') since?: string,
  ): Promise<QueueEventDto[]> {
    const value = await lastValueFrom(
      this.workers
        .readJournal(
          {
            scopeId: project.id,
            class: CLASSES.queues,
            name,
            since: Number(since ?? 0),
            firstHop: true,
          },
          this.internalMetadata(),
        )
        .pipe(toHttpException()),
    );
    const events = this.parseValue(value.valueJson);
    return Array.isArray(events) ? (events as QueueEventDto[]) : [];
  }

  /** Overview of one durable object's private storage; a user class's
   * file is a SQLite database like any other. */
  @Get('objects/:class/:name/overview')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectOverview(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
  ): Promise<DatabaseOverviewDto> {
    this.refusePlatformClass(className);
    return this.overviewOf(project, className, name);
  }

  /** A read-only query against one object's storage, from the nearest
   * copy; the script-guard authorizer applies, so reserved tables stay
   * out of reach. Writes only ever happen through the object's methods. */
  @Post('objects/:class/:name/query')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectQuery(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    this.refusePlatformClass(className);
    const rows = await this.workerRead(project, className, name, {
      sql: body.sql,
    });
    return { rows: Array.isArray(rows) ? rows : [] };
  }

  /** Platform classes have their own typed endpoints; the generic object
   * read is for user classes alone. */
  private refusePlatformClass(className: string) {
    if (className.startsWith('__')) {
      throw new BadGatewayException(
        'Platform classes are read through their own endpoints.',
      );
    }
  }

  @Get('databases/:name/overview')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async databaseOverview(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<DatabaseOverviewDto> {
    return this.overviewOf(project, CLASSES.databases, name);
  }

  /** One overview read mapped onto the DTO, whatever class owns the file. */
  private async overviewOf(
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

  /**
   * Runs a read-only query from the nearest copy, bounded staleness by
   * design; the script-guard authorizer applies exactly as it does to
   * script sql.
   */
  @Post('databases/:name/query')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async query(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    return this.dispatchSql(project, name, 'read', body);
  }

  /** Executes a statement through the owner, transactional, single-writer. */
  @Post('databases/:name/execute')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async execute(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    return this.dispatchSql(project, name, 'query', body);
  }

  /** The union listing: declared by live contracts, present in the directory, or both. */
  private async listResources(
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

  /** One typed platform read over the data plane, answered from the
   * freshest copy the worker can reach (its file, the holder's, the
   * replica). With `sql`, one read-only statement instead of the class
   * overview; with `messages`, the queue's message rows. */
  private async workerRead(
    project: Projects,
    className: string,
    name: string,
    options: { sql?: string; messages?: boolean } = {},
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
            firstHop: true,
          },
          this.internalMetadata(),
        )
        .pipe(toHttpException()),
    );
    return this.parseValue(value.valueJson);
  }

  /** The json a read answered with; an empty or `null` value reads as
   * "no observable state yet". */
  private parseValue(
    json: string | undefined,
  ): Record<string, unknown> | unknown[] | null {
    if (!json) return null;
    try {
      return JSON.parse(json);
    } catch {
      throw new BadGatewayException('The worker answered garbage.');
    }
  }

  /** One method call through the worker's data plane; lands on any node
   * and forwards once to the holder. A method failure is the object's
   * own user-safe error and surfaces as a 400. */
  private async dispatchObject(
    project: Projects,
    className: string,
    name: string,
    method: string,
    args: unknown[],
  ): Promise<unknown> {
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

  private async dispatchSql(
    project: Projects,
    database: string,
    method: 'read' | 'query',
    body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    const rows = await this.dispatchObject(
      project,
      '__database',
      database,
      method,
      [body.sql, body.params ?? []],
    );
    return { rows: Array.isArray(rows) ? rows : rows == null ? [] : [rows] };
  }
}
