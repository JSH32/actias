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
    if (project.owner === user) {
      return new BitField(AccessFields.FULL);
    }

    const permissions = await this.em.findOne(Access, { project, user });

    if (!permissions) {
      throw new ForbiddenException(`You can't access this project.`);
    }

    return BitField.deserialize(permissions.permissionBitfield);
  }

  async setPermissions(
    project: Projects,
    user: Users,
    permissions: string[],
  ): Promise<string> {
    const bitfield = new BitField<AccessFields>();

    for (const p of permissions) {
      const perm = AccessFields[p];
      if (!perm) throw new BadRequestException(`Invalid permission: ${p}`);
      bitfield.on(perm);
    }

    const access = await this.em.findOneOrFail(Access, { project, user });

    if (bitfield.serialize() !== '0') {
      access.permissionBitfield = bitfield.serialize();
      await this.em.persistAndFlush(project);
    }

    return bitfield.serialize();
  }

  /**
   * Get ACL list for a single user.
   */
  async getAclList(user: Users, project: Projects) {
    if (project.owner === user) {
      const bitfield = new BitField();
      bitfield.on(AccessFields.FULL);

      return new AclListDto({
        userId: user.id,
        permissions: getListFromBitfield(bitfield.serialize()),
      });
    }

    const access = await this.em.findOneOrFail(Access, { project, user });
    if (!access) {
      return new AclListDto({
        userId: user.id,
        permissions: getListFromBitfield('0'),
      });
    }

    return new AclListDto({
      userId: user.id,
      permissions: getListFromBitfield(access.permissionBitfield),
    });
  }
}
