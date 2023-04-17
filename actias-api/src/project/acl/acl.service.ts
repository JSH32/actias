import { EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable } from '@nestjs/common';
import { BitField, BitSet } from 'easy-bits';
import { Access } from 'src/entities/Access';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { AccessFields } from './accessFields';

@Injectable()
export class AclService {
  constructor(
    @InjectRepository(Access)
    private readonly accessRepository: EntityRepository<Access>,
  ) {}

  /**
   * Get access bitfield for a user within a project scope.
   */
  async getProjectAccess(
    user: Users,
    project: Projects,
  ): Promise<BitSet<AccessFields>> {
    return BitField.deserialize(
      (
        (await this.accessRepository.findOne({
          user,
          project,
        })) || project.defaultPermissions
      ).toString(),
    );
  }
}
