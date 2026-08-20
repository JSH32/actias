import {
  BadGatewayException,
  Body,
  Controller,
  Get,
  Inject,
  Param,
  Post,
} from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { ClientGrpc } from '@nestjs/microservices';
import { ConfigService } from '@nestjs/config';
import { lastValueFrom } from 'rxjs';
import { AclByProject } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { node_registry } from 'src/protobufs/node_registry';
import {
  QueueStatsDto,
  ResourceInstanceDto,
  SqlQueryDto,
  SqlRowsDto,
  TableInfoDto,
} from './dto/resources.dto';

/** The platform class each resource kind rides on. */
const CLASSES = { queues: '__queue', databases: '__database' } as const;

/**
 * A project's object-backed resources: queues and sql databases. Listings
 * are the union of what live contracts declare and what the instance
 * directory records, so data outlives the revision that declared it;
 * numbers come from the worker's typed platform reads.
 */
@ApiTags('resources')
@Controller('project/:project/resources')
export class ResourcesController {
  private scripts: script_service.ScriptService;
  private registry: node_registry.NodeRegistryService;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly scriptClient: ClientGrpc,
    @Inject('NODE_REGISTRY') private readonly registryClient: ClientGrpc,
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

  @Get('queues/:script/:name/stats')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async queueStats(
    @EntityParam('project', Projects) project: Projects,
    @Param('script') script: string,
    @Param('name') name: string,
  ): Promise<QueueStatsDto> {
    const stats = (await this.workerStats(script, CLASSES.queues, name)) as {
      depth?: number;
      oldest_pending?: number;
      dead_letters?: number;
    } | null;
    return {
      depth: stats?.depth ?? 0,
      oldestPending: stats?.oldest_pending ?? undefined,
      deadLetters: stats?.dead_letters ?? 0,
    };
  }

  @Get('databases/:script/:name/tables')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async databaseTables(
    @EntityParam('project', Projects) project: Projects,
    @Param('script') script: string,
    @Param('name') name: string,
  ): Promise<TableInfoDto[]> {
    const overview = await this.workerStats(script, CLASSES.databases, name);
    return Array.isArray(overview) ? (overview as TableInfoDto[]) : [];
  }

  /**
   * Runs a read-only query from the nearest copy, bounded staleness by
   * design; the script-guard authorizer applies exactly as it does to
   * script sql.
   */
  @Post('databases/:script/:name/query')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async query(
    @EntityParam('project', Projects) project: Projects,
    @Param('script') script: string,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    return this.dispatchSql(script, name, 'read', body);
  }

  /** Executes a statement through the owner, transactional, single-writer. */
  @Post('databases/:script/:name/execute')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async execute(
    @EntityParam('project', Projects) project: Projects,
    @Param('script') script: string,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    return this.dispatchSql(script, name, 'query', body);
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

    const declared = new Map<string, ResourceInstanceDto>();
    await Promise.all(
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
          for (const name of revision.scriptConfig?.capabilities?.[kind] ??
            []) {
            declared.set(`${script.id}/${name}`, {
              name,
              scriptId: script.id,
              scriptIdentifier: script.publicIdentifier,
              orphaned: false,
            });
          }
        }),
    );

    const directory = await lastValueFrom(
      this.registry
        .listInstances({ scriptIds: scripts.map((script) => script.id) })
        .pipe(toHttpException()),
    );
    const identifiers = new Map(
      scripts.map((script) => [script.id, script.publicIdentifier]),
    );
    for (const instance of directory.instances || []) {
      if (instance.class !== CLASSES[kind]) continue;
      const key = `${instance.scriptId}/${instance.name}`;
      if (!declared.has(key)) {
        declared.set(key, {
          name: instance.name,
          scriptId: instance.scriptId,
          scriptIdentifier: identifiers.get(instance.scriptId) ?? '',
          orphaned: true,
        });
      }
    }

    return [...declared.values()].sort((a, b) => a.name.localeCompare(b.name));
  }

  /** One typed platform read off the worker's local file or replica. */
  private async workerStats(
    scriptId: string,
    className: string,
    name: string,
  ): Promise<Record<string, unknown> | unknown[] | null> {
    const base = this.config.get<string>('worker.internalUrl');
    const url = `${base}/_platform/stats?script=${encodeURIComponent(
      scriptId,
    )}&class=${encodeURIComponent(className)}&name=${encodeURIComponent(name)}`;

    const response = await fetch(url, {
      headers: {
        'x-actias-internal': this.config.get<string>('worker.internalToken'),
      },
    }).catch(() => null);
    if (!response || !response.ok) {
      throw new BadGatewayException('The worker did not answer the read.');
    }
    return response.json();
  }

  /** One sql call through the worker's object transport. */
  private async dispatchSql(
    scriptId: string,
    database: string,
    method: 'read' | 'query',
    body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    const base = this.config.get<string>('worker.internalUrl');
    const response = await fetch(`${base}/_object`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-actias-internal': this.config.get<string>('worker.internalToken'),
      },
      body: JSON.stringify({
        script_id: scriptId,
        class: '__database',
        name: database,
        method,
        arguments: [body.sql, body.params ?? []],
      }),
    }).catch(() => null);
    if (!response) {
      throw new BadGatewayException('The worker did not answer the query.');
    }
    const answer = await response.json().catch(() => null);
    if (!response.ok) {
      throw new BadGatewayException(
        typeof answer === 'string' ? answer : 'The query failed.',
      );
    }
    return { rows: Array.isArray(answer) ? answer : [answer] };
  }
}
