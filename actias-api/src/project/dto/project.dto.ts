import { Projects } from 'src/entities/Projects';

export class ProjectDto {
  id!: string;
  name!: string;
  ownerId!: string;
  createdAt: Date;
  updatedAt: Date;

  constructor(entity: Projects) {
    return Object.assign(this, {
      id: entity.id,
      created: entity.createdAt,
      ownerId: entity.ownerId,
      createdAt: entity.createdAt,
      updatedAt: entity.updatedAt,
    });
  }
}
