import {
  BadRequestException,
  ForbiddenException,
  Injectable,
} from '@nestjs/common';
import { BitField, BitSet } from 'easy-bits';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { AccessFields, getListFromBitfield } from './accessFields';
import { Access } from 'src/entities/Access';
import { AclListDto } from './dto/acl.dto';
import { EntityManager } from '@mikro-orm/core';

@Injectable()
export class AclService {
  constructor(private readonly em: EntityManager) {}

  /**
   * Get access bitfield for a user within a project scope.
   * This raises {@link ForbiddenException} if the user can't access the project.
   */
  async getProjectAccess(
    user: Users,
    project: Projects,
  ): Promise<BitSet<AccessFields>> {
    if (project.ownerId === user.id) {
      return new BitField(AccessFields.FULL);
    }

    const permissions = project.access.find((access) => access.user === user);
    if (!permissions) {
      throw new ForbiddenException(`You can't access this project.`);
    }

    return BitField.deserialize(permissions.toString());
  }

  setPermissions(
    project: Projects,
    user: Users,
    permissions: string[],
  ): number {
    const bitfield = new BitField<AccessFields>();

    for (const p of permissions) {
      const perm = AccessFields[p];
      if (!perm) throw new BadRequestException(`Invalid permission: ${p}`);
      bitfield.on(perm);
    }

    // Remove (if exist).
    const access = project.access.filter((a) => a.user.id !== user.id);

    if (bitfield.serialize() !== '0') {
      access.push(
        new Access({
          user,
          permissionBitfield: Number(bitfield.serialize()),
        }),
      );
    }

    project.access = access;

    this.em.persistAndFlush(project);

    return Number(bitfield.serialize());
  }

  /**
   * Get ACL list for a single user.
   */
  getAclList(user: Users, project: Projects) {
    if (project.ownerId === user.id) {
      const bitfield = new BitField();
      bitfield.on(AccessFields.FULL);

      return new AclListDto({
        userId: user.id,
        permissions: getListFromBitfield(Number(bitfield.serialize())),
      });
    }

    const access = project.access.find((a) => a.user.id === user.id);
    if (!access) {
      return new AclListDto({
        userId: user.id,
        permissions: getListFromBitfield(0),
      });
    }

    return new AclListDto({
      userId: user.id,
      permissions: getListFromBitfield(access.permissionBitfield),
    });
  }
}
