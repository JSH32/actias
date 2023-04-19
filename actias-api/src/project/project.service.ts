import { EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable } from '@nestjs/common';
import { Projects } from 'src/entities/Projects';

@Injectable()
export class ProjectService {
  constructor(
    @InjectRepository(Projects)
    private readonly projectRepository: EntityRepository<Projects>,
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
}
