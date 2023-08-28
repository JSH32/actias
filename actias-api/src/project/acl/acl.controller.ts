import {
  Body,
  Controller,
  Get,
  Put,
  UseGuards,
} from '@nestjs/common/decorators';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { AclByProject, AclGuard } from './acl.guard';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclService } from './acl.service';
import { ACCESS_KEYS, AccessFields, getListFromBitfield } from './accessFields';
import { Projects } from 'src/entities/Projects';
import { EntityParam } from 'src/util/entitydecorator';
import { Users } from 'src/entities/Users';
import { AclListDto } from './dto/acl.dto';
import { User } from 'src/auth/user.decorator';
import { UserDto } from 'src/users/dto/user.dto';

@ApiTags('acl')
@Controller('acl')
export class AclInfoController {
  /**
   * Get a list of all permissions.
   */
  @Get('permissions')
  getPermissions() {
    return ACCESS_KEYS;
  }
}

@UseGuards(AuthGuard, AclGuard)
@ApiTags('acl')
@Controller('project/:project/acl')
export class AclController {
  constructor(private readonly aclService: AclService) {}

  /**
   * Get ACL list for the current authorized user.
   * This doesn't require the `PERMISSIONS_READ` permission.
   */
  @Get('@me')
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  getAclMe(
    @EntityParam('project', Projects) project: Projects,
    @User() user: Users,
  ) {
    return this.aclService.getAclList(user, project);
  }

  /**
   * Get ACL list for a single user.
   */
  @Get(':user')
  @AclByProject(AccessFields.PERMISSIONS_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  @ApiParam({
    name: 'user',
    schema: { type: 'string' },
    type: 'string',
  })
  getAclSingle(
    @EntityParam('project', Projects) project: Projects,
    @EntityParam('user', Users) user: Users,
  ) {
    return this.aclService.getAclList(user, project);
  }

  /**
   * Get ACL list for all users.
   */
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  @Get()
  @AclByProject(AccessFields.PERMISSIONS_READ)
  async getAcl(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<AclListDto[]> {
    return (await project.access.loadItems()).map(
      (a) =>
        new AclListDto({
          user: new UserDto(a.user),
          permissions: getListFromBitfield(a.permissionBitfield),
        }),
    );
  }

  /**
   * Set ACL access for the project for the user.
   * This will implicitly add the user to the project.
   * If `permissions` is empty then the user will be removed from the project.
   */
  @Put(':user')
  @AclByProject(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  @ApiParam({
    name: 'user',
    schema: { type: 'string' },
    type: 'string',
  })
  async putAcl(
    @EntityParam('user', Users) user: Users,
    @EntityParam('project', Projects) project: Projects,
    @Body() permissions: string[],
  ) {
    return new AclListDto({
      user: new UserDto(user),
      permissions: getListFromBitfield(
        await this.aclService.setPermissions(project, user, permissions),
      ),
    });
  }
}
