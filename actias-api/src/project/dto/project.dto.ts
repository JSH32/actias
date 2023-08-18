import { Projects } from 'src/entities/Projects';

export class ProjectDto {
  id!: string;
  /**
   * Name of project.
   */
  name!: string;
  /**
   * Owner of project, has full access.
   */
  ownerId!: string;
  createdAt: Date;
  updatedAt: Date;

  constructor(entity: Projects) {
    return Object.assign(this, {
      id: entity.id,
      name: entity.name,
      created: entity.createdAt,
      ownerId: entity.owner.id,
      createdAt: entity.createdAt,
      updatedAt: entity.updatedAt,
    });
  }
}
