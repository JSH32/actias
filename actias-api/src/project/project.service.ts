import { EntityManager } from '@mikro-orm/core';
import { Injectable } from '@nestjs/common';
import { Access } from 'src/entities/Access';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';
import { AccessFields } from './acl/accessFields';
import { BitField } from 'easy-bits';

@Injectable()
export class ProjectService {
  constructor(private readonly em: EntityManager) {}

  async createProject(owner: Users, name: string): Promise<Projects> {
    const project = new Projects({
      owner,
      name,
    });

    const access = new Access({
      user: owner,
      project,
      permissionBitfield: new BitField<AccessFields>()
        .on(AccessFields.FULL)
        .serialize(),
    });

    await this.em.persistAndFlush(access);
    await this.em.persistAndFlush(project);
    return project;
  }

  async findById(id: string): Promise<Projects> {
    return await this.em.findOneOrFail(Projects, { id });
  }

  async deleteProject(project: Projects) {
    const entity = await this.em.find(
      Projects,
      { id: project.id },
      { populate: true },
    );

    return await this.em.removeAndFlush(entity);
  }

  /**
   * Get all projects that a user has access to.
   *
   * @param user
   */
  async getAll(
    user: Users,
    pageSize: number,
    page: number,
  ): Promise<PaginatedResponseDto<Projects>> {
    const [projects, count] = await this.em.findAndCount(
      Projects,
      {
        $or: [{ owner: user }, { access: { user } }],
      },
      { limit: pageSize, offset: page * pageSize, populate: ['access'] },
    );

    return PaginatedResponseDto.fromArray(
      page,
      Math.ceil(count / pageSize),
      projects,
    );
  }
}
