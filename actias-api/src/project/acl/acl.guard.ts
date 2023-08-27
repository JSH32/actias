import {
  CanActivate,
  ExecutionContext,
  ForbiddenException,
  Injectable,
  SetMetadata,
} from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { AclService } from './acl.service';
import { ModuleRef, Reflector } from '@nestjs/core';
import { EntityManager } from '@mikro-orm/core';
import { Projects } from 'src/entities/Projects';

export const AclByProject = (bitfield: number) =>
  SetMetadata('acl', { bitfield });

export const AclByFinder = (
  bitfield: number,
  projectFinder:
    | ((request: any, em: EntityManager) => Promise<Projects>)
    | string,
) => SetMetadata('acl', { bitfield, projectFinder });

@Injectable()
export class AclGuard implements CanActivate {
  constructor(
    private readonly aclService: AclService,
    // Not using project service to avoid cyclic dependency.
    private readonly em: EntityManager,
    private readonly reflector: Reflector,
    private readonly moduleRef: ModuleRef,
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();

    const aclData = this.reflector.get<any>('acl', context.getHandler());

    if (aclData?.bitfield) {
      // Should be provided by AuthGuard
      const user = request['user'] as Users;

      let project;

      if (aclData.projectFinder != null) {
        if (
          typeof aclData.projectFinder === 'string' ||
          aclData.projectFinder instanceof String
        ) {
          // Create instance and run init hook.
          const instance = await this.moduleRef.create(context.getClass());
          instance.onModuleInit();

          project = await instance[aclData.projectFinder](request, this.em);
        } else {
          project = await aclData.projectFinder(request, this.em);
        }
      } else {
        project = await this.em.findOneOrFail(
          Projects,
          request.params['project'],
        );
      }

      if (project.owner === user) return true;

      const access = await this.aclService.getProjectAccess(user, project);

      if (!access.test(aclData.bitfield)) {
        throw new ForbiddenException({
          message: 'You do not have enough permissions to perform this action',
        });
      }
    }

    return true;
  }
}
