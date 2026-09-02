import { Body, Controller, Get, Param, Post, UseGuards } from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { CLASSES, ResourcesService } from './resources.service';
import {
  DatabaseOverviewDto,
  ResourceInstanceDto,
  SqlQueryDto,
  SqlRowsDto,
} from './dto/resources.dto';

/**
 * A project's sql databases: single-writer durable objects whose file
 * any node can read. One family of the backplane surface
 * (`/project/:id/databases`).
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('databases')
@Controller('project/:project/databases')
export class DatabasesController {
  constructor(private readonly resources: ResourcesService) {}

  @Get()
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listDatabases(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ResourceInstanceDto[]> {
    return this.resources.listResources(project, 'databases');
  }

  @Get(':name/overview')
  @AclByProject(AccessFields.DATABASE_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async databaseOverview(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<DatabaseOverviewDto> {
    return this.resources.overviewOf(project, CLASSES.databases, name);
  }

  /**
   * Runs a read-only query from the nearest copy, bounded staleness by
   * design; the script-guard authorizer applies exactly as it does to
   * script sql.
   */
  @Post(':name/query')
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
  @Post(':name/execute')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async execute(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Body() body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    return this.dispatchSql(project, name, 'query', body);
  }

  private async dispatchSql(
    project: Projects,
    database: string,
    method: 'read' | 'query',
    body: SqlQueryDto,
  ): Promise<SqlRowsDto> {
    const rows = await this.resources.dispatchObject(
      project,
      CLASSES.databases,
      database,
      method,
      [body.sql, body.params ?? []],
    );
    return { rows: Array.isArray(rows) ? rows : rows == null ? [] : [rows] };
  }
}
