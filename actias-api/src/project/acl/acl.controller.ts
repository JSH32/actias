import {
  Body,
  Controller,
  Get,
  Put,
  UseGuards,
} from '@nestjs/common/decorators';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { Acl, AclGuard } from './acl.guard';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclService } from './acl.service';
import { ACCESS_KEYS, AccessFields, getListFromBitfield } from './accessFields';
import { Projects } from 'src/entities/Projects';
import { EntityParam } from 'src/util/entitydecorator';
import { Users } from 'src/entities/Users';
import { AclListDto } from './dto/acl.dto';
import { User } from 'src/auth/user.decorator';

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
  @Acl(AccessFields.PERMISSIONS_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
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
  @Get()
  @Acl(AccessFields.PERMISSIONS_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  getAcl(@EntityParam('project', Projects) project: Projects): AclListDto[] {
    return project.access.map(
      (a) =>
        new AclListDto({
          userId: a.user.id,
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
  @Acl(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  async putAcl(
    @EntityParam('user', Users) user: Users,
    @EntityParam('project', Projects) project: Projects,
    @Body() permissions: string[],
  ) {
    return new AclListDto({
      userId: user.id,
      permissions: getListFromBitfield(
        this.aclService.setPermissions(project, user, permissions),
      ),
    });
  }
}
