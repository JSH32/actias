import { EntityManager } from '@mikro-orm/core';
import { Injectable } from '@nestjs/common';
import { Projects } from 'src/entities/Projects';
import { ResourceType, Resources } from 'src/entities/Resources';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';

@Injectable()
export class ProjectService {
  constructor(private readonly em: EntityManager) {}

  async createProject(owner: string, name: string): Promise<Projects> {
    const project = new Projects({
      ownerId: owner,
      name,
    });

    await this.em.persistAndFlush(project);
    return project;
  }

  async findById(id: string): Promise<Projects> {
    return await this.em.findOneOrFail(Projects, { id });
  }

  async createResource(
    project: Projects,
    type: ResourceType,
    serviceId: string,
  ) {
    const resource = new Resources({
      serviceId,
      resourceType: type,
      project,
    });

    await this.em.persistAndFlush(resource);
    return resource;
  }

  /**
   * Get a resource by its type/id.
   */
  async getResource(type: ResourceType, id: string) {
    return await this.em.findOneOrFail(Resources, {
      resourceType: type,
      id,
    });
  }

  async listResources<T>(
    type: ResourceType,
    project: Projects,
    page: number,
    callback: (serviceId: Resources) => Promise<T>,
    pageSize = 25,
  ): Promise<PaginatedResponseDto<T>> {
    const [resources, count] = await this.em.findAndCount(
      Resources,
      {
        resourceType: type,
        project,
      },
      { limit: pageSize, offset: pageSize * page },
    );

    return new PaginatedResponseDto({
      page,
      lastPage: count / pageSize,
      items: await Promise.all(resources.map((res) => callback(res))),
    });
  }
}
