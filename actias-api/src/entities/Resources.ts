import { Entity, ManyToOne, PrimaryKey } from '@mikro-orm/core';
import { Projects } from './Projects';

@Entity()
export class Resources {
  @PrimaryKey({ columnType: 'resource_type' })
  resourceType!: unknown;

  @PrimaryKey({ columnType: 'uuid' })
  resourceId!: string;

  @ManyToOne({ entity: () => Projects, onDelete: 'cascade' })
  project!: Projects;
}
