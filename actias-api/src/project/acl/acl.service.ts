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
import { UserDto } from 'src/users/dto/user.dto';
import { WsException } from '@nestjs/websockets';

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
    isWs = false,
  ): Promise<BitSet<AccessFields>> {
    if (project.owner === user) {
      return new BitField(AccessFields.FULL);
    }

    const permissions = await this.em.findOne(Access, { project, user });

    if (!permissions) {
      const ExceptionClass = isWs ? WsException : ForbiddenException;
      throw new ExceptionClass(`You can't access this project.`);
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

    let access = await this.em.findOne(Access, { project, user });

    if (bitfield.serialize() !== '0') {
      if (!access) {
        access = new Access({
          user,
          project,
        });
      }

      access.permissionBitfield = bitfield.serialize();
      await this.em.persistAndFlush(access);
    } else {
      if (access) {
        await this.em.removeAndFlush(access);
      }
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
        user: new UserDto(user),
        permissions: getListFromBitfield(bitfield.serialize()),
      });
    }

    const access = await this.em.findOneOrFail(Access, { project, user });
    if (!access) {
      return new AclListDto({
        user: new UserDto(user),
        permissions: getListFromBitfield('0'),
      });
    }

    return new AclListDto({
      user: new UserDto(user),
      permissions: getListFromBitfield(access.permissionBitfield),
    });
  }
}
