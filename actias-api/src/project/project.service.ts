import { EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable } from '@nestjs/common';
import { Projects } from 'src/entities/Projects';
import { ResourceType, Resources } from 'src/entities/Resources';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';

@Injectable()
export class ProjectService {
  constructor(
    @InjectRepository(Projects)
    private readonly projectRepository: EntityRepository<Projects>,
    @InjectRepository(Resources)
    private readonly resourceRepository: EntityRepository<Resources>,
  ) {}

  async createProject(owner: string, name: string): Promise<Projects> {
    const project = new Projects({
      ownerId: owner,
      name,
    });

    await this.projectRepository.persistAndFlush(project);
    return project;
  }

  async findById(id: string): Promise<Projects> {
    return await this.projectRepository.findOneOrFail({ id });
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

    await this.resourceRepository.persistAndFlush(resource);
    return resource;
  }

  /**
   * Get resource owned by a project.
   */
  async getResource(type: ResourceType, project: Projects, id: string) {
    return await this.resourceRepository.findOneOrFail({
      resourceType: type,
      project,
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
    const [resources, count] = await this.resourceRepository.findAndCount(
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
