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
import { EntityRepository } from '@mikro-orm/core';
import { Projects } from 'src/entities/Projects';
import { InjectRepository } from '@mikro-orm/nestjs';

export const Acl = (bitfield: number) => SetMetadata('acl', bitfield);

@Injectable()
export class AclGuard implements CanActivate {
  constructor(
    private aclService: AclService,
    // Not using project service to avoid cyclic dependency.
    @InjectRepository(Projects)
    private projectRepository: EntityRepository<Projects>,
    private reflector: Reflector,
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();

    const bitfield = this.reflector.get<number>('acl', context.getHandler());
    if (bitfield) {
      // Should be provided by AuthGuard
      const user = request['user'] as Users;
      const project = await this.projectRepository.findOneOrFail(
        request.params['project'],
      );

      if (project.ownerId === user.id) return true;

      const access = await this.aclService.getProjectAccess(user, project);

      if (!access.test(bitfield)) {
        throw new ForbiddenException({
          message: 'You do not have enough permissions to perform this action',
        });
      }
    }

    return true;
  }
}
