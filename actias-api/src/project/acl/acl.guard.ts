import {
  CanActivate,
  ExecutionContext,
  ForbiddenException,
  Injectable,
  SetMetadata,
} from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { ServiceTokens } from 'src/entities/ServiceTokens';
import { AclService } from './acl.service';
import { ModuleRef, Reflector } from '@nestjs/core';
import { EntityManager } from '@mikro-orm/core';
import { Projects } from 'src/entities/Projects';
import { WsException } from '@nestjs/websockets';
import { requireUuid } from 'src/util/entitydecorator';

export const AclByProject = (bitfield: number) =>
  SetMetadata('acl', { bitfield });

/**
 * Membership alone, without naming a permission: for routes that
 * describe the project itself rather than one of its resources. A
 * member holding any grant passes, a stranger does not.
 *
 * Every project route needs one of these decorators. A route carrying
 * no acl metadata is open to every authenticated caller, so this is
 * what the weakest requirement looks like when written down.
 */
export const AclMember = () => SetMetadata('acl', { bitfield: 0 });

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
    const isWs = this.reflector.get<boolean>(
      'websocketAuth',
      context.getHandler(),
    );

    const ExceptionClass = isWs ? WsException : ForbiddenException;

    // Presence of the metadata is what arms the check; a zero bitfield
    // still demands membership (see AclMember).
    if (aclData) {
      // Should be provided by AuthGuard
      const user = request['user'] as Users;

      let project;

      if (aclData.projectFinder != null) {
        if (
          typeof aclData.projectFinder === 'string' ||
          aclData.projectFinder instanceof String
        ) {
          const instance = await this.moduleRef.create(context.getClass());
          instance.onModuleInit();

          project = await instance[aclData.projectFinder](request, this.em);
        } else {
          project = await aclData.projectFinder(request, this.em);
        }
      } else {
        // Guards run before pipes, so the EntityPipe's shape check has
        // not happened yet; a malformed id refuses here instead of
        // surfacing as a database error.
        project = await this.em.findOneOrFail(
          Projects,
          requireUuid(request.params['project']),
        );
      }

      const serviceToken = request['serviceToken'] as ServiceTokens | undefined;
      if (!serviceToken && project.owner === user) return true;

      const access = await this.aclService.getPrincipalAccess(
        { user, serviceToken },
        project,
      );

      if (aclData.bitfield && !access.test(aclData.bitfield)) {
        throw new ExceptionClass({
          message: 'You do not have enough permissions to perform this action',
        });
      }
    }

    return true;
  }
}
