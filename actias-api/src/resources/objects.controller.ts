import {
  BadGatewayException,
  BadRequestException,
  Body,
  Controller,
  Delete,
  Get,
  Param,
  Post,
  Query,
  Logger,
  UseGuards,
} from '@nestjs/common';
import { ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Principal } from 'src/auth/user.decorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { ResourcesService, clampPageSize } from './resources.service';
import {
  ClassCountDto,
  DatabaseOverviewDto,
  DeleteOutcomeDto,
  DirectoryPageDto,
  DirectoryQueryDto,
  DirectoryRebuiltDto,
  ObjectCallDto,
  ObjectCallResultDto,
  VisitPageDto,
  FollowerEdgeDto,
  FollowersDto,
  ObjectPageDto,
  SqlQueryDto,
  SqlRowsDto,
  StateDto,
} from './dto/resources.dto';

/**
 * A project's durable object instances, user classes only: the
 * directory that outlives the contracts that declared it. One family of
 * the backplane surface (`/project/:id/objects`).
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('objects')
@Controller('project/:project/objects')
export class ObjectsController {
  private readonly logger = new Logger(ObjectsController.name);

  constructor(private readonly resources: ResourcesService) {}

  /** Instances the directory knows of one class, filterable by name
   * prefix, always paged, because a per-user class holds one instance
   * per user. The classes come from the counts endpoint. */
  @Get()
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiQuery({ name: 'class', required: true, type: String })
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
    if (!className) {
      throw new BadRequestException(
        'A listing names its class; the classes endpoint lists them.',
      );
    }
    const scripts = await lastValueFrom(
      this.resources.scripts
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
      this.resources.registry
        .listInstances({
          projectIds: [project.id],
          class: className,
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
          createdMs: Number(instance.createdMs ?? 0),
          expireAtMs: Number(instance.expireAtMs ?? 0),
          deletedAtMs: Number(instance.deletedAtMs ?? 0),
          alarmDueMs: Number(instance.alarmDueMs ?? 0),
          nodeId: instance.nodeId ?? '',
        })),
      total: Number(directory.total ?? 0),
    };
  }

  /** Deletion is forget: storage, snapshot and edges are reclaimed,
   * the name may be recreated later and starts fresh, and there is no
   * undo. This tombstones; the janitor finishes within a sweep. */
  @Delete(':class/:name')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiParam({ name: 'class', type: 'string' })
  @ApiParam({ name: 'name', type: 'string' })
  async deleteObject(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
  ): Promise<DeleteOutcomeDto> {
    if (className.startsWith('__')) {
      throw new BadGatewayException('Platform classes are not deletable.');
    }
    const outcome = await lastValueFrom(
      this.resources.registry
        .deleteInstance({
          scopeId: project.id,
          class: className,
          name,
          objectId: '',
          onlyIfExpired: false,
        })
        .pipe(toHttpException()),
    );
    return { deleting: outcome.tombstoned ? 1 : 0 };
  }

  /** Every instance of one class, for dev cleanup; pages through the
   * directory and tombstones each row. */
  @Delete(':class')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiParam({ name: 'class', type: 'string' })
  async deleteClass(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
  ): Promise<DeleteOutcomeDto> {
    if (className.startsWith('__')) {
      throw new BadGatewayException('Platform classes are not deletable.');
    }
    let deleting = 0;
    loop: for (;;) {
      const page = await lastValueFrom(
        this.resources.registry
          .listInstances({
            projectIds: [project.id],
            class: className,
            namePrefix: '',
            pageSize: 200,
            page: 0,
          })
          .pipe(toHttpException()),
      );
      const live = (page.instances || []).filter(
        (instance) => !Number(instance.deletedAtMs ?? 0),
      );
      if (live.length === 0) {
        break loop;
      }
      for (const instance of live) {
        const outcome = await lastValueFrom(
          this.resources.registry
            .deleteInstance({
              scopeId: project.id,
              class: className,
              name: instance.name,
              objectId: '',
              onlyIfExpired: false,
            })
            .pipe(toHttpException()),
        );
        if (outcome.tombstoned) {
          deleting += 1;
        }
      }
      if ((page.instances || []).length < 200) {
        break loop;
      }
    }
    return { deleting };
  }

  /** How many instances each user class holds: what the rail renders
   * before anyone asks for names. */
  @Get('counts')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async countObjects(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ClassCountDto[]> {
    const counted = await lastValueFrom(
      this.resources.registry
        .countInstances({ projectIds: [project.id] })
        .pipe(toHttpException()),
    );
    // Platform classes have their own sections; the object family is
    // user classes alone.
    const declared = await this.resources.classDeclarations(project);
    return (counted.counts || [])
      .filter((row) => !row.class.startsWith('__'))
      .map((row) => ({
        class: row.class,
        count: Number(row.count ?? 0),
        hasDirectory: declared.get(row.class)?.directory ?? false,
        directoryFields: declared.get(row.class)?.fields ?? [],
        methods: declared.get(row.class)?.methods ?? [],
      }));
  }

  /**
   * One method call on one instance, as a script would make it: through
   * the object's own lane, serialized with every other call, directory
   * derivation and alarms running as they always do. Never a side
   * channel into the file.
   *
   * This is the shell's write mode, and a person touching live data,
   * so every call is logged against the account that made it. Naming
   * an instance that does not exist creates it, admission permitting,
   * exactly as in a script.
   */
  @Post(':class/:name/call')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectCall(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
    @Body() body: ObjectCallDto,
    @Principal()
    principal: {
      user?: { id: string; username?: string };
      serviceToken?: { id: string; name?: string };
    },
  ): Promise<ObjectCallResultDto> {
    this.refusePlatformClass(className);
    // Whoever authenticated: a person's session or a project's service
    // token. Logged either way; a shell session is live data touched
    // by hand.
    const who = principal.user
      ? principal.user.username ?? principal.user.id
      : `service token ${
          principal.serviceToken?.name ?? principal.serviceToken?.id ?? ''
        }`;
    this.logger.log(
      `shell call by ${who} on ${project.id}: ${className}("${name}"):${body.method}`,
    );
    const value = await this.resources.dispatchObject(
      project,
      className,
      name,
      body.method,
      body.args ?? [],
    );
    return { valueJson: JSON.stringify(value ?? null) };
  }

  /** Overview of one durable object's private storage; a user class's
   * file is a SQLite database like any other. */
  @Get(':class/:name/overview')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectOverview(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
  ): Promise<DatabaseOverviewDto> {
    this.refusePlatformClass(className);
    return this.resources.overviewOf(project, className, name);
  }

  /** The edges other things hold on this object: who follows it, on
   * which topic, with what filter, and how far behind the publisher's
   * event log each durable edge sits. Runtime state, never contract. */
  @Get(':class/:name/followers')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectFollowers(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
  ): Promise<FollowersDto> {
    this.refusePlatformClass(className);
    const value = (await this.resources.workerRead(project, className, name, {
      followers: true,
    })) as {
      head?: number;
      edges?: Record<string, unknown>[];
    } | null;
    return {
      head: Number(value?.head ?? 0),
      edges: (value?.edges ?? []).map(
        (edge): FollowerEdgeDto => ({
          kind: String(edge.kind ?? ''),
          follower: String(edge.follower ?? ''),
          connection: edge.connection == null ? null : String(edge.connection),
          topic: String(edge.topic ?? ''),
          filter: (edge.filter as Record<string, unknown> | null) ?? null,
          cursor: Number(edge.cursor ?? 0),
          lag: edge.lag == null ? null : Number(edge.lag),
          attempts: Number(edge.attempts ?? 0),
          nextAt: Number(edge.next_at ?? 0),
        }),
      ),
    };
  }

  /** The object's key-value state pairs, in key order. The reserved
   * table is denied to SQL from every direction, so this typed read is
   * the console's only window on the store face. Read-only: writes go
   * through the object's methods, like everything else it keeps. */
  @Get(':class/:name/state')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectState(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
  ): Promise<StateDto> {
    this.refusePlatformClass(className);
    const value = await this.resources.workerRead(project, className, name, {
      state: true,
    });
    return {
      entries: (Array.isArray(value) ? value : []).map((pair) => {
        const row = pair as Record<string, unknown>;
        return {
          key: String(row.key ?? ''),
          type: String(row.type ?? ''),
          value: String(row.value ?? ''),
        };
      }),
    };
  }

  /** A read-only query against one object's storage, from the nearest
   * copy; the script-guard authorizer applies, so reserved tables stay
   * out of reach. Writes only ever happen through the object's methods. */
  @Post(':class/:name/query')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectQuery(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    this.refusePlatformClass(className);
    const rows = await this.resources.workerRead(project, className, name, {
      sql: body.sql,
    });
    return { rows: Array.isArray(rows) ? rows : [] };
  }

  /** One page of the class's directory: the row every object in it
   * contributes, answered without waking any of them.
   *
   * A POST because the predicate is a tree, not a query string. The
   * rows are each object's last saved write, so a listing decides
   * which objects to call and never substitutes for calling one.
   */
  @Post(':class/directory')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectDirectory(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Body() body: DirectoryQueryDto,
  ): Promise<DirectoryPageDto> {
    this.refusePlatformClass(className);
    const page = await this.resources.listDirectory(project, className, body);
    return {
      entries: (page.entries ?? []).map((entry) => ({
        name: entry.name,
        objectId: entry.objectId,
        fields: entry.fields ?? {},
      })),
      cursor: page.cursor,
      building: page.building ?? [],
    };
  }

  /**
   * Rebuilds the class's index from what still exists: the placement
   * store's live identities, and each object's shipping manifest.
   *
   * The operator's path for damage the background pass cannot reach.
   * That pass finds classes by listing the blob store, so a class
   * whose prefix is gone entirely is invisible to it; a name can always
   * be asked for. Nothing is woken and no object file is opened, so
   * the cost is one small read per live object.
   */
  @Post(':class/directory/rebuild')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectDirectoryRebuild(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
  ): Promise<DirectoryRebuiltDto> {
    this.refusePlatformClass(className);
    const rebuilt = await this.resources.rebuildDirectory(project, className);
    return {
      live: Number(rebuilt.live ?? 0),
      rows: Number(rebuilt.rows ?? 0),
      withoutRow: Number(rebuilt.withoutRow ?? 0),
      tombstones: Number(rebuilt.tombstones ?? 0),
      held: rebuilt.held ?? false,
    };
  }

  /**
   * The verified read over a class's directory. Same query as the
   * listing; every candidate is checked against its object's settled
   * state before it is served, so stale rows drop, fresher rows arrive
   * fresh, and the uncheckable come back flagged rather than missing.
   */
  @Post(':class/directory/visit')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async objectDirectoryVisit(
    @EntityParam('project', Projects) project: Projects,
    @Param('class') className: string,
    @Body() body: DirectoryQueryDto,
  ): Promise<VisitPageDto> {
    this.refusePlatformClass(className);
    const page = await this.resources.visitDirectory(project, className, body);
    return {
      entries: (page.entries ?? []).map((served) => ({
        entry: {
          name: served.entry?.name ?? '',
          objectId: served.entry?.objectId ?? '',
          fields: served.entry?.fields ?? {},
        },
        unverified: served.unverified ?? false,
        reason: served.reason || undefined,
      })),
      cursor: page.cursor,
      building: page.building ?? [],
    };
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
}
