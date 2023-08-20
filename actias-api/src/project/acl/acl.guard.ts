import {
  CanActivate,
  ExecutionContext,
  ForbiddenException,
  Injectable,
  SetMetadata,
} from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { AclService } from './acl.service';
import { Reflector } from '@nestjs/core';
import { EntityManager } from '@mikro-orm/core';
import { Projects } from 'src/entities/Projects';
import { Resources } from 'src/entities/Resources';

export const AclByResource = (bitfield: number, resourceParam: string) =>
  SetMetadata('acl', { bitfield, resourceParam });

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
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();

    const aclData = this.reflector.get<any>('acl', context.getHandler());

    if (aclData?.bitfield) {
      // Should be provided by AuthGuard
      const user = request['user'] as Users;

      let project;

      if (aclData.projectFinder != null) {
        if (aclData.projectFinder instanceof String) {
          const finderMethod = this.reflector.get<string>(
            aclData.projectFinder,
            context.getHandler(),
          ) as any;

          project = await finderMethod(request, this.em);
        } else {
          project = await aclData.projectFinder(request, this.em);
        }
      } else if (aclData.resourceParam != null) {
        // Get the project by the resource ID if resourceParam is provided.
        const id = request.params[aclData.resourceParam];
        const resource = await this.em.findOneOrFail(
          Resources,
          { id },
          { populate: ['project'] },
        );

        project = resource.project;
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
