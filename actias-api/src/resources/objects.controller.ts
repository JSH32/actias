import {
  BadGatewayException,
  Body,
  Controller,
  Get,
  Param,
  Post,
  Query,
} from '@nestjs/common';
import { ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { AclByProject } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { ResourcesService, clampPageSize } from './resources.service';
import {
  ClassCountDto,
  DatabaseOverviewDto,
  FollowerEdgeDto,
  FollowersDto,
  ObjectPageDto,
  SqlQueryDto,
  SqlRowsDto,
} from './dto/resources.dto';

/**
 * A project's durable object instances, user classes only: the
 * directory that outlives the contracts that declared it. One family of
 * the backplane surface (`/project/:id/objects`).
 */
@ApiTags('objects')
@Controller('project/:project/objects')
export class ObjectsController {
  constructor(private readonly resources: ResourcesService) {}

  /** Instances the directory knows, filterable by class and name
   * prefix, always paged, because a per-user class holds one instance
   * per user. */
  @Get()
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
    return (counted.counts || [])
      .filter((row) => !row.class.startsWith('__'))
      .map((row) => ({ class: row.class, count: Number(row.count ?? 0) }));
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
