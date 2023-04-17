import {
  CanActivate,
  ExecutionContext,
  ForbiddenException,
  Injectable,
  SetMetadata,
} from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { AclService } from './acl.service';
import { ProjectService } from '../project.service';
import { Reflector } from '@nestjs/core';

export const Acl = (bitfield: number) => SetMetadata('acl', bitfield);

@Injectable()
export class AclGuard implements CanActivate {
  constructor(
    private aclService: AclService,
    private projectService: ProjectService,
    private reflector: Reflector,
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();

    const bitfield = this.reflector.get<number>('acl', context.getHandler());
    if (bitfield) {
      // Should be provided by AuthGuard
      const user = request['user'] as Users;
      const project = await this.projectService.findById(
        request.params['project'],
      );

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
