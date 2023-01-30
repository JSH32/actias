import { Entity, ManyToOne } from '@mikro-orm/core';
import { BaseEntity } from './BaseEntity';
import { Projects } from './Projects';

enum ResourceType {
  SCRIPT = 'script',
}

@Entity()
export class Resources extends BaseEntity {
  resourceType!: ResourceType;

  @ManyToOne()
  project!: Projects;
}
