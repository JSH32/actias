import { Controller, Get, UseGuards } from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { ResourcesService } from './resources.service';
import { ConnectionDto } from './dto/resources.dto';

/**
 * A project's live connections: the sockets clients hold open to it
 * and the wires it opened outward, listed across every node. Runtime
 * state only; nothing here is declared by a contract.
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('connections')
@Controller('project/:project/connections')
export class ConnectionsController {
  constructor(private readonly resources: ResourcesService) {}

  @Get()
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listConnections(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ConnectionDto[]> {
    const rows = await this.resources.listConnections(project);
    return rows.map(
      (row): ConnectionDto => ({
        id: row.id,
        connectionClass: row.connectionClass,
        identity: `${row.class}/${row.name}`,
        direction: row.direction,
        peer: row.peer ? row.peer : null,
        node: row.node,
        scriptId: row.scriptId,
        openedAt: Number(row.openedAtMs ?? 0),
        status: row.status,
        follows: Number(row.follows ?? 0),
      }),
    );
  }
}
